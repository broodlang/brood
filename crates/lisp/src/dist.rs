//! Distributed nodes: connect two Brood runtimes and route messages between
//! them. Two nodes on one machine speak over a **Unix-domain socket** addressed
//! by name (no port); across machines, over **TCP**. The handshake, framing,
//! heartbeat and teardown are identical over both — only the carrier ([`Stream`])
//! differs (ADR-068). Distribution falls out of share-nothing +
//! copy-on-send — *the network is just a longer copy* (ADR-013, `concurrency.md`).
//!
//! **Slice 1 (this module):** node naming, an authenticated TCP handshake (a
//! shared cookie), and
//! location-transparent [`send`](crate::process::send) to a remote process. A
//! process is addressed either by a [`Value::Pid`](crate::core::value::Value::Pid)
//! — which carries node identity, so the same value works locally or across the
//! link — or, to bootstrap before you hold a peer's pid, by a `{:name :node}`
//! registered-name address.
//!
//! **One node per OS process.** The node identity, connection table, name table
//! and symbol interner are process-global, so a "node" *is* the OS process; two
//! nodes are two `brood` processes (typically over loopback). (Everything the
//! original slice-1 doc deferred — remote `spawn`/code shipping, distributed
//! monitors, node-down detection, reconnect, and channel encryption — has since
//! landed; see the §sections below and `docs/distribution.md`.)
//!
//! ## Threads (off the green-process scheduler)
//! Each connection owns two plain OS threads — a **reader** (decodes inbound
//! frames and hands messages to [`process::deliver`]) and a **writer** (drains an
//! `mpsc` channel onto the socket). They never touch the coroutine scheduler;
//! inbound messages land in a local mailbox exactly as an in-process `send` would.
//!
//! ## Wire codec
//! Hand-rolled and length-prefixed (`[u32 len][payload]`). It reuses the existing
//! [`Message`] deep-copy, with one cross-process detail: **symbols travel by
//! name**, re-interned on arrival, because separate runtimes have independent
//! interners.
//!
//! ## Channel security
//! The handshake authenticates the peer with a shared-cookie HMAC (the cookie is
//! never on the wire), and the steady-state link is then **encrypted + integrity-
//! protected** by a Noise-style session: ephemeral X25519 ECDH (forward secrecy,
//! authenticated by the cookie-HMAC) → ChaCha20-Poly1305 per frame, with the send
//! and receive ciphers owned by the writer and reader threads respectively (see
//! [`session`] and [`handshake`], ADR-089). So a TCP node is safe on an untrusted
//! network — a passive observer learns nothing and a post-handshake forged frame
//! (e.g. a `Send` carrying a closure → RCE) fails the per-frame tag.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;
use web_time::Instant;

use crate::core::value::{self, Symbol};
use crate::process::keywords as pk;
use crate::process::{self, Message};

/// Hard ceiling on a single wire frame (bytes). A peer can otherwise put any
/// `u32` in the length prefix and make us allocate it sight unseen — including
/// random bytes from a port scan or a stray HTTP request hitting the port. Cap
/// it so a bad/oversized frame is rejected, not OOM'd. 64 MiB is far above any
/// real message.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Hard ceiling on a frame read *during the handshake*, before the peer is
/// authenticated. A `Hello` (a short node name + a 32-byte nonce) or `Auth` (a
/// 32-byte MAC) is only tens of bytes; even a long FQDN node name stays well
/// under this. Capping the pre-auth read here — rather than at the 64 MiB
/// steady-state [`MAX_FRAME`] — stops an *unauthenticated* peer from making us
/// `vec![0u8; 64MiB]` off an 8-byte probe (magic + an oversized length prefix).
/// 4 KiB is generous headroom over any real handshake frame.
const MAX_HANDSHAKE_FRAME: usize = 4 * 1024;

/// Cap on inbound handshakes *in flight at once*. Each accepted connection
/// holds a slot from accept until its handshake finishes (success, failure, or
/// the [`HANDSHAKE_TIMEOUT`] firing); a steady-state link holds none. Without
/// this an attacker reachable on a TCP listener can open unbounded connections
/// — each spawning an OS thread, arming the 10 s timeout, and able to commit a
/// [`MAX_HANDSHAKE_FRAME`] allocation — *before* authenticating, exhausting
/// threads/FDs/memory. Past the cap we shed the connection (close it) without
/// spawning a thread or logging (logging per-shed would itself be a flood
/// vector). 128 is far above any realistic simultaneous-peer fan-in, which is
/// rare and bursty; legitimate peers retry.
const MAX_IN_FLIGHT_HANDSHAKES: usize = 128;

/// Live count of in-flight inbound handshakes, gated by [`MAX_IN_FLIGHT_HANDSHAKES`]
/// via [`HandshakeSlot`].
static IN_FLIGHT_HANDSHAKES: AtomicUsize = AtomicUsize::new(0);

/// Bound a *single* read during a handshake, so a peer that connects and then
/// goes silent can't pin a thread forever (the steady-state reader has the
/// timeout cleared — it *should* block until the next message arrives).
///
/// **This is a per-read bound, not a per-handshake one**, which on its own is no
/// defence at all: it is `SO_RCVTIMEO`, so it restarts on every byte that
/// arrives. A peer dribbling one byte every 9 s satisfies it forever — a 4 KiB
/// pre-auth frame would take ~10 hours, all of it holding a [`HandshakeSlot`],
/// and 128 such sockets shut inbound dist down completely. The whole-handshake
/// wall clock is [`HANDSHAKE_DEADLINE`]; both are needed (this one bounds a
/// silent peer between bytes, that one bounds a slow one across them).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Wall-clock budget for an ENTIRE handshake, enforced by [`Deadline`] across
/// every pre-auth read and write. Unlike [`HANDSHAKE_TIMEOUT`] this cannot be
/// restarted by trickling data, so it is what actually bounds how long an
/// unauthenticated peer may hold a slot. Generous next to a real handshake (four
/// small frames on an established TCP connection — microseconds on a LAN, and
/// the dialer's own connect timeout is already 5 s).
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(15);

/// Timeout on dialer socket connect. Without this, `TcpStream::connect(addr)` blocks
/// at the kernel's TCP SYN timeout (minutes on Linux) when the peer's port is
/// silently dropping packets — fine for a healthy LAN, but on a flaky network the
/// dialer wedges. Several seconds is enough for a real LAN/WAN round-trip.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-write timeout on the steady-state writer socket. A peer that stops reading
/// (slowloris-style) drives its TCP receive window to zero; without this, our
/// `write_all` blocks forever and the writer thread is pinned. Generous so a
/// genuinely slow peer doesn't get torn down for an occasional slow drain. The
/// companion guard against the backlog ballooning while the writer is stalled is
/// the **bounded** writer queue (`WRITER_QUEUE_CAP` / [`Conn::enqueue`]); this
/// timeout only bounds a single `write_all`.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound on the per-link writer queue (frames awaiting send). The channel is a
/// `sync_channel`, not an unbounded `channel`: a peer that stalls its TCP read
/// window stops the writer draining, and an unbounded queue would let local
/// producers (`route`, mesh gossip, link/monitor signals, …) balloon the backlog
/// into a remote-controlled OOM. On overflow [`Conn::enqueue`] **severs the link**
/// rather than block a producer or buffer without limit — a peer that lets this
/// many frames back up is wedged and better disconnected; the reader's `drop_link`
/// then deregisters it and watchers learn it's unreachable.
///
/// Sized generously so a *transiently* slow-but-healthy link isn't severed for a
/// burst (one `write_all` can block up to `WRITE_TIMEOUT`): a frame *count*, not a
/// byte ceiling, so worst-case memory is `CAP × frame size` — fine for the small
/// frames that dominate, bounded even for large ones. If false-severance of a
/// genuinely slow peer ever bites, the precise follow-up is an outstanding-*bytes*
/// ceiling per `Conn` (the audit's alternative), not a bigger count.
const WRITER_QUEUE_CAP: usize = 4096;

/// How many sealed bytes the link writer gathers from its queue before one `write_all`:
/// frames already queued behind the one it woke for join that write until the batch
/// passes this (a single larger frame is written whole). A bound on what the writer holds
/// off the queue, on top of `WRITER_QUEUE_CAP`, not a wait — a batch is never held open.
const WRITE_BATCH_BYTES: usize = 256 * 1024;

/// Minimum node-cookie length (bytes) accepted by [`node_listen`]. The cookie
/// is the whole trust boundary (possession ⇒ remote eval), and the HMAC
/// imposes no strength requirement of its own — an empty or few-byte cookie
/// authenticates "successfully" and is guessable online. 16 bytes of a
/// `(rand/token …)`-style secret is far beyond online brute force; the
/// default `(node/cookie)` generates 32.
const MIN_COOKIE_LEN: usize = 16;

/// Monotonic clock base, so `last_seen` can live in an `AtomicU64` of millis.
/// `dist::heartbeat` reads this same clock; keep the source here at the root
/// so the readers (link establishment, reader thread) and the writer
/// (`heartbeat_loop`) share one zero point.
static START: LazyLock<Instant> = LazyLock::new(Instant::now);
fn now_millis() -> u64 {
    START.elapsed().as_millis() as u64
}

// ----- transport (the link carrier) ------------------------------------------

/// A live link's byte stream. The whole protocol above it — handshake, framing,
/// heartbeat, teardown — is transport-agnostic, so this enum is the *only* place
/// TCP-vs-Unix matters. The reader/writer threads hold an `Arc<Stream>` and do
/// I/O through `&Stream`, mirroring the `&TcpStream: Read` shape std provides;
/// the handshake runs over `&mut Stream` before the link goes steady-state.
enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Stream {
    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.shutdown(how),
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(how),
        }
    }
    fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_read_timeout(d),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(d),
        }
    }
    fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_write_timeout(d),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_write_timeout(d),
        }
    }
}

// Owned-stream I/O: the handshake drives `&mut Stream` (`TcpStream`/`UnixStream`
// each impl `Read`/`Write`).
impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
        }
    }
}

// Shared-ref I/O: the reader (`&*sock`) and writer (`(&*sock).write_all`) hold an
// `Arc<Stream>` and never have `&mut`, exactly like `&TcpStream: Read` in std.
impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match **self {
            Stream::Tcp(ref s) => (&*s).read(buf),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).read(buf),
        }
    }
}
impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match **self {
            Stream::Tcp(ref s) => (&*s).write(buf),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match **self {
            Stream::Tcp(ref s) => (&*s).flush(),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).flush(),
        }
    }
}

// ----- node identity ---------------------------------------------------------

struct NodeIdentity {
    name: Symbol,
    cookie: String,
    started: bool,
}

/// The name a pid carries before `node-start` runs: every such pid is local.
static NONODE: LazyLock<Symbol> = LazyLock::new(|| value::intern(pk::NONODE));

static NODE: LazyLock<RwLock<NodeIdentity>> = LazyLock::new(|| {
    RwLock::new(NodeIdentity {
        name: *NONODE,
        cookie: String::new(),
        started: false,
    })
});

/// A lock-free cache of this node's name (the `NODE` lock holds the cookie too,
/// but the *name* is read on every `send` — see `is_local`/`route` — so we keep
/// it in an atomic to keep that hot path off the lock). `u32::MAX` is the
/// "unset" sentinel (→ `:nonode`); a real symbol id never reaches it.
static LOCAL_NODE: AtomicU32 = AtomicU32::new(u32::MAX);

/// This runtime's node name (interned). `:nonode` until `node-start`. Lock-free.
pub(crate) fn local_node() -> Symbol {
    // `Acquire` pairs with the `Release` `store` in `node_start` — any reader
    // that sees the published name is also guaranteed to see the `NODE`
    // lock's writes (cookie + name) made before that store.
    match LOCAL_NODE.load(Ordering::Acquire) {
        u32::MAX => *NONODE,
        id => id,
    }
}

/// Is `node` *us* (or a pre-`node-start` `:nonode` pid)? Such targets deliver
/// in-process rather than over a link.
pub(crate) fn is_local(node: Symbol) -> bool {
    node == *NONODE || node == local_node()
}

// ----- connection + name tables ----------------------------------------------

/// A live link to a peer node.
struct Conn {
    /// A generation id, unique per physical connection. Teardown removes a `NODES`
    /// entry only if the stored link still has *this* id, so an evicted/old link's
    /// reader can't clobber a newer replacement (see `drop_link`).
    id: u64,
    /// Which node *initiated* this link. The tie-break for a duplicate keeps the
    /// link initiated by the lexicographically smaller node name, computed
    /// identically on both ends (see `establish`).
    connector: Symbol,
    /// The peer's incarnation, from its authenticated `Hello`: which life of the
    /// peer this link reaches. A new link under the same name with a different one
    /// means the peer restarted, and this link is to a process that no longer
    /// exists (see `establish`).
    incarnation: u64,
    /// The address a *third* node should dial to reach this peer (`"unix:PATH"` /
    /// `"tcp:HOST:PORT"`, or empty if the peer didn't advertise one), learned from
    /// the peer's authenticated `Hello`. We gossip this to other peers so the
    /// cluster meshes (ADR-088).
    addr: String,
    /// The writer thread's inbox (length-framed bytes). **Bounded**
    /// (`WRITER_QUEUE_CAP`): see [`Conn::enqueue`] — a stalled peer can't balloon
    /// it into an OOM. Outbound frames carry an `Arc<Vec<u8>>` so liveness probes
    /// (one `ping` per tick, one `pong` per inbound `Ping`) reuse a single buffer
    /// per link instead of cloning a `Vec<u8>` each time — and an `Arc<Vec>`, not an
    /// `Arc<[u8]>`, so wrapping a freshly encoded frame takes its buffer as it is
    /// instead of copying it into a new allocation.
    tx: SyncSender<Outbound>,
    /// Whether the peer speaks v11, so a send in a lane goes as `FRAME_SEND_LANE`
    /// (ADR-492). A v10 peer is sent every message as the plain `Send` it knows.
    lanes: bool,
    /// The `proc/send-latest` frames queued on this link and not yet written, one per
    /// target and key (ADR-492) — shared with the writer, which writes a ticket's frame
    /// only if no newer one has replaced it. Whatever the peer speaks: replacing a frame
    /// before it is written is this end's business.
    latest: LatestSlots,
    /// A handle to the socket, for `shutdown` — the single teardown lever.
    sock: Arc<Stream>,
    /// Millis (on the `START` clock) of the last inbound frame. The heartbeat
    /// thread reads this to decide liveness; the reader writes it.
    last_seen: Arc<AtomicU64>,
    /// `Some` for a GUEST link (ADR-415): its grant and the pids it has been given, the
    /// same `Arc` the link's reader filters every frame through. Fixed at `establish` for
    /// this one connection, so whichever link holds the peer's NAME later cannot change
    /// what this link's frames may do (KI-286).
    guest: Option<Arc<guest::GuestLink>>,
}

impl Conn {
    /// Hand a sealed-frame payload to the writer thread. The queue is **bounded**
    /// (`WRITER_QUEUE_CAP`), so this never blocks a producer and never buffers
    /// without limit: if the queue is `Full` — the peer has stalled its read
    /// window so the writer can't drain — or `Disconnected` (the writer has gone),
    /// it **severs the link** by shutting the socket down. The reader thread
    /// observes the shutdown and runs `drop_link`, deregistering this `Conn`.
    /// Returns whether the payload was accepted onto the queue; callers that must
    /// report unreachability (`route`, `link_remote`) use it to fire
    /// `:noconnection`, while best-effort signals ignore it.
    /// The pieces [`enqueue_to`] needs, cloned so the caller can drop the `NODES`
    /// lock before enqueueing. Both clones are cheap (a channel handle and an `Arc`).
    fn writer_handles(&self) -> (SyncSender<Outbound>, Arc<Stream>) {
        (self.tx.clone(), self.sock.clone())
    }
}

/// Hand `bytes` to a link's writer thread, severing the link if its queue is full.
///
/// Free-standing so a caller can run it **without holding `NODES`**: the `Err` arm
/// issues a `shutdown` syscall, and that must not run under the `NODES` lock or it
/// delays every concurrent link registration and teardown for the syscall's
/// duration — the rule [`broadcast_peer_table`] documents. So never call
/// `Conn::enqueue` straight out of a `read(&NODES).get(..)`.
fn enqueue_to(tx: &SyncSender<Outbound>, sock: &Arc<Stream>, bytes: Arc<Vec<u8>>) -> bool {
    match tx.try_send(bytes.into()) {
        Ok(()) => true,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            false
        }
    }
}

/// What a link's writer queue holds: a frame payload, or a ticket for the newest
/// `proc/send-latest` frame to one target and key (ADR-492), whose payload waits in the
/// link's [`LatestSlots`] so a newer one can replace it before it is written.
pub(crate) enum Outbound {
    Frame(Arc<Vec<u8>>),
    Latest { slot: String, generation: u64 },
}

impl From<Arc<Vec<u8>>> for Outbound {
    fn from(payload: Arc<Vec<u8>>) -> Self {
        Outbound::Frame(payload)
    }
}

/// A link's queued `proc/send-latest` frames: slot (target and key) → the generation
/// and payload of the newest one sent and not yet written.
type LatestSlots = Arc<Mutex<HashMap<String, (u64, Arc<Vec<u8>>)>>>;

/// Source of [`Outbound::Latest`] generations.
static NEXT_LATEST: AtomicU64 = AtomicU64::new(0);

/// The slot a `proc/send-latest` to `target` with `key` (its printed form) is queued in:
/// one per target, since two processes' keys are not each other's.
fn latest_slot(target: &Target, key: &str) -> String {
    match target {
        Target::Pid(id) => format!("pid {id} {key}"),
        Target::Name(name) => format!("name {} {key}", value::symbol_name(*name)),
    }
}

/// Queue a `proc/send-latest` frame on a link: its payload replaces any for the same
/// slot still unwritten, and its ticket goes to the back of the queue, so the frame is
/// written after everything sent before it — what the receiving mailbox does with a
/// LATEST message (ADR-490), done here before a slow link ships a stale copy. Severs the
/// link when the queue is full, as [`enqueue_to`] does.
fn enqueue_latest(
    tx: &SyncSender<Outbound>,
    sock: &Arc<Stream>,
    latest: &LatestSlots,
    slot: String,
    payload: Arc<Vec<u8>>,
) -> bool {
    let generation = NEXT_LATEST.fetch_add(1, Ordering::Relaxed);
    crate::core::sync::lock(latest).insert(slot.clone(), (generation, payload));
    match tx.try_send(Outbound::Latest { slot, generation }) {
        Ok(()) => true,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            false
        }
    }
}

/// The payload the writer writes for `item`: a frame's own, or a ticket's when it is
/// still the newest for its slot (which it then leaves). A ticket a later
/// `send-latest` replaced writes nothing; the later one's ticket is behind it.
fn claim_outbound(latest: &LatestSlots, item: Outbound) -> Option<Arc<Vec<u8>>> {
    match item {
        Outbound::Frame(payload) => Some(payload),
        Outbound::Latest { slot, generation } => {
            let mut slots = crate::core::sync::lock(latest);
            match slots.get(&slot) {
                Some((newest, _)) if *newest == generation => {
                    slots.remove(&slot).map(|(_, payload)| payload)
                }
                _ => None,
            }
        }
    }
}

/// Source of per-connection generation ids.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

/// Connected peer node-name → its connection.
static NODES: LazyLock<RwLock<HashMap<Symbol, Conn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Dial address (`"unix:PATH"` / `"tcp:HOST:PORT"`, as `node_connect` was given it)
/// → the authenticated name of the node a handshake over it reached.
///
/// The name a caller hands `node_connect` is only a *claim* — `(node/connect "ed")`
/// claims `ed@<this host>`, while the node listening on that socket may call itself
/// `ed@127.0.0.1` — so checking `NODES` by the claimed name alone misses a live link
/// spelled differently and dials it again. The address is what actually identifies
/// the listener, so this lets the pre-dial check answer a second connect to a live
/// peer from the existing link whatever name it was spelled with. Entries are
/// dropped with their peer's link (`forget_dial_routes`), and a lookup is only
/// trusted while `NODES` still holds the name.
static DIAL_ROUTES: LazyLock<RwLock<HashMap<String, Symbol>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// This runtime process's **incarnation**: a random id minted once, sent in every
/// `Hello`. A node name outlives a process — a crashed node comes back under the same
/// name — so the name alone cannot tell a *restarted* peer's new link from a redundant
/// second link to the *same* live peer. The incarnation can: `establish` replaces a
/// registered link whose incarnation differs (the peer restarted; the old link reaches
/// nothing), and resolves a same-incarnation duplicate by the connector tie-break
/// (both links reach the live peer, so nothing may be torn down for it).
static INCARNATION: LazyLock<u64> = LazyLock::new(|| {
    let mut bytes = [0u8; 8];
    match getrandom::fill(&mut bytes) {
        Ok(()) => u64::from_be_bytes(bytes),
        // No OS RNG: the handshake cannot mint its nonce either, so no link will form;
        // anything distinct per process is enough for the id itself.
        Err(_) => {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0);
            nanos ^ ((std::process::id() as u64) << 32)
        }
    }
});

/// This runtime's incarnation (see [`INCARNATION`]).
fn local_incarnation() -> u64 {
    *INCARNATION
}

/// Locally registered name → local process id, so a peer can address a process by
/// a stable name before anyone holds its pid (`(proc/register :echo (self))`).
static NAMES: LazyLock<RwLock<NameTable>> = LazyLock::new(|| RwLock::new(NameTable::default()));

/// The registered names, indexed both ways. `by_pid` exists for the death path: every
/// process exit sweeps its names, and with only `name → pid` that sweep was a pass over
/// EVERY registered name, under the write lock, per exit. Both maps change together
/// under the one `NAMES` lock, through the methods below and nowhere else, so each is
/// always the other's inverse.
#[derive(Default)]
struct NameTable {
    by_name: HashMap<Symbol, u64>,
    /// pid → the names bound to it, in binding order. Nearly always one name or none (an
    /// entry is removed when its last name goes).
    by_pid: HashMap<u64, Vec<Symbol>>,
}

impl NameTable {
    fn get(&self, name: Symbol) -> Option<u64> {
        self.by_name.get(&name).copied()
    }

    /// Bind `name` to `pid`, taking it from any previous holder.
    fn bind(&mut self, name: Symbol, pid: u64) {
        match self.by_name.insert(name, pid) {
            Some(previous) if previous == pid => return,
            Some(previous) => self.forget_name_of(previous, name),
            None => {}
        }
        self.by_pid.entry(pid).or_default().push(name);
    }

    /// Drop `name`'s binding, returning the pid it named.
    fn unbind(&mut self, name: Symbol) -> Option<u64> {
        let pid = self.by_name.remove(&name)?;
        self.forget_name_of(pid, name);
        Some(pid)
    }

    /// Drop every name bound to `pid`.
    fn unbind_pid(&mut self, pid: u64) {
        for name in self.by_pid.remove(&pid).unwrap_or_default() {
            self.by_name.remove(&name);
        }
    }

    fn holds_any(&self, pid: u64) -> bool {
        self.by_pid.contains_key(&pid)
    }

    /// The first name still bound to `pid`.
    fn first_name_of(&self, pid: u64) -> Option<Symbol> {
        self.by_pid
            .get(&pid)
            .and_then(|names| names.first().copied())
    }

    /// Remove `name` from `pid`'s reverse entry (the forward one is the caller's).
    fn forget_name_of(&mut self, pid: u64, name: Symbol) {
        if let Some(names) = self.by_pid.get_mut(&pid) {
            names.retain(|&held| held != name);
            if names.is_empty() {
                self.by_pid.remove(&pid);
            }
        }
    }
}

/// Node-name → pids that asked to watch it (`monitor-node`). Each gets a
/// `[:nodedown name]` message when a link to that node tears down.
static NODE_MONITORS: LazyLock<RwLock<HashMap<Symbol, Vec<u64>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The addresses this node listens on, in registration order (`"unix:PATH"` /
/// `"tcp:HOST:PORT"`). One entry per `node-start`/`node-also-listen` listener.
/// Read by [`advertised_addr`] to tell a peer how others should dial us.
static LISTEN_ADDRS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// Peer node-names we're *currently dialing* because cluster gossip named them
/// (ADR-088). Entered before a mesh dial thread spawns and cleared when it
/// finishes, so two gossip frames naming the same not-yet-connected peer don't
/// race into two redundant dials. A peer already in `NODES` is never re-dialed.
static PENDING_DIALS: LazyLock<RwLock<HashSet<Symbol>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// Whether to form a transitive **cluster mesh** (ADR-088): when set (the
/// default), connecting to one node auto-connects you to
/// every node it knows. Set `BROOD_NO_MESH=1` for point-to-point links only
/// (you connect to exactly the nodes you dial, no transitive discovery). Read
/// once at first use.
static MESH_ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("BROOD_NO_MESH").is_none());

fn mesh_enabled() -> bool {
    *MESH_ENABLED
}

/// The address a peer should advertise as "dial this to reach me" — the first
/// TCP listener if any (reachable from other machines *and* same-machine over
/// loopback), else the first listener (a local Unix socket), else empty (not
/// listening; peers then can't gossip us onward). Preferring TCP means a
/// dual-listen node (local Unix + remote TCP) advertises the address that works
/// from anywhere.
fn advertised_addr() -> String {
    let addrs = crate::core::sync::read(&LISTEN_ADDRS);
    addrs
        .iter()
        .find(|a| a.starts_with("tcp:"))
        .or_else(|| addrs.first())
        .cloned()
        .unwrap_or_default()
}

/// What [`register`] did.
pub(crate) enum Registration {
    /// `name` now names the pid, replacing any previous holder.
    Bound,
    /// The pid is not alive, so nothing was bound. Indistinguishable from binding it and
    /// the process dying a moment later — a name lives only as long as its process, so the
    /// death would have released it — which is why this is not an error: a supervisor that
    /// registers a child which crashed on its first line must not crash with it.
    DeadPid,
}

/// `(proc/register name pid)` — bind a local name to a local process id, replacing any
/// previous holder. Replacing is the contract every caller relies on (a restarted
/// logger, a test standing in as `:editor` and handing the name back), so a live
/// holder is not refused.
///
/// Decided under the `NAMES` write lock, against the same lock the death path holds
/// across its name sweep AND its `REGISTRY` removal ([`unregister_dead_pid_while`]), so
/// the liveness read here is exact: a pid this sees alive has not yet been swept, and
/// the sweep will find the binding. Without the check a dead pid's name was bound after
/// its sweep and stayed bound forever.
pub(crate) fn register(name: Symbol, id: u64) -> Registration {
    let mut names = crate::core::sync::write(&NAMES);
    if !process::is_alive(id) {
        return Registration::DeadPid;
    }
    names.bind(name, id);
    Registration::Bound
}

/// `(proc/whereis name)` — the local pid registered under `name`, or `None`. Lets
/// callers test for an existing registration before re-`spawn`ing a server
/// they're about to register (idempotent bootstrap; used by `remote-spawn`).
pub(crate) fn whereis(name: Symbol) -> Option<u64> {
    crate::core::sync::read(&NAMES).get(name)
}

/// `(proc/unregister name)` — drop `name`'s binding, returning whether one existed.
///
/// The inverse of [`register`], which had none: a name could be bound to a pid and never
/// released except by that process dying. A service that wants to hand its name to a
/// replacement, or step down without exiting, needs this.
pub(crate) fn unregister(name: Symbol) -> bool {
    crate::core::sync::write(&NAMES).unbind(name).is_some()
}

/// The name `pid` is registered under, if any — the reverse of [`whereis`].
/// Used by the scheduler's death reporter to name a crashed process
/// (`process ticker (pid 6) died: …`) instead of only its opaque pid. With several
/// names bound, the first still held. Must be read *before*
/// [`unregister_dead_pid_while`] clears the entry.
pub(crate) fn name_for_pid(pid: u64) -> Option<Symbol> {
    crate::core::sync::read(&NAMES).first_name_of(pid)
}

/// The death path's name sweep: remove every `NAMES` entry pointing at `pid` (a name
/// lives only as long as its process — else `whereis` returns a corpse and named-spawn
/// never respawns), and prune `pid` from every `NODE_MONITORS` watcher list.
///
/// `retire` (the `REGISTRY` removal) runs **inside** the `NAMES` lock with the sweep, so
/// no [`register`] can fall between them, see the pid still alive, and bind a name the
/// sweep has already passed — and a pid absent from `REGISTRY` is already absent from
/// `NAMES` (KI-80). NAMES → REGISTRY is the nesting [`spawn_or_get`] and [`register`]
/// already use, so this adds no new lock order.
///
/// A pid holding no name (nearly every one) retires under the READ lock, so exits do not
/// serialise on the table. That keeps both properties, because every binding is made
/// under the WRITE lock, which a held read lock excludes: a [`register`] that saw the pid
/// alive finished before this read and left a binding it sees (the write path below), and
/// one that runs after the read is released finds the pid already gone from `REGISTRY`.
pub(crate) fn unregister_dead_pid_while<R>(pid: u64, retire: impl FnOnce() -> R) -> R {
    let retired = {
        let names = crate::core::sync::read(&NAMES);
        if names.holds_any(pid) {
            drop(names);
            // Anything bound between the two locks is swept too: the write lock holds
            // off every further `register` until `retire` has run.
            let mut names = crate::core::sync::write(&NAMES);
            names.unbind_pid(pid);
            retire()
        } else {
            retire()
        }
    };
    // Prune the dead pid from every NODE_MONITORS watcher list — when it is on one. The
    // read is the sweep's linearisation point when it is not: a `monitor_node` naming
    // this pid after it is a registration after the sweep, which the unconditional write
    // lock admitted just the same (`monitor_node` does not check liveness).
    let watching = crate::core::sync::read(&NODE_MONITORS)
        .values()
        .any(|watchers| watchers.contains(&pid));
    if watching {
        let mut monitors = crate::core::sync::write(&NODE_MONITORS);
        for watchers in monitors.values_mut() {
            watchers.retain(|&w| w != pid);
        }
        monitors.retain(|_, v| !v.is_empty());
    }
    retired
}

/// Named-spawn's atomic check-or-spawn primitive. If `name` is registered
/// to a still-alive pid, return that pid and skip the spawn. Otherwise,
/// drop any stale entry, call `spawner` to create a fresh process, register
/// it under `name`, and return the new pid.
///
/// The whole sequence runs under the `NAMES` write lock so two concurrent
/// `(spawn :name …)` calls can't both spawn — the loser sees the winner's
/// pid and returns it. Inside, REGISTRY is briefly acquired **twice**:
/// once via `process::is_alive` for the staleness check, and once inside
/// `spawner()` (`process::spawn` inserts a new mailbox). Both are short
/// — sequential acquisitions, not held across awaits, never overlap with
/// each other. Lock-ordering vs `deregister` is safe: deregister nests its
/// REGISTRY removal INSIDE NAMES ([`unregister_dead_pid_while`]) — the same
/// NAMES → REGISTRY order as here — and never holds REGISTRY while reaching
/// for NAMES, so no cycle can form.
///
/// `spawner` is **fallible** — if creating the process errors (e.g. a
/// type-check or heap-promotion failure), we propagate without inserting
/// into NAMES, so a failed spawn leaves no stale entry behind.
pub(crate) fn spawn_or_get<E>(
    name: Symbol,
    spawner: impl FnOnce() -> Result<u64, E>,
) -> Result<u64, E> {
    let mut names = crate::core::sync::write(&NAMES);
    if let Some(existing) = names.get(name) {
        if process::is_alive(existing) {
            return Ok(existing);
        }
        // Stale (the process registered under this name has died); drop and
        // fall through to a fresh spawn.
        names.unbind(name);
    }
    let pid = spawner()?;
    names.bind(name, pid);
    Ok(pid)
}

/// Encode `frame` and enqueue it on the link to `target_node`. Returns whether
/// the bytes actually made it onto that link's writer queue: `false` if the link
/// is down (no `NODES` entry) *or* its bounded send queue is full/severed. This
/// is the single send path for every control frame (monitor/link/exit family),
/// so their error handling is uniform — the two callers that care about delivery
/// (`monitor_remote`, `link_remote`) branch on the bool to fire `:noconnection`;
/// the rest are best-effort and ignore it.
///
/// Error-handling choice: a failed *encode* is logged via `eprintln!` (adapted
/// from the old `monitor_remote`, the better of the two prior tails — the others
/// silently swallowed it) rather than dropped, because a control frame failing to
/// encode is a real bug, not an expected condition — its only variable-width field
/// is `Exit`'s `reason`, and an oversized reason that can't be shipped is worth a
/// diagnostic. A failed encode counts as not-sent.
///
/// A frame to a GUEST link names the guest's pids the way the guest spells itself
/// (ADR-425): the link's [`guest::GuestLink::translate_outbound`] rewrites it first.
fn send_frame(target_node: Symbol, mut frame: Frame) -> bool {
    // Snapshot the writer handles, then release NODES before encoding and enqueueing —
    // see [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let link = crate::core::sync::read(&NODES)
        .get(&target_node)
        .map(|conn| (conn.writer_handles(), conn.guest.clone()));
    let Some(((tx, sock), guest)) = link else {
        return false;
    };
    if let Some(guest) = &guest {
        guest.translate_outbound(&mut frame);
    }
    let bytes: Arc<Vec<u8>> = match encode_payload(&frame) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode control frame for {}: {}",
                value::symbol_name(target_node),
                e
            );
            return false;
        }
    };
    enqueue_to(&tx, &sock, bytes)
}

/// `(monitor (Pid remote_node remote_pid))` from the cross-node path: ship a
/// `Frame::Monitor` to the peer and record the pending remote watcher locally
/// (so net-split can fire `:noconnection` to the watcher even though the
/// monitor target lives elsewhere). If the peer link isn't up, deliver
/// `:noconnection` immediately — same shape an immediately-dead local target
/// gets (`:noproc` from `add_monitor`), just a different reason.
pub(crate) fn monitor_remote(target_node: Symbol, target_pid: u64, watcher_pid: u64, mref: u64) {
    // Record the pending entry **before** sending. This closes a race against
    // `drop_link`/`handle_node_down`:
    //   • If we record before they run, they'll find our entry in
    //     `PENDING_REMOTE` and fire `:noconnection` to us — even if our send
    //     never made it onto the wire.
    //   • If they run first (`NODES` already empty when `send_frame` looks),
    //     `send_frame` returns false and we fall through to the explicit cleanup
    //     below, dropping our pending entry and firing `:noconnection` ourselves.
    // The pending entry can't be orphaned in either branch. And it fires ONCE: when
    // the link drops after the record but before the send, `handle_node_down` has
    // already claimed the entry and fired, the send then fails, and the fallback finds
    // nothing left to claim, so no second `[:down mref … :noconnection]` fires.
    process::record_pending_remote(target_node, target_pid, watcher_pid, mref);
    #[cfg(test)]
    tests::run_setup_gap_hook();
    let sent = send_frame(
        target_node,
        Frame::Monitor {
            watcher_pid,
            target: target_pid,
            mref,
        },
    );
    if !sent {
        process::fire_noconnection_if_pending(target_node, target_pid, watcher_pid, mref);
    }
}

/// `(monitor {:name n :node target_node})`: [`monitor_remote`] for a process registered
/// under `name` on the peer, which resolves it on arrival (`Frame::MonitorName`). The pid
/// is not known here, so the pending entry and an unreachable `:noconnection` name the
/// placeholder pid `u64::MAX` — the one a local `monitor` of an unregistered name fires
/// `:noproc` for. A resolved monitor's DOWN carries the real pid.
pub(crate) fn monitor_remote_name(target_node: Symbol, name: Symbol, watcher_pid: u64, mref: u64) {
    process::record_pending_remote(target_node, u64::MAX, watcher_pid, mref);
    let sent = send_frame(
        target_node,
        Frame::MonitorName {
            watcher_pid,
            name,
            mref,
        },
    );
    if !sent {
        process::fire_noconnection_if_pending(target_node, u64::MAX, watcher_pid, mref);
    }
}

/// `(demonitor mref)` for a monitor that was set up against a remote pid:
/// ship a `Frame::Demonitor` and forget the pending entry locally. Best
/// effort, like the local demonitor — the peer drops the matching watcher
/// from its `MONITORS` table.
pub(crate) fn demonitor_remote(target_node: Symbol, watcher_pid: u64, mref: u64) {
    process::drop_pending_remote(target_node, watcher_pid, mref);
    let _ = send_frame(target_node, Frame::Demonitor { watcher_pid, mref });
}

/// A monitor a peer registered on one of our local pids (`Frame::Monitor`) just
/// fired: ship the DOWN as a dedicated `Frame::Down`, not an ordinary `Send`, so
/// the watcher's node can retire its `PENDING_REMOTE` entry the moment the
/// one-shot delivers — as a plain message that entry outlived its own DOWN and a
/// later node-down fired a second `[:down mref … :noconnection]` (KI-96).
/// Best-effort: a DOWN to a disconnected watcher has nowhere to go (its own
/// `[:nodedown]`/`:noconnection` path already covers it).
pub(crate) fn send_down(
    target_node: Symbol,
    watcher_pid: u64,
    mref: u64,
    dying_pid: u64,
    reason: Message,
) {
    let _ = send_frame(
        target_node,
        Frame::Down {
            watcher_pid,
            mref,
            target_pid: dying_pid,
            reason,
        },
    );
}

// ---- cross-node links (ADR-067) — the symmetric cousin of monitor_remote ----

/// `(link remote-pid)`: record our half of the link, ship a `Frame::Link` so the
/// peer records its half, and — if the link to that node isn't up — fire an
/// immediate `:noconnection` to the local linker (same shape a monitor's
/// unreachable target gets). `local_pid` is the linker (self). Race-free against
/// net-split exactly as `monitor_remote`: record before sending.
pub(crate) fn link_remote(target_node: Symbol, target_pid: u64, local_pid: u64) {
    // `local_pid` is the calling process, so this is true by construction; binding it
    // keeps the `#[must_use]`-ish intent visible rather than discarding a meaningful bool.
    let _linked = process::record_remote_link(local_pid, target_node, target_pid);
    #[cfg(test)]
    tests::run_setup_gap_hook();
    let sent = send_frame(
        target_node,
        Frame::Link {
            from_pid: local_pid,
            to_pid: target_pid,
        },
    );
    if !sent {
        // No link to that node: the target is unreachable. Fire `:noconnection`
        // to the linker (this also drops the half-entry we just recorded) — unless a
        // link that dropped between the record and the send already did, in which
        // case the entry is gone and this fires nothing (no duplicate exit).
        process::deliver_remote_link_exit(
            local_pid,
            target_node,
            target_pid,
            Message::Keyword(value::intern(pk::NOCONNECTION)),
        );
    }
}

/// `(unlink remote-pid)`: drop our half and ship a best-effort `Frame::Unlink`.
pub(crate) fn unlink_remote(target_node: Symbol, target_pid: u64, local_pid: u64) {
    process::drop_remote_link(local_pid, target_node, target_pid);
    let _ = send_frame(
        target_node,
        Frame::Unlink {
            from_pid: local_pid,
            to_pid: target_pid,
        },
    );
}

/// A local linked process `from_pid` died with `reason`: ship a link
/// `Frame::Exit` to its remote peer `target_pid` on `target_node`. Best-effort —
/// if the link is down the peer already learns via its own net-split handling.
/// Called from `links::notify_peers`.
pub(crate) fn send_link_exit(target_node: Symbol, target_pid: u64, from_pid: u64, reason: Message) {
    let _ = send_frame(
        target_node,
        Frame::Exit {
            from_pid,
            to_pid: target_pid,
            reason,
            link: true,
        },
    );
}

/// `(exit remote-pid reason)`: ship a non-link `Frame::Exit` routed straight to
/// the peer's `scheduler::exit` (kill-style, like the local builtin). Used for an
/// explicit remote exit and for a supervisor terminating a remote child.
pub(crate) fn exit_remote(target_node: Symbol, target_pid: u64, reason: Message) {
    let _ = send_frame(
        target_node,
        Frame::Exit {
            from_pid: 0, // unused for an explicit (non-link) exit
            to_pid: target_pid,
            reason,
            link: false,
        },
    );
}

/// Resolve a bare node name (no `@`) to the qualified form by looking in NODES.
///
/// `(monitor-node :a)` passes symbol `a`; NODES is keyed by `a@127.0.0.1`.
/// Without this step the liveness check `!NODES.contains_key(&name)` always
/// returns true for bare names, firing an immediate `[:nodedown]` even while the
/// peer is alive — and `fire_nodedown` never finds the watcher on a real down.
///
/// Returns the name unchanged if it already contains `@`, or if no connected
/// peer with the given base name exists (peer is already down; the bare name
/// is used for the immediate-delivery path in `monitor_node`).
fn qualify_node_name(name: Symbol) -> Symbol {
    let s = value::symbol_name(name);
    if s.contains('@') {
        return name;
    }
    let prefix = format!("{s}@");
    crate::core::sync::read(&NODES)
        .keys()
        .find(|&&k| value::symbol_name(k).starts_with(&prefix))
        .copied()
        .unwrap_or(name)
}

/// `(monitor-node name pid)` — deliver `[:nodedown name]` to `pid` when a link to
/// `name` goes down. Persistent (fires on each down) until the process exits.
/// If `name` isn't us and there's no current link, the node is effectively
/// already down and `[:nodedown]` is delivered immediately.
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    let name = qualify_node_name(name);
    // Registration and the liveness check must be atomic w.r.t. `fire_nodedown`
    // (which reads NODE_MONITORS then delivers). Holding the write lock across
    // both prevents the race where fire_nodedown sees a new watcher AND our
    // own fallback also fires.
    //
    // Always register — monitor_node is persistent: fires on
    // every future down event until the process exits. Dedup so a pid calling
    // (monitor-node name) again after a reconnect doesn't double-fire per down.
    //
    // Lock order: NODE_MONITORS write → NODES read. Safe: drop_link releases
    // NODES write *before* calling fire_nodedown, so no thread holds NODES write
    // while waiting for NODE_MONITORS.
    let immediate = {
        let mut monitors = crate::core::sync::write(&NODE_MONITORS);
        let watchers = monitors.entry(name).or_default();
        if !watchers.contains(&pid) {
            watchers.push(pid);
        }
        // If the peer is already down, deliver immediately as well as register.
        // A tiny residual race: if fire_nodedown was blocked on our write lock
        // (peer died in this same instant), it will also deliver to our new
        // watcher once we release → two [:nodedown] messages possible in that
        // sub-microsecond window. Receivers must tolerate duplicate nodedowns.
        !is_local(name) && !crate::core::sync::read(&NODES).contains_key(&name)
    };
    if immediate {
        process::deliver(pid, nodedown_msg(name));
    }
}

/// Cancel `pid`'s node monitor for `name`. A no-op if no monitor is registered.
/// Needed when a live process wants to stop watching a node before it exits;
/// [`unregister_dead_pid_while`] handles the death case automatically.
///
/// **Residual race**: `fire_nodedown` snapshots the watcher list under a *read*
/// lock and then delivers outside any lock. If a `fire_nodedown` for `name` is
/// already past the snapshot step when `demonitor_node` removes `pid`, one
/// spurious `[:nodedown name]` will still arrive. Callers must tolerate it
/// (the same tolerance required for the `monitor_node` registration race).
pub(crate) fn demonitor_node(name: Symbol, pid: u64) {
    let name = qualify_node_name(name);
    let mut monitors = crate::core::sync::write(&NODE_MONITORS);
    if let Some(watchers) = monitors.get_mut(&name) {
        watchers.retain(|&w| w != pid);
        if watchers.is_empty() {
            monitors.remove(&name);
        }
    }
}

/// The `[:nodedown <name>]` message a downed link delivers to its watchers.
fn nodedown_msg(name: Symbol) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern(pk::NODEDOWN)),
        Message::Keyword(name),
    ])
}

/// The guest standing of the link now registered for `peer` — its grant (ADR-415) and the
/// name it claimed (ADR-425) — or `None` for a full link or no link, for
/// `(node/peer-grant node)`. Asked of the registered
/// link, so it answers for the connection that is up, never one that has gone.
pub(crate) fn peer_grant(peer: Symbol) -> Option<Arc<guest::GuestLink>> {
    crate::core::sync::read(&NODES)
        .get(&peer)
        .and_then(|conn| conn.guest.clone())
}

/// Connected peer node names (for `(nodes)`).
pub(crate) fn connected_nodes() -> Vec<Symbol> {
    crate::core::sync::read(&NODES).keys().copied().collect()
}

/// `(disconnect name)` — tear the link to peer `name` down now, *without* exiting
/// this process: remove the `NODES` entry, shut the socket down (so the peer's reader
/// hits EOF and fires its own node-down), and fire `[:nodedown name]` to our monitors —
/// the teardown the reader takes on a clean peer exit, triggered deliberately. Returns
/// `true` if a link existed, `false` if there was nothing connected under `name`.
///
/// The entry goes BEFORE the shutdown, so `[:nodedown]` has been delivered when this
/// returns. Shutting down first woke our own reader, which could remove the entry and be
/// about to deliver when this side's removal found nothing and returned — a caller that
/// looked at its mailbox next saw no `[:nodedown]` yet. The reader's `drop_link` on EOF
/// now finds the entry gone and is a no-op, so it still fires exactly once.
pub(crate) fn disconnect(peer: Symbol) -> bool {
    let conn = crate::core::sync::read(&NODES)
        .get(&peer)
        .map(|c| (Arc::clone(&c.sock), c.id));
    match conn {
        Some((sock, id)) => {
            let removed = remove_current_link(peer, id);
            let _ = sock.shutdown(Shutdown::Both);
            if removed {
                forget_dial_routes(peer);
                fire_nodedown(peer);
            }
            true
        }
        None => false,
    }
}

// ----- routing ---------------------------------------------------------------

/// How a `send` names its target within a node.
pub(crate) enum Target {
    /// A concrete process id (a pid's local part).
    Pid(u64),
    /// A registered name resolved on the destination node.
    Name(Symbol),
}
/// Warn, once per name, that a message addressed to a **registered name no process holds**
/// was dropped.
///
/// The drop itself is correct and stays correct, and legitimate — the name
/// may simply be gone, or never have existed. What this fixes is the **silence**. `send` is
/// fire-and-forget, so the sender has already moved on and cannot be told; the receiving node
/// is the only party that knows the message died. A silent drop otherwise surfaces only as a
/// deadline expiring with no information anywhere (KI-36).
///
/// **Default-on, deliberately.** A flag-gated trace is worth nothing for this class, because
/// nobody arms a flag before the bug they have not diagnosed yet (KI-39). Silence it with
/// `BROOD_NO_DROP_WARN=1`.
///
/// **Deduplicated per name**, so a hot loop addressing a dead service warns once rather than
/// flooding: the set is bounded by the number of distinct names, not by traffic.
fn warn_dropped_to_unregistered_name(name: Symbol, whence: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static QUIET: AtomicBool = AtomicBool::new(false);
    static QUIET_INIT: std::sync::Once = std::sync::Once::new();
    QUIET_INIT.call_once(|| {
        QUIET.store(
            std::env::var_os("BROOD_NO_DROP_WARN").is_some(),
            Ordering::Relaxed,
        );
    });
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    /// Ceiling on the dedup set. The set exists so a hot loop addressing a dead service
    /// warns once rather than flooding, and its size is "the number of distinct names" —
    /// which for an INBOUND drop is chosen by the peer, not by us. Past the cap we stop
    /// growing and simply warn again: a flood of distinct names is itself worth seeing,
    /// and the alternative is a remote-controlled set that never shrinks. KI-97 item 4.
    const MAX_WARNED_NAMES: usize = 4096;
    static SEEN: std::sync::Mutex<Option<std::collections::HashSet<Symbol>>> =
        std::sync::Mutex::new(None);
    let first = match SEEN.lock() {
        Ok(mut g) => {
            let seen = g.get_or_insert_with(Default::default);
            if seen.len() >= MAX_WARNED_NAMES && !seen.contains(&name) {
                true // at the cap: warn without recording, rather than grow
            } else {
                seen.insert(name)
            }
        }
        // A poisoned lock must not silence the diagnostic this exists to print.
        Err(_) => true,
    };
    if first {
        eprintln!(
            "dist: dropped {whence} message for unregistered name :{} \
             (no process holds this name; warns once per name, BROOD_NO_DROP_WARN=1 to silence)",
            value::symbol_name(name)
        );
    }
}

/// Deliver `msg` to `target` on `node`, location-transparently: a local node
/// delivers in-process; a remote one forwards over the link. Unknown name,
/// unknown/disconnected node, or a dead pid is a silent no-op.
///
/// Returns whether a route existed at all: `true` for any local target (even a
/// dead pid — process liveness stays silent) and for a remote node with
/// a live link; `false` when the node is **unknown/disconnected** (the message
/// was dropped on the floor). `send` surfaces that as a catchable
/// `:noconnection` error when the sending process opted in via
/// `(proc/flag :send-errors true)` — so a caller can queue-and-retry instead
/// of silently losing messages until the reconnect (the dist self-healing
/// seam); every other caller ignores it.
///
/// The message is queued in `lane` wherever it lands (ADR-492): a remote node gets the lane
/// in the `Send` frame, so `proc/send-priority` and `proc/send-latest` mean the same thing
/// to a pid on another node as to one here.
pub(crate) fn route_in_lane(
    node: Symbol,
    target: Target,
    msg: Message,
    lane: process::Lane,
) -> bool {
    if is_local(node) {
        let id = match target {
            Target::Pid(id) => id,
            Target::Name(name) => match crate::core::sync::read(&NAMES).get(name) {
                Some(id) => id,
                None => {
                    warn_dropped_to_unregistered_name(name, "local");
                    // Still `true`: a route existed, the *name* did not. Returning false here
                    // would surface this as `:noconnection` to a `send-errors` sender, which
                    // is a different and wrong diagnosis — the node is fine.
                    return true;
                }
            },
        };
        process::deliver_in_lane(id, msg, lane);
        return true;
    }
    // Remote: encode a Send frame and hand it to the peer's writer thread. Snapshot the
    // writer handles (and a guest link's standing), then release NODES before enqueueing —
    // see [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let link = crate::core::sync::read(&NODES).get(&node).map(|conn| {
        (
            conn.writer_handles(),
            conn.guest.clone(),
            conn.lanes,
            Arc::clone(&conn.latest),
        )
    });
    let Some(((tx, sock), guest, lanes, latest)) = link else {
        return false;
    };
    // A `send-latest` replaces its slot's unwritten frame on the link whatever the peer
    // speaks; the lane itself goes on the wire only to a v11 peer, and to a v10 one the
    // message is the plain `Send` it knows (ADR-492).
    let slot = match &lane {
        process::Lane::Latest(key) => Some(latest_slot(&target, key)),
        _ => None,
    };
    let lane = if lanes { lane } else { process::Lane::Normal };
    // A guest link is told about every local pid in the message, which is what lets it
    // reply to one (ADR-415) — noted on the link the message goes out on, before it is
    // queued there.
    if let Some(guest) = &guest {
        guest.note_outbound(&msg);
    }
    // And its own pids go back in the guest's own spelling (ADR-425).
    let mut frame = Frame::Send { target, msg, lane };
    if let Some(guest) = &guest {
        guest.translate_outbound(&mut frame);
    }
    let bytes: Arc<Vec<u8>> = match encode_payload(&frame) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode message for {}: {}",
                value::symbol_name(node),
                e
            );
            return true; // a link exists; the payload was the problem
        }
    };
    // Either severs the link if the writer is gone or stalled.
    let _ = match slot {
        Some(slot) => enqueue_latest(&tx, &sock, &latest, slot, bytes),
        None => enqueue_to(&tx, &sock, bytes),
    };
    true
}

/// [`route_in_lane`] on the normal lane: the tests' plain send.
#[cfg(test)]
pub(crate) fn route(node: Symbol, target: Target, msg: Message) -> bool {
    route_in_lane(node, target, msg, process::Lane::Normal)
}

// ----- connection lifecycle --------------------------------------------------

/// `(%node-listen name addr cookie)` — set this runtime's identity (name +
/// cookie) and listen for peers. `addr` carries the transport: `"unix:PATH"`
/// (local, addressed by name) or `"tcp:HOST:PORT"` (remote). Each accepted
/// connection is authenticated (cookie) and, on success, gets reader + writer
/// threads. Errors if this runtime is already a node — a second listener would
/// leak the first. The *policy* (socket path, cookie source, transport choice)
/// lives in `std/prelude.blsp`; this primitive is the mechanism (ADR-068).
pub(crate) fn node_listen(name: Symbol, addr: &str, cookie: String) -> io::Result<()> {
    // Guardrail: the cookie is the *entire* trust
    // boundary — a holder has remote code execution by design — so refuse one
    // short enough to guess or brute-force online. The default policy
    // (`node/cookie`) generates `(rand/token 32)`; this only
    // rejects a deliberately weak override (e.g. a short `$BROOD_COOKIE`).
    if cookie.len() < MIN_COOKIE_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "node cookie too short: {} bytes (minimum {MIN_COOKIE_LEN}) — \
                 a cookie-holder has full remote eval on this node; use the \
                 default (node/cookie) or e.g. (rand/token 32)",
                cookie.len()
            ),
        ));
    }
    // The guest namespace is the host's to mint in (ADR-425): every peer refuses a full
    // link under such a name, so a node started with one could link to nothing.
    if guest::is_guest_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "node name {} is reserved: names starting {:?} are what a host calls its \
                 guest links",
                value::symbol_name(name),
                guest::NAME_PREFIX
            ),
        ));
    }
    // Guard against a second node-start and publish identity atomically under
    // the same write lock — closing the TOCTOU window a separate read-check
    // + set_identity would leave. The acceptor reads identity lazily (at
    // accept time), so the write happens before any peer is served; if the
    // bind fails below, clear_identity rolls it back so node-start can retry.
    {
        let mut n = crate::core::sync::write(&NODE);
        if n.started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this runtime is already a node (node-start called twice)",
            ));
        }
        n.name = name;
        n.cookie = cookie;
        n.started = true;
    }
    LOCAL_NODE.store(name, Ordering::Release);
    if let Err(e) = start_listener(addr) {
        clear_identity();
        return Err(e);
    }
    Ok(())
}

/// `(%node-also-listen addr)` — add another listener (`unix:PATH` / `tcp:HOST:PORT`)
/// to an already-started node, so one node serves several transports at once
/// (ADR-074): a local Unix socket *and* a remote TCP endpoint — the editor-daemon
/// "reachable locally by name and remotely over the network" shape. Shares the
/// node's existing identity + cookie; errors if this runtime isn't a node yet.
pub(crate) fn node_also_listen(addr: &str) -> io::Result<()> {
    {
        let n = crate::core::sync::read(&NODE);
        if !n.started {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "node-also-listen: this runtime is not a node yet (call node-start first)",
            ));
        }
    }
    start_listener(addr).map(|_| ())
}

/// Bind one listener for `addr` and spawn its accept loop, returning the address it
/// records for peers to dial. Identity-agnostic — the per-connection handshake reads
/// `NODE` at accept time — so it serves both the first listener (`node_listen`) and any
/// added later (`node_also_listen`).
fn start_listener(addr: &str) -> io::Result<String> {
    // What peers are told to dial: the BOUND address, never the requested one — a
    // requested port 0 would otherwise be advertised as port 0.
    let advertised = if let Some(path) = addr.strip_prefix("unix:") {
        bind_unix_listener(path)?;
        addr.to_string()
    } else if let Some(host_and_port) = addr.strip_prefix("tcp:") {
        let listener = TcpListener::bind(host_and_port)?;
        let bound = listener.local_addr()?;
        spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Tcp(s)));
        // A wildcard host (`0.0.0.0`, `::`) listens everywhere but is not an address
        // anyone can dial from another machine: advertise the node's own host instead.
        if bound.ip().is_unspecified() {
            format!("tcp:{}:{}", node_host(), bound.port())
        } else {
            format!("tcp:{bound}")
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ));
    };
    // Record only after a successful bind, so a failed listener doesn't leave a
    // dead address we'd advertise to peers.
    crate::core::sync::write(&LISTEN_ADDRS).push(advertised.clone());
    Ok(advertised)
}

/// The host this node is reachable at, for advertising a wildcard listener: the host
/// half of its qualified `name@host` identity (ADR-073), else `localhost`.
fn node_host() -> String {
    let name = crate::core::sync::read(&NODE).name;
    value::symbol_name(name)
        .split_once('@')
        .map(|(_, host)| host.to_string())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Roll back a failed `node-start` — used when the first listener's bind fails
/// so the runtime stays a non-node (retryable) rather than a node with no listener.
fn clear_identity() {
    {
        let mut n = crate::core::sync::write(&NODE);
        n.name = *NONODE;
        n.cookie = String::new();
        n.started = false;
    }
    LOCAL_NODE.store(u32::MAX, Ordering::Release);
}

/// A `Read`/`Write` shim that fails once a wall-clock deadline passes — the
/// whole-handshake bound the socket's `SO_RCVTIMEO` cannot express.
///
/// The distinction is the entire point: a socket read timeout restarts on every
/// byte, so a peer sending one byte just inside it stays alive indefinitely
/// while holding a [`HandshakeSlot`]. This checks an *absolute* instant that no
/// amount of progress moves, so a trickling peer is cut off on schedule.
///
/// Writes are covered too, not just reads: a peer that completes its side and
/// then stops reading would otherwise park us in `write_all` against a full
/// socket buffer with no timeout at all — the same slot held, by the other
/// direction.
///
/// The deadline is checked *around* each call rather than by shortening the
/// socket timeout, so a single blocking read can still overshoot by at most one
/// [`HANDSHAKE_TIMEOUT`]; the bound is the sum, which is what matters against an
/// unbounded hold.
struct Deadline<'a, S> {
    inner: &'a mut S,
    until: Instant,
}

impl<'a, S> Deadline<'a, S> {
    fn new(inner: &'a mut S, budget: Duration) -> Self {
        Self {
            inner,
            until: Instant::now() + budget,
        }
    }

    fn check(&self) -> io::Result<()> {
        if Instant::now() >= self.until {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "handshake exceeded its deadline",
            ));
        }
        Ok(())
    }
}

impl<S: Read> Read for Deadline<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.read(buf)?;
        // Also *after* the call: a read that dribbled back a byte just under the
        // socket timeout must not be allowed to start another one past the
        // deadline. `read_exact` loops on short reads, and this is the loop's
        // brake.
        self.check()?;
        Ok(n)
    }
}

impl<S: Write> Write for Deadline<'_, S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check()?;
        let n = self.inner.write(buf)?;
        self.check()?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}

/// How many inbound connections have been shed at the [`MAX_IN_FLIGHT_HANDSHAKES`]
/// cap since start.
static SHED_HANDSHAKES: AtomicU64 = AtomicU64::new(0);

/// Millisecond timestamp of the last shed warning, for rate limiting.
///
/// `u64::MAX` — not 0 — is the "never warned yet" sentinel: `now_millis()` counts
/// from process start, so 0 is a perfectly real timestamp during the first
/// millisecond, and a 0 sentinel would silently swallow the first warning of a
/// flood that began at startup.
const SHED_WARN_NEVER: u64 = u64::MAX;
static LAST_SHED_WARN_MS: AtomicU64 = AtomicU64::new(SHED_WARN_NEVER);

/// Minimum gap between shed warnings. Long enough that a sustained flood costs
/// one line a minute, short enough that an operator watching a log sees it.
const SHED_WARN_INTERVAL_MS: u64 = 60_000;

/// Count a shed inbound connection and, at most once per
/// [`SHED_WARN_INTERVAL_MS`], say so.
///
/// The shed itself is correct and stays correct — past the cap, closing the
/// socket is the whole response. It must not be **silent**: hitting the cap means
/// *every further inbound link is refused*, i.e. inbound distribution is
/// effectively down (the KI-36 lesson about silent drops).
///
/// Rate-limited rather than per-shed, because the trigger is by definition a
/// flood and a line each would be its own amplification vector — the same shape
/// as the ADR-232 per-name dedup. The cumulative count rides along, so one line
/// still conveys the scale.
fn note_shed_handshake() {
    let total = SHED_HANDSHAKES.fetch_add(1, Ordering::Relaxed) + 1;
    let now = now_millis();
    let last = LAST_SHED_WARN_MS.load(Ordering::Relaxed);
    // The first shed ever always warns; afterwards, once per interval.
    if last != SHED_WARN_NEVER && now.saturating_sub(last) < SHED_WARN_INTERVAL_MS {
        return;
    }
    // Only the thread that wins the stamp prints, so a burst yields one line.
    if LAST_SHED_WARN_MS
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    eprintln!(
        "dist: inbound connection shed — {MAX_IN_FLIGHT_HANDSHAKES} handshakes already in \
         flight ({total} shed so far). Inbound links are being refused; if this is not a \
         flood, a peer may be connecting without completing its handshake."
    );
}

/// RAII permit for one in-flight handshake slot (see [`MAX_IN_FLIGHT_HANDSHAKES`]).
/// Held by the per-connection thread for the whole pre-auth window; released on
/// drop (thread end), whether the handshake succeeded, failed, or timed out.
struct HandshakeSlot {
    count: &'static AtomicUsize,
}
impl HandshakeSlot {
    /// Take a slot, or `None` if the cap is already reached (caller sheds the
    /// connection). The over-count from a losing `fetch_add` is immediately
    /// rolled back, so the gate can't drift above the cap under contention.
    fn try_acquire() -> Option<Self> {
        Self::try_acquire_on(&IN_FLIGHT_HANDSHAKES)
    }

    /// [`try_acquire`](Self::try_acquire) against `count` — the process's counter in
    /// production, a test's own one in the test (sibling test threads hold real slots).
    fn try_acquire_on(count: &'static AtomicUsize) -> Option<Self> {
        if count.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT_HANDSHAKES {
            count.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(HandshakeSlot { count })
        }
    }
}
impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Spawn a named background thread, reporting a refusal instead of panicking on it.
///
/// `std::thread::spawn` **panics** when the OS refuses a thread (EAGAIN under thread/fd
/// pressure), and dist spawns threads at attacker-influenced rates: one per accepted
/// connection, one per gossiped peer (up to 4096 in a single `Peers` frame). A panic in
/// the accept loop unwinds the acceptor itself and the listener is gone for good — the
/// node stops accepting inbound links until it restarts, from one transient EAGAIN.
/// Returns whether the thread started, so each caller can shed that unit of work rather
/// than lose the loop around it. KI-97 item 3.
fn spawn_bg<F>(name: &str, f: F) -> bool
where
    F: FnOnce() + Send + 'static,
{
    match std::thread::Builder::new().name(name.to_string()).spawn(f) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("dist: cannot spawn {name} thread ({e}); shedding this work");
            false
        }
    }
}

/// The accept loop, shared by both transports: pull the next link off `accept`
/// and hand each to a panic-isolated per-connection thread. A transient accept
/// error (EMFILE etc.) logs and re-loops with a tiny backoff rather than
/// burn-looping or killing the acceptor.
fn spawn_acceptor(accept: impl FnMut() -> io::Result<Stream> + Send + 'static) {
    let mut accept = accept;
    spawn_bg("dist-acceptor", move || {
        loop {
            match accept() {
                Ok(stream) => {
                    // Shed past the in-flight-handshake cap *before* spawning a thread
                    // or reading a byte, so a flood of unauthenticated connections
                    // can't exhaust threads/memory. Closing the socket is the whole
                    // response, but not a *silent* one: reaching the cap means inbound
                    // links are being refused outright, which the node reports
                    // (rate-limited — see `note_shed_handshake`).
                    let permit = match HandshakeSlot::try_acquire() {
                        Some(p) => p,
                        None => {
                            note_shed_handshake();
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }
                    };
                    // `spawn_bg`, not `thread::spawn`: a refused thread must not panic
                    // *inside the accept loop* (that would close the listener permanently);
                    // the connection is refused and the node keeps accepting.
                    spawn_bg("dist-conn", move || {
                        // Hold the slot until the handshake finishes (this thread ends
                        // right after `establish` hands off to the steady-state reader
                        // and writer threads, which don't hold a slot).
                        let _permit = permit;
                        // Catch a panic in the per-connection thread so one bad peer
                        // doesn't take down the runtime via thread-panic unwind (the
                        // rest of the dist surface assumes its background threads
                        // stay alive).
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Err(e) = accept_link(stream) {
                                eprintln!("dist: incoming connection failed: {}", e);
                            }
                        }));
                    });
                    // No explicit shed on failure is needed: `Builder::spawn` drops the
                    // closure when it cannot start, which drops the accepted `stream` (closing
                    // the socket) and the `HandshakeSlot` permit with it. The connection is
                    // refused and the acceptor loops on, which is the whole point.
                }
                Err(e) => {
                    eprintln!("dist: accept error: {}", e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    });
}

/// Bind a Unix-domain listener and spawn its accept loop (the `unix:` transport).
/// wasm and other non-unix targets have no Unix sockets, so it reports unsupported.
#[cfg(unix)]
fn bind_unix_listener(path: &str) -> io::Result<()> {
    let path = path.to_string();
    prepare_unix_path(&path)?;
    let listener = UnixListener::bind(&path)?;
    spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Unix(s)));
    Ok(())
}
#[cfg(not(unix))]
fn bind_unix_listener(_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unix-socket nodes are not available on this target",
    ))
}

/// Dial a Unix-domain peer (the `unix:` transport); unsupported off unix.
#[cfg(unix)]
fn dial_unix(path: &str) -> io::Result<Stream> {
    Ok(Stream::Unix(UnixStream::connect(path)?))
}
#[cfg(not(unix))]
fn dial_unix(_path: &str) -> io::Result<Stream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unix-socket nodes are not available on this target",
    ))
}

/// Ready a Unix-socket path for `bind`: create the parent directory (`0700`) and
/// clear a **stale** socket left by a crashed node. A path that still has a live
/// listener is refused (another node owns that name); a path that refuses a
/// connection is stale and gets unlinked so we can rebind. Best-effort against a
/// concurrent same-name start — a same-user dev footgun, not a security boundary
/// (the `0700` dir already gates other users).
#[cfg(unix)]
fn prepare_unix_path(path: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    if p.exists() {
        match UnixStream::connect(p) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("node socket {path} is already in use by a live node"),
                ));
            }
            // ConnectionRefused → no listener; socket file is stale from a
            // crashed node. Unlink it so `bind` can recreate it.
            Err(ref e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(p);
            }
            // Any other connect error (EACCES, ENOENT after a race, …) means
            // we can't determine liveness. Leave the file alone and let `bind`
            // fail with a clear OS error rather than destroying a potentially
            // live peer's socket.
            Err(_) => {}
        }
    }
    Ok(())
}

// `Role` + the four-step `handshake` live in `dist::handshake`; only the link
// lifecycle uses them, and they keep the cookie/nonce/MAC plumbing self-
// contained.
use handshake::{Offer, Role, handshake};

/// The protocol version this node offers in a handshake (ADR-492): v11, or exactly v10
/// under `BROOD_DIST_V10` — a runtime from before lanes, which a mixed-cluster test
/// needs and cannot otherwise build.
fn local_offer() -> Offer {
    static OFFER: LazyLock<Offer> = LazyLock::new(|| {
        if std::env::var_os("BROOD_DIST_V10").is_some() {
            Offer::V10
        } else {
            Offer::Current
        }
    });
    *OFFER
}

/// Dial `addr` and drive the client handshake, offering `offer`.
fn dial_and_greet(addr: &str, offer: Offer) -> io::Result<(Stream, handshake::Authenticated)> {
    let mut stream = dial(addr)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // Deadlined like the accept side: a malicious or wedged *listener* can
    // trickle at a dialer just as easily, and this call is made from a scheduler
    // worker, so an unbounded hold here wedges a worker rather than a slot.
    let authenticated = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Initiator, offer)?
    };
    Ok((stream, authenticated))
}

/// `(%node-connect peer addr)` — dial a peer and complete the client handshake.
/// `addr` carries the transport (`"unix:PATH"` / `"tcp:HOST:PORT"`); `peer` is
/// the name we expect (used for the self-dial guard + de-dup, before the
/// handshake reveals the peer's authoritative name). Uses this runtime's
/// already-published identity (the prelude `connect` requires a prior
/// `node-start`). Returns the peer's authoritative node name on success.
pub(crate) fn node_connect(peer: Symbol, addr: &str) -> io::Result<Symbol> {
    // Refuse to dial ourselves — it would race through the handshake and form a
    // tie-break loser in the same process; cleaner to reject up front.
    if peer == local_node() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot connect to self ({})", value::symbol_name(peer)),
        ));
    }
    // Pre-dial de-dup: if we already have a link to this node — under the claimed
    // name, or reached before through this very address under whatever name the
    // handshake gave it — reuse it without dialing. A second dial to a live peer is
    // not harmless: it costs a handshake and leaves `establish` a same-incarnation
    // duplicate to resolve. The claim may still be stale/wrong (e.g. gossip lag, a
    // never-dialed spelling), so `establish` re-checks under the authenticated name.
    if let Some(linked) = linked_peer_for(peer, addr) {
        return Ok(linked);
    }
    // A v10 node answers a v11 offer with its own magic and closes: dial it again in v10,
    // so a node from before lanes still links (ADR-492).
    let (stream, authenticated) = match dial_and_greet(addr, local_offer()) {
        Err(error) if handshake::is_v10_peer(&error) => dial_and_greet(addr, Offer::V10)?,
        greeted => greeted?,
    };
    let peer = authenticated.peer;
    // The claim can name another node while the address reaches us (a second
    // spelling of our own name, a relay looping back): the accept side refuses a
    // peer claiming to be us, and so must the dial side, or `NODES` gains an entry
    // for ourselves.
    if peer == local_node() {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "cannot connect to self ({addr} is this node, {})",
                value::symbol_name(peer)
            ),
        ));
    }
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    // Always pass to `establish` — even when we already have a link under the
    // authenticated name. `establish` has its own symmetric tie-break (both sides
    // compare connectors by name and reach the same decision). The losing side
    // closes its own socket and returns; the winning side replaces the link. A
    // short-circuit `stream.shutdown` here would skip the tie-break on our end
    // while the peer still runs `establish` on theirs — they might win, register
    // our doomed socket, and later fire a spurious `[:nodedown]` when the reader
    // hits the EOF our shutdown sent.
    establish(authenticated, stream, Role::Initiator);
    // Whether our link won or an existing one did, `addr` reaches `peer`.
    crate::core::sync::write(&DIAL_ROUTES).insert(addr.to_string(), peer);
    Ok(peer)
}

/// The connected node a dial of `addr` claiming `claimed` would reach, if we can
/// tell without dialing: `claimed` itself when linked; else the node an earlier dial
/// of the same address authenticated as (`DIAL_ROUTES`); else the node that
/// advertises `addr` as its dial address. Each answer is trusted only while that
/// node is still in `NODES`.
///
/// Only a FULL link answers (KI-287). A guest's advertised address is its own claim, so
/// a guest link is never the link a dial was looking for, even when it advertises the
/// dialed address: the dial goes ahead. (Its name cannot match a dial's claim — it is
/// one this node minted, ADR-425.)
fn linked_peer_for(claimed: Symbol, addr: &str) -> Option<Symbol> {
    // Lock order: NODES, then DIAL_ROUTES — nothing takes them the other way round.
    let nodes = crate::core::sync::read(&NODES);
    let full = |name: &Symbol| nodes.get(name).is_some_and(|link| link.guest.is_none());
    if full(&claimed) {
        return Some(claimed);
    }
    if let Some(&routed) = crate::core::sync::read(&DIAL_ROUTES).get(addr)
        && full(&routed)
    {
        return Some(routed);
    }
    nodes
        .iter()
        .find(|(_, link)| link.guest.is_none() && !link.addr.is_empty() && link.addr == addr)
        .map(|(&name, _)| name)
}

/// Drop every `DIAL_ROUTES` entry naming `peer` — its link is gone, so an address
/// that reached it no longer proves anything about what answers there now.
fn forget_dial_routes(peer: Symbol) {
    crate::core::sync::write(&DIAL_ROUTES).retain(|_, routed| *routed != peer);
}

/// Open the carrier for `addr`. Unix connects are local and effectively instant
/// (or refuse immediately); TCP goes through [`dial_tcp`](crate::host::dial::dial_tcp),
/// which bounds **both** the name resolution and each connect — for `node/connect` the
/// dialling thread is a scheduler worker, which cannot be preempted mid-syscall, so
/// neither a wedged resolver nor a silently-dropping peer may pin it.
fn dial(addr: &str) -> io::Result<Stream> {
    if let Some(path) = addr.strip_prefix("unix:") {
        dial_unix(path)
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        let (host, port) = crate::host::dial::split_host_port(hostport)?;
        crate::host::dial::dial_tcp(host, port, CONNECT_TIMEOUT).map(Stream::Tcp)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ))
    }
}

/// Server side of the handshake: drive the v2 exchange, then start the link
/// threads. See [`handshake`] for the protocol.
fn accept_link(mut stream: Stream) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // The per-read timeout above bounds a peer that goes *silent*; the deadline
    // bounds one that stays slow. Only the second bounds how long an
    // unauthenticated peer can hold its `HandshakeSlot` (KI-97 item 1).
    let authenticated = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Responder, local_offer())?
    };
    let peer = authenticated.peer;
    // Refuse a peer claiming to BE us — the accept-side counterpart of the check
    // `node_connect` already makes on the dial side. Reachable by a relay that
    // cross-wires two connections back to our own listener, or by a misconfigured
    // second node reusing our name. `establish` would otherwise register a
    // `NODES[our-own-name]` entry: mostly inert (`is_local()` still short-circuits
    // sends to our own name) but it produces heartbeat traffic, corrupts `(nodes)`
    // output, and emits a spurious `[:nodedown our-name]` on teardown.
    if peer == local_node() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "peer claims our own node name ({})",
                value::symbol_name(peer)
            ),
        ));
    }
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(authenticated, stream, Role::Responder);
    Ok(())
}

/// Register the authenticated link and spawn its reader + writer threads —
/// resolving a duplicate against any existing link to the same peer first.
/// `authenticated.addr` is how a third node should dial this peer (for mesh gossip).
///
/// A duplicate is one of two different things, and the peer's incarnation tells
/// them apart:
///
/// - **The peer restarted** (incarnations differ). The registered link reaches a
///   process that no longer exists — its host vanished without a FIN, so nothing has
///   torn it down yet, and the heartbeat would only notice after `DOWN_AFTER`. The
///   new link always wins: the old one is retired as a node-down of that old life
///   (`retire_previous_incarnation` — `[:nodedown]` to node monitors, `:noconnection`
///   to every monitor and link made over it), then this link registers as a
///   brand-new peer.
/// - **A second link to the same live peer** (same incarnation: two dials crossing,
///   or two dials from one side racing). Both reach the live peer, so tearing either
///   down must not reach a monitor: the symmetric connector tie-break picks one, and
///   an equal connector keeps the registered link.
///
/// A GUEST link (ADR-415) stands outside both: its name is only its claim, so it is never
/// registered under it. It gets a name this node mints in the guest namespace
/// ([`guest::GuestLink::name`], ADR-425), unique to the link — so it can be neither the
/// duplicate nor the restart of anything, and cannot share a name with any full peer,
/// here or anywhere in the cluster (KI-287).
fn establish(authenticated: handshake::Authenticated, stream: Stream, role: Role) {
    let handshake::Authenticated {
        peer,
        addr: peer_addr,
        incarnation,
        session,
        grant,
        lanes,
    } = authenticated;
    // Who initiated *this* connection (the tie-break key).
    let connector = match role {
        Role::Initiator => local_node(),
        Role::Responder => peer,
    };
    let sock = Arc::new(stream);
    let (tx, rx) = mpsc::sync_channel::<Outbound>(WRITER_QUEUE_CAP);
    let latest = LatestSlots::default();
    let last_seen = Arc::new(AtomicU64::new(now_millis()));
    let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);
    // A guest link's every frame is filtered (ADR-415), through a standing that belongs
    // to THIS connection: the `Conn` and the reader share it, so it is in place before
    // the link is visible in `NODES`, it goes when the link goes, and no other link under
    // the same name — a full peer's that reclaims it, a later guest's — can change what
    // this link's reader admits, even for frames it is still draining after the name has
    // moved on (KI-286).
    //
    // And a guest link is known by the name minted for it, never the one it claimed
    // (ADR-425): that name is its `NODES` key, the node its reader stamps on what it
    // reads, and the node of its pids here.
    let guest = grant.map(|grant| Arc::new(guest::GuestLink::new(grant, peer)));
    let guest_link = guest.is_some();
    let peer = guest.as_ref().map_or(peer, |standing| standing.name());
    let link = Conn {
        id,
        connector,
        incarnation,
        addr: peer_addr,
        tx: tx.clone(),
        lanes,
        latest: Arc::clone(&latest),
        sock: Arc::clone(&sock),
        last_seen: Arc::clone(&last_seen),
        guest: guest.clone(),
    };
    let Some(was_new) = register_link(peer, link) else {
        // We lost the tie-break: close our socket, register and spawn nothing.
        let _ = sock.shutdown(Shutdown::Both);
        return;
    };

    ensure_heartbeat();

    // The link is authenticated and registered: split the session into its two
    // directional ciphers (ADR-089) — the writer owns the send cipher, the reader the
    // receive cipher, so neither shares crypto state.
    let Session { send, recv } = session;
    spawn_link_writer(peer, Arc::clone(&sock), rx, latest, send);
    spawn_link_reader(peer, id, sock, tx, recv, last_seen, guest);

    // A brand-new peer just joined: tell the cluster (ADR-088) — one broadcast sends the
    // newcomer our other peers and tells our peers about the newcomer. Skipped for a
    // reconnect/duplicate (the name was already known) and when meshing is disabled.
    if was_new && !guest_link && mesh_enabled() {
        broadcast_peer_table();
    }
}

/// Register `link` as THE link to `peer`, resolving a duplicate against any existing
/// one (see [`establish`]). Returns `None` when an existing link to the same life of
/// the peer wins the tie-break, else whether `peer` was brand-new (no link before).
///
/// Connectors are compared by *name* — interned ids differ per process, but both ends
/// share the names, so they pick the same physical link.
fn register_link(peer: Symbol, link: Conn) -> Option<bool> {
    let (was_new, evicted) = loop {
        // The guest namespace is this node's to mint in (ADR-425): a full link claiming a
        // name in it — an old node started under one — would share a namespace with the
        // guests, so it is refused, and every name in it here is a guest link's.
        if link.guest.is_none() && guest::is_guest_name(peer) {
            eprintln!(
                "dist: refused a peer claiming the guest name {}: that namespace is a host's \
                 own, for its guest links",
                value::symbol_name(peer)
            );
            return None;
        }
        let mut nodes = crate::core::sync::write(&NODES);
        // A guest link's name was minted for it alone, so no link is ever registered under
        // it before: a guest is never the duplicate or the restart of anything (KI-287).
        debug_assert!(
            link.guest.is_none() || !nodes.contains_key(&peer),
            "a guest link's minted name was already registered"
        );
        match nodes.get(&peer) {
            Some(existing) if existing.incarnation != link.incarnation => {
                // The peer restarted. Retire the previous link OUTSIDE the
                // lock — its node-down delivers to monitors and sweeps the process
                // tables — and with `NODES` empty for this name meanwhile, so a
                // monitor or link made in that window fails over to `:noconnection`
                // rather than being recorded against a link the sweep then claims.
                // Then decide again: another link of the new life may have registered.
                let previous = nodes.remove(&peer);
                drop(nodes);
                if let Some(previous) = previous {
                    retire_previous_incarnation(peer, previous);
                }
                continue;
            }
            Some(existing)
                if value::symbol_name(link.connector) >= value::symbol_name(existing.connector) =>
            {
                // The existing link to this same life of the peer wins (its connector
                // sorts first, or it's the same initiator — a plain duplicate).
                return None;
            }
            other => {
                // We win (or there was no existing link). Take over the slot; any
                // evicted link is torn down below, outside the lock.
                let was_new = other.is_none();
                let old = nodes.remove(&peer);
                nodes.insert(peer, link);
                break (was_new, old);
            }
        }
    };
    if let Some(old) = evicted {
        // Its reader unblocks, and its `drop_link` no-ops on the new id.
        let _ = old.sock.shutdown(Shutdown::Both);
        // Whatever was queued on the evicted link dies with its writer, so nothing else
        // would ever resolve the couplings made over it: sever them now, before this
        // link's reader starts, so nothing it carries is swept. No `[:nodedown]`: the
        // node is still connected, over the winner.
        sever_couplings(peer);
    }
    Some(was_new)
}

/// The link's writer thread: pull each plaintext frame payload off the channel, **seal**
/// it (with whatever else is already queued, into one reused buffer), and write the
/// ciphertext. A per-write timeout (`WRITE_TIMEOUT`) keeps a slowloris
/// peer from pinning the writer and ballooning the queue; a timeout or a seal failure is
/// treated like any I/O error — shut the socket down.
fn spawn_link_writer(
    peer: Symbol,
    sock: Arc<Stream>,
    queue: mpsc::Receiver<Outbound>,
    latest: LatestSlots,
    mut seal: session::SealKey,
) {
    if let Err(error) = sock.set_write_timeout(Some(WRITE_TIMEOUT)) {
        eprintln!(
            "dist: warning: could not set write timeout on link to {}: {error}",
            value::symbol_name(peer)
        );
    }
    spawn_bg("dist-writer", move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            // One buffer for the life of the link: each wake seals the frame it woke for
            // and every frame already queued behind it (up to `WRITE_BATCH_BYTES`) into
            // it, and writes them with one `write_all`. The byte stream is the one a
            // write per frame produced — the frames in queue order, each sealed with the
            // next nonce — in fewer syscalls; nothing waits for a batch to fill, since
            // only what is already queued joins it.
            let mut batch: Vec<u8> = Vec::new();
            // A replaced `send-latest` ticket claims nothing and is passed over (ADR-492).
            while let Ok(item) = queue.recv() {
                let Some(payload) = claim_outbound(&latest, item) else {
                    continue;
                };
                batch.clear();
                let mut sealed = seal.seal_into(&payload, &mut batch).is_ok();
                while sealed && batch.len() < WRITE_BATCH_BYTES {
                    match queue.try_recv() {
                        Ok(next) => {
                            if let Some(next) = claim_outbound(&latest, next) {
                                sealed = seal.seal_into(&next, &mut batch).is_ok();
                            }
                        }
                        Err(_) => break,
                    }
                }
                // A seal failure leaves the frames before it intact in the batch; they
                // are written before the link goes down, as they were one at a time.
                let written = (&*sock).write_all(&batch).is_ok();
                if !(sealed && written) {
                    let _ = sock.shutdown(Shutdown::Both);
                    break;
                }
                // One huge frame must not pin its buffer for the life of the link.
                if batch.capacity() > 4 * WRITE_BATCH_BYTES {
                    batch = Vec::new();
                }
            }
        }));
    });
}

/// The link's reader thread: every inbound frame refreshes liveness and is dispatched by
/// [`handle_inbound_frame`]. On EOF/error (incl. a `shutdown` from the writer or the
/// heartbeat) it runs `drop_link`, which removes the entry iff it's still this
/// generation. The receive cipher is owned solely by this thread; a tag failure (a
/// tampered, forged, replayed or reordered frame) ends the loop and tears the link down.
fn spawn_link_reader(
    peer: Symbol,
    id: u64,
    sock: Arc<Stream>,
    writer_queue: SyncSender<Outbound>,
    mut open: session::OpenKey,
    last_seen: Arc<AtomicU64>,
    guest: Option<Arc<guest::GuestLink>>,
) {
    // One shared Pong buffer per reader; sending is an `Arc::clone`, not a copy.
    let pong: Arc<Vec<u8>> = Arc::new(encode_payload(&Frame::Pong).expect("encode Pong"));
    spawn_bg("dist-reader", move || {
        // Buffered: a frame is a 4-byte length read then a body read, so unbuffered it was
        // two syscalls a frame at least. The buffer only gathers bytes the socket already
        // has (a read returns what is there, it does not wait to fill), so a frame is
        // handled — and `last_seen` refreshed — exactly when its last byte arrives, as
        // before; the steady-state socket has no read timeout for it to interact with.
        // Nothing else reads this socket after the handshake, so nothing is lost in it.
        let mut reader = std::io::BufReader::new(&*sock);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(frame) = open.open(&mut reader) {
                last_seen.store(now_millis(), Ordering::Release);
                if let Frame::Ping = frame {
                    // Bounded queue: if we can't even enqueue a Pong, the writer is
                    // stalled (peer not draining) or gone — sever and let `drop_link`
                    // below deregister, rather than buffer.
                    if writer_queue.try_send(Arc::clone(&pong).into()).is_err() {
                        let _ = sock.shutdown(Shutdown::Both);
                        break;
                    }
                } else {
                    handle_inbound_frame(peer, guest.as_deref(), frame);
                }
            }
        }));
        drop_link(peer, id);
    });
}

/// Act on one steady-state frame from `peer`. `peer` is the *authenticated* node name
/// from the handshake — every process-coupling frame (Monitor/Demonitor/Link/Unlink/Exit)
/// is keyed to it, never to wire-supplied data, so a malicious peer can't claim to be
/// node X and inject `[:down …]` / link-exit deliveries to processes coupled with X.
/// (These frames carry no `from_node` field at all — see the security note on `Frame`.)
///
/// `guest` is the standing of the link the frame was READ from, as `establish` gave it to
/// that link's reader — never looked up by `peer`, which another link may hold by now.
fn handle_inbound_frame(peer: Symbol, guest: Option<&guest::GuestLink>, mut frame: Frame) {
    // A guest link acts only on frames that reach what it was given and carry no code
    // (ADR-415); a full link's pass untouched.
    if !guest::admit(peer, guest, &frame) {
        return;
    }
    // A guest names its own pids by the name it claimed, which is no link here: they
    // become pids of the name this node minted for the link (ADR-425), so a reply routes
    // back over it — and a pid it spells with any other node's name stays that node's.
    if let Some(guest) = guest {
        guest.translate_inbound(&mut frame);
    }
    match frame {
        Frame::Send { target, msg, lane } => process::interrupt::delivering_from(
            process::interrupt::Origin::Peer {
                node: peer,
                guest: guest.is_some(),
            },
            || deliver_inbound(target, msg, lane),
        ),
        // A peer asked to watch one of our local pids — the shared `add_monitor` core
        // with a `Watcher::Remote`, so the alive/dead-target paths are the local ones.
        Frame::Monitor {
            watcher_pid,
            target,
            mref,
        } => process::add_monitor(
            target,
            process::Watcher::Remote {
                node: peer,
                pid: watcher_pid,
                mref,
            },
        ),
        // The same for a process registered under `name` here: resolve it now, and an
        // unregistered name watches the placeholder pid, which fires `:noproc` at once —
        // the local `monitor`'s behaviour for an unregistered name.
        Frame::MonitorName {
            watcher_pid,
            name,
            mref,
        } => process::add_monitor(
            whereis(name).unwrap_or(u64::MAX),
            process::Watcher::Remote {
                node: peer,
                pid: watcher_pid,
                mref,
            },
        ),
        // Peer dropped a remote monitor — the local `demonitor`'s `drop_monitor`, matching
        // the Remote variant's identity (node + pid + mref).
        Frame::Demonitor { watcher_pid, mref } => process::drop_monitor(|watcher| {
            matches!(*watcher, process::Watcher::Remote { node, pid, mref: r }
                         if node == peer && pid == watcher_pid && r == mref)
        }),
        // A monitor we asked the peer to keep just fired. Retire our `PENDING_REMOTE`
        // entry (a later node-down must not fire it again, KI-96), then deliver the
        // `[:down …]`, the dying pid node-qualified by the authenticated `peer`.
        Frame::Down {
            watcher_pid,
            mref,
            target_pid,
            reason,
        } => process::deliver_remote_down(peer, watcher_pid, mref, target_pid, reason),
        // A peer linked its `from_pid` to our local `to_pid` — record our half.
        Frame::Link { from_pid, to_pid } => {
            // `to_pid` is wire data and may name a dead pid; recording that would leak
            // an entry forever (see `record_remote_link`), so tell the peer instead — its
            // linked process gets `:noproc`, exactly what a LOCAL `link` to a dead pid gets.
            if !process::record_remote_link(to_pid, peer, from_pid) {
                send_link_exit(peer, from_pid, to_pid, Message::Keyword(*pk::NOPROC_SYMBOL));
            }
        }
        Frame::Unlink { from_pid, to_pid } => {
            process::drop_remote_link(to_pid, peer, from_pid);
        }
        // An exit signal for our local `to_pid`: a link death goes through the
        // trap-or-propagate path; an explicit remote exit is kill-style.
        Frame::Exit {
            from_pid,
            to_pid,
            reason,
            link,
        } => {
            if link {
                process::deliver_remote_link_exit(to_pid, peer, from_pid, reason);
            } else {
                process::exit(to_pid, reason);
            }
        }
        // Cluster-mesh gossip: dial any node the peer knows that we don't (ADR-088).
        Frame::Peers { peers } => mesh_consider(peers),
        // `Ping` is answered by the reader itself. Handshake-only frames in steady state
        // are malformed but harmless — keep reading.
        Frame::Ping | Frame::Pong | Frame::Hello { .. } | Frame::Auth { .. } => {}
    }
}

/// Send every connected peer the current peer table (each *other* peer's name +
/// dial address), so newcomers and incumbents converge to a full mesh. Idempotent:
/// a recipient ignores any entry it's already connected to, so re-broadcasting on
/// each join can't loop. Entries with no advertised address are skipped — a peer
/// that isn't listening can't be dialed onward.
fn broadcast_peer_table() {
    // Snapshot the peer table (cheap: Arc/channel clones) and release the
    // NODES lock before encoding or enqueueing. `enqueue` calls
    // `sock.shutdown()` when the writer queue is full — that syscall must not
    // run while holding NODES or it delays concurrent link registration and
    // teardown for the duration of every shutdown it triggers.
    struct PeerSnap {
        name: Symbol,
        addr: String,
        tx: SyncSender<Outbound>,
        sock: Arc<Stream>,
    }
    let snaps: Vec<PeerSnap> = {
        let nodes = crate::core::sync::read(&NODES);
        nodes
            .iter()
            // a guest is neither told about the cluster nor introduced to it (ADR-415)
            .filter(|(_, conn)| conn.guest.is_none())
            .map(|(&name, c)| PeerSnap {
                name,
                addr: c.addr.clone(),
                tx: c.tx.clone(),
                sock: Arc::clone(&c.sock),
            })
            .collect()
    };
    for s in &snaps {
        let peers: Vec<(Symbol, String)> = snaps
            .iter()
            .filter(|p| p.name != s.name && !p.addr.is_empty())
            .map(|p| (p.name, p.addr.clone()))
            .collect();
        if peers.is_empty() {
            continue;
        }
        if let Ok(bytes) = encode_payload(&Frame::Peers { peers })
            && s.tx.try_send(Arc::new(bytes).into()).is_err()
        {
            let _ = s.sock.shutdown(Shutdown::Both);
        }
    }
}

/// Handle an inbound gossip list: dial any named peer we're neither connected to
/// nor already dialing. Each dial runs on its own short-lived thread (the dial +
/// handshake blocks, and must not stall the reader). The `PENDING_DIALS` claim
/// makes concurrent gossip frames naming the same peer dial it once; the dialed
/// link's own `establish` re-gossips, so the mesh closes transitively.
fn mesh_consider(peers: Vec<(Symbol, String)>) {
    if !mesh_enabled() {
        return;
    }
    let me = local_node();
    // Snapshot who we're already linked to (a different lock than PENDING_DIALS;
    // take it first and drop it before claiming, so the two are never held nested).
    // No gossiped name is a guest's here: a guest's name is minted by its host, in a
    // namespace a full peer may not use (ADR-425), and no host gossips its guests — so a
    // gossiped name in that namespace is no node to dial, and is skipped.
    let connected: HashSet<Symbol> = crate::core::sync::read(&NODES).keys().copied().collect();
    let mut to_dial: Vec<(Symbol, String)> = Vec::new();
    {
        let mut pending = crate::core::sync::write(&PENDING_DIALS);
        for (name, addr) in peers {
            if name == me
                || addr.is_empty()
                || guest::is_guest_name(name)
                || connected.contains(&name)
                || pending.contains(&name)
            {
                continue;
            }
            pending.insert(name);
            to_dial.push((name, addr));
        }
    }
    for (name, addr) in to_dial {
        // One thread per gossiped peer, and a `Peers` frame may name up to
        // MAX_GOSSIP_PEERS of them — the highest-rate, most attacker-influenced spawn in
        // the runtime. A refusal must drop this dial, not unwind the caller.
        let pending_name = name;
        if !spawn_bg("dist-dial", move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Best-effort: a peer may be unreachable or a simultaneous dial
                // from the other side may win the tie-break — either way we just
                // stop. `node_connect` re-checks `NODES` and handles the self/dup
                // guards; `establish` does the tie-break.
                let _ = node_connect(name, &addr);
            }));
            crate::core::sync::write(&PENDING_DIALS).remove(&name);
        }) {
            // The dial never started, so nothing will clear the in-flight marker we
            // inserted above; drop it here or this peer could never be dialed again.
            crate::core::sync::write(&PENDING_DIALS).remove(&pending_name);
        }
    }
}

/// Remove a link from `NODES` **iff** it's still this generation (so an evicted or
/// replaced link can't tear down its successor), and fire node-down watchers.
fn drop_link(peer: Symbol, id: u64) {
    let removed = remove_current_link(peer, id);
    if removed {
        forget_dial_routes(peer);
        fire_nodedown(peer);
    }
}

/// Remove `peer`'s `NODES` entry iff it is still generation `id`; `true` when this call
/// removed it, so the caller is the one that fires its node-down.
fn remove_current_link(peer: Symbol, id: u64) -> bool {
    let mut nodes = crate::core::sync::write(&NODES);
    match nodes.get(&peer) {
        Some(c) if c.id == id => {
            nodes.remove(&peer);
            true
        }
        _ => false,
    }
}

/// Tear down `previous`, a link to an earlier life of `peer` that `establish` has
/// already removed from `NODES` because the peer came back with a new incarnation.
///
/// This is a real node-down, not a duplicate eviction: the process at the far end of
/// `previous` is gone, so its watchers get `[:nodedown]`, every monitor and link made
/// over it gets its `:noconnection`, and the watchers it held on our pids are dropped
/// — exactly what the heartbeat would have done once it expired the link, only now
/// rather than `DOWN_AFTER` later. The link's own reader then hits the shutdown and
/// runs `drop_link`, which no-ops on the generation id.
fn retire_previous_incarnation(peer: Symbol, previous: Conn) {
    let _ = previous.sock.shutdown(Shutdown::Both);
    forget_dial_routes(peer);
    fire_nodedown(peer);
}

/// Deliver `[:nodedown name]` to every process that called `(monitor-node name)`,
/// and fire any pid-monitors that crossed this link — pending remote monitors
/// fire `:noconnection` to their local watchers, and inbound remote watchers
/// the peer had registered are dropped (no point keeping entries that route
/// to a vanished peer). All three sit behind one node-down trigger so a
/// reconnect later starts from a clean slate.
fn fire_nodedown(peer: Symbol) {
    let watchers = crate::core::sync::read(&NODE_MONITORS).get(&peer).cloned();
    if let Some(watchers) = watchers {
        let msg = nodedown_msg(peer);
        for w in watchers {
            process::deliver(w, msg.clone());
        }
    }
    sever_couplings(peer);
}

/// The net-split cleanup for every coupling made over a link to `peer` that is gone:
/// each pending remote monitor fires `:noconnection`, as does each cross-node link
/// (ADR-067), and the watchers `peer` held on our pids are dropped. Each entry is claimed,
/// so it fires once. A node-down runs it after `[:nodedown]`; an evicted duplicate link
/// runs it alone, since the node is still up over the winner.
fn sever_couplings(peer: Symbol) {
    process::handle_node_down(peer);
    process::handle_link_node_down(peer);
}

/// An inbound `Send` from a peer: resolve the target locally and deliver.
fn deliver_inbound(target: Target, msg: Message, lane: process::Lane) {
    let id = match target {
        Target::Pid(id) => id,
        Target::Name(name) => match crate::core::sync::read(&NAMES).get(name) {
            Some(id) => id,
            None => {
                warn_dropped_to_unregistered_name(name, "inbound");
                return;
            }
        },
    };
    process::deliver_in_lane(id, msg, lane);
}

pub(crate) mod guest;
mod handshake;
mod heartbeat;
mod session;
pub(crate) mod wire;

use heartbeat::ensure_heartbeat;
use session::Session;
use wire::{Frame, encode_payload};

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    thread_local! {
        static SETUP_GAP_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Run, once, whatever a test armed for the gap between `monitor_remote` /
    /// `link_remote` recording their entry and sending the frame — the window a link
    /// can drop in.
    pub(super) fn run_setup_gap_hook() {
        if let Some(hook) = SETUP_GAP_HOOK.with(|slot| slot.borrow_mut().take()) {
            hook();
        }
    }

    fn arm_setup_gap_hook(hook: impl FnOnce() + 'static) {
        SETUP_GAP_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    /// A link that drops between `monitor_remote` recording its pending entry and
    /// sending the `Monitor` frame fires the watcher's `:noconnection` through the
    /// node-down path; the send then fails, and the fallback must not fire it again.
    #[test]
    fn a_link_dropping_during_monitor_setup_fires_one_noconnection() {
        let me = process::self_pid();
        let node = value::intern("gone-during-monitor@test");
        let mref = process::next_ref();
        arm_setup_gap_hook(move || fire_nodedown(node));
        monitor_remote(node, 7, me, mref);
        assert_eq!(
            process::mailbox_len(me),
            Some(1),
            "one [:down … :noconnection] per monitor, not one from each path"
        );
        // Control: with no drop in the gap, the fallback alone fires it.
        monitor_remote(node, 8, me, process::next_ref());
        assert_eq!(process::mailbox_len(me), Some(2));
    }

    /// The link twin: a link dropping between `link_remote`'s record and its send
    /// delivers one `[:EXIT … :noconnection]`, not two.
    #[test]
    fn a_link_dropping_during_link_setup_fires_one_noconnection() {
        let me = process::self_pid();
        process::set_trap_exit(me, true);
        let node = value::intern("gone-during-link@test");
        arm_setup_gap_hook(move || fire_nodedown(node));
        link_remote(node, 9, me);
        assert_eq!(
            process::mailbox_len(me),
            Some(1),
            "one [:EXIT … :noconnection] per link, not one from each path"
        );
        link_remote(node, 10, me);
        assert_eq!(process::mailbox_len(me), Some(2));
    }

    /// A remote DOWN already in flight when the watcher demonitors must not land after
    /// the demonitor — the guarantee the local path makes (KI-213).
    #[test]
    fn a_remote_down_arriving_after_demonitor_is_dropped() {
        let me = process::self_pid();
        let node = value::intern("peer-for-demonitor@test");
        let reason = Message::Keyword(value::intern("normal"));
        let mref = process::next_ref();
        process::record_pending_remote(node, 11, me, mref);
        demonitor_remote(node, me, mref);
        process::deliver_remote_down(node, me, mref, 11, reason.clone());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "a DOWN for a demonitored ref was delivered"
        );
        // Control: a monitor still pending gets its DOWN.
        let pending = process::next_ref();
        process::record_pending_remote(node, 12, me, pending);
        process::deliver_remote_down(node, me, pending, 12, reason);
        assert_eq!(process::mailbox_len(me), Some(1));
    }

    /// A remote link exit already in flight when the local process unlinks must not
    /// arrive after the unlink.
    #[test]
    fn a_remote_link_exit_arriving_after_unlink_is_dropped() {
        let me = process::self_pid();
        process::set_trap_exit(me, true);
        let node = value::intern("peer-for-unlink@test");
        let reason = Message::Keyword(value::intern("boom"));
        assert!(process::record_remote_link(me, node, 13));
        unlink_remote(node, 13, me);
        process::deliver_remote_link_exit(me, node, 13, reason.clone());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "a link exit for an unlinked peer was delivered"
        );
        // Control: a live link's exit arrives.
        assert!(process::record_remote_link(me, node, 14));
        process::deliver_remote_link_exit(me, node, 14, reason);
        assert_eq!(process::mailbox_len(me), Some(1));
    }

    /// A duplicate link that loses the tie-break to a newer one is evicted with its
    /// writer queue — and with it any `Monitor` request still queued there. Its reader's
    /// `drop_link` no-ops on the new id, so the eviction itself must resolve the
    /// couplings made over it: a pending remote monitor gets its `:noconnection`.
    #[test]
    fn evicting_a_link_fires_noconnection_for_its_pending_monitors() {
        let me = process::self_pid();
        // This runtime never ran `node-start`, so it is `nonode`: a link WE dialed has
        // connector "nonode", one the peer dialed has the peer's name. "nonode" sorts
        // before "zz-…", so the second (Initiator) link evicts the first.
        let peer = value::intern("zz-evicted-peer@test");
        let (first, _first_far_end) = stream_pair();
        establish(authenticated_as(peer, 5), first, Role::Responder);
        // A monitor that went out over the first link.
        process::record_pending_remote(peer, 21, me, process::next_ref());
        let (second, _second_far_end) = stream_pair();
        establish(authenticated_as(peer, 5), second, Role::Initiator);
        assert_eq!(
            crate::core::sync::read(&NODES)
                .get(&peer)
                .map(|link| link.connector),
            Some(local_node()),
            "the newer, smaller-connector link must have won"
        );
        assert_eq!(
            process::mailbox_len(me),
            Some(1),
            "the evicted link's pending monitor must get its :noconnection"
        );
    }

    /// A connected link carrier and its far end, over loopback TCP: the transport every
    /// platform has, so these tests run on Windows as on Unix.
    fn stream_pair() -> (Stream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far, _) = listener.accept().unwrap();
        (Stream::Tcp(near), far)
    }

    /// What a completed handshake with `peer` (in its life `incarnation`) hands
    /// `establish`, with a throwaway session.
    fn authenticated_as(peer: Symbol, incarnation: u64) -> handshake::Authenticated {
        handshake::Authenticated {
            peer,
            addr: String::new(),
            incarnation,
            grant: None,
            lanes: true,
            session: Session {
                send: session::SealKey::new([1; session::KEY_LEN]),
                recv: session::OpenKey::new([2; session::KEY_LEN]),
            },
        }
    }

    fn registered_link(peer: Symbol) -> Option<(u64, u64)> {
        crate::core::sync::read(&NODES)
            .get(&peer)
            .map(|link| (link.id, link.incarnation))
    }

    /// The same, for a GUEST link (ADR-415) authenticated with a grant that names nothing.
    fn authenticated_as_guest(peer: Symbol, incarnation: u64) -> handshake::Authenticated {
        handshake::Authenticated {
            grant: Some(Arc::new(guest::Grant {
                cookie: "unit-test-guest-cookie".to_string(),
                names: HashSet::new(),
                as_name: None,
            })),
            ..authenticated_as(peer, incarnation)
        }
    }

    /// The guest standing of the link now registered for `peer`, if it is a guest link.
    fn registered_guest(peer: Symbol) -> Option<Arc<guest::GuestLink>> {
        crate::core::sync::read(&NODES)
            .get(&peer)
            .and_then(|link| link.guest.clone())
    }

    /// The minted name and standing of every guest link registered as having claimed to
    /// be `claimed` (ADR-425), in no particular order.
    fn guests_claiming(claimed: Symbol) -> Vec<(Symbol, Arc<guest::GuestLink>)> {
        crate::core::sync::read(&NODES)
            .iter()
            .filter_map(|(&name, link)| {
                link.guest
                    .as_ref()
                    .filter(|standing| standing.claimed() == claimed)
                    .map(|standing| (name, Arc::clone(standing)))
            })
            .collect()
    }

    /// The one guest link that claimed `claimed`.
    fn the_guest_claiming(claimed: Symbol) -> (Symbol, Arc<guest::GuestLink>) {
        let mut found = guests_claiming(claimed);
        assert_eq!(found.len(), 1, "one guest link claimed the name");
        found.pop().expect("one")
    }

    /// KI-286: a guest link's reader can still be delivering frames it read after its
    /// socket was shut down — by `node/disconnect`, say — and after a full link came up
    /// under the name the guest claimed. Those frames are the guest's. When the filter
    /// was keyed by name, a full link taking the name emptied the guest's entry and they
    /// were admitted as a full peer's — a `:remote-spawn` carrying a closure among them.
    /// The reader hands each frame to `handle_inbound_frame` with ITS link's standing;
    /// that must refuse it whoever holds which name, while the same frame from a full link
    /// is delivered.
    #[test]
    fn a_guest_links_frames_stay_a_guests_whoever_holds_its_claimed_name() {
        let me = process::self_pid();
        let peer = value::intern("reclaimed-guest-peer@test");
        let probe = value::intern("ki-286-probe@test");
        assert!(matches!(register(probe, me), Registration::Bound));
        let to_probe = || Frame::Send {
            target: Target::Name(probe),
            msg: Message::Keyword(value::intern("ki-286")),
            lane: crate::process::Lane::Normal,
        };

        let (guest_stream, _guest_far_end) = stream_pair();
        establish(
            authenticated_as_guest(peer, 1),
            guest_stream,
            Role::Responder,
        );
        let (guest_name, guest_standing) = the_guest_claiming(peer);

        // A full link under the name the guest claimed: a different link, under a
        // different name, so the guest's is untouched (ADR-425).
        let (full_stream, _full_far_end) = stream_pair();
        establish(authenticated_as(peer, 2), full_stream, Role::Responder);
        assert!(
            registered_link(peer).is_some() && registered_guest(peer).is_none(),
            "the claimed name is the full link's"
        );
        handle_inbound_frame(guest_name, Some(&guest_standing), to_probe());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "a guest link's frame was admitted with a full link's privileges"
        );

        // And once the guest link is gone, with its reader still draining.
        assert!(disconnect(guest_name));
        handle_inbound_frame(guest_name, Some(&guest_standing), to_probe());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "a guest link's frame was admitted once it had no link"
        );

        // The control: the same frame from a full link is delivered.
        handle_inbound_frame(peer, None, to_probe());
        assert_eq!(process::mailbox_len(me), Some(1));
        assert!(unregister(probe));
        assert!(disconnect(peer));
    }

    /// KI-286's other half: a local pid sent to a peer becomes reachable by the LINK the
    /// message went out on. A pid sent to the full peer whose name the guest claimed went
    /// to that full link, so the guest link must not be able to reach it.
    #[test]
    fn a_pid_sent_to_a_peer_is_reachable_only_by_the_link_it_went_out_on() {
        let peer = value::intern("introduced-guest-peer@test");
        let local_pid = |id| Message::Pid { node: *NONODE, id };
        let to_pid = |id| Frame::Send {
            target: Target::Pid(id),
            msg: Message::Keyword(value::intern("ki-286")),
            lane: crate::process::Lane::Normal,
        };

        let (guest_stream, _guest_far_end) = stream_pair();
        establish(
            authenticated_as_guest(peer, 1),
            guest_stream,
            Role::Responder,
        );
        let (guest_name, guest_standing) = the_guest_claiming(peer);
        assert!(route(guest_name, Target::Pid(1), local_pid(77)));
        assert!(
            guest::admit(guest_name, Some(&guest_standing), &to_pid(77)),
            "a pid sent over the guest link is reachable by it"
        );

        let (full_stream, _full_far_end) = stream_pair();
        establish(authenticated_as(peer, 2), full_stream, Role::Responder);
        assert!(route(peer, Target::Pid(1), local_pid(78)));
        assert!(
            !guest::admit(guest_name, Some(&guest_standing), &to_pid(78)),
            "a pid sent over the full link under the guest's claimed name reached the guest"
        );
        assert!(disconnect(peer));
        assert!(disconnect(guest_name));
    }

    /// KI-287 / ADR-425: a guest's node name is only its claim, so it is never the name
    /// it is registered under — not a connected full peer's (whose link it used to
    /// retire, taking its traffic and the pids in it), not a full peer's that is down,
    /// and not one that is a full peer's somewhere in the cluster and has NEVER linked
    /// here, which no check on this node could have known to refuse. Each gets a name
    /// minted in the guest namespace, and a pid of the real peer routes to no guest.
    #[test]
    fn a_guest_is_never_registered_under_the_name_it_claims() {
        let me = process::self_pid();
        let peer = value::intern("full-peer-name-taken@test");
        let (full_stream, _full_far_end) = stream_pair();
        establish(authenticated_as(peer, 1), full_stream, Role::Responder);
        let full_link = registered_link(peer).expect("the full link registers");
        monitor_node(peer, me);

        let (guest_stream, _guest_far_end) = stream_pair();
        establish(
            authenticated_as_guest(peer, 2),
            guest_stream,
            Role::Responder,
        );
        assert_eq!(
            registered_link(peer),
            Some(full_link),
            "a guest claiming a full peer's name displaced its link"
        );
        assert!(registered_guest(peer).is_none());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "the full peer never went down"
        );
        let (guest_name, _) = the_guest_claiming(peer);
        assert!(guest::is_guest_name(guest_name) && guest_name != peer);

        // The full peer goes away; a guest claiming its name still does not get it.
        assert!(disconnect(peer));
        assert_eq!(process::mailbox_len(me), Some(1), "its [:nodedown]");
        demonitor_node(peer, me);
        let (late_guest_stream, _late_guest_far_end) = stream_pair();
        establish(
            authenticated_as_guest(peer, 3),
            late_guest_stream,
            Role::Responder,
        );
        assert_eq!(
            registered_link(peer),
            None,
            "a guest took the name of a full peer that was down"
        );

        // The case KI-287 left open: a full peer elsewhere in the cluster that has never
        // linked here. A third node forwarding one of its pids here must not reach the
        // guest that claimed its name: nothing is registered under it, so the send has no
        // route (`:noconnection` to a sender that asked) rather than going to the guest.
        let never_linked = value::intern("never-linked-full-peer@elsewhere");
        let (stranger_stream, _stranger_far_end) = stream_pair();
        establish(
            authenticated_as_guest(never_linked, 4),
            stranger_stream,
            Role::Responder,
        );
        assert_eq!(
            registered_link(never_linked),
            None,
            "a guest took the name of a full peer that has never linked here"
        );
        assert!(
            !route(
                never_linked,
                Target::Pid(5),
                Message::Keyword(value::intern("for-the-real-peer"))
            ),
            "a pid of a never-linked full peer routed to the guest that claimed its name"
        );
        let (stranger_name, _) = the_guest_claiming(never_linked);
        for (name, _) in guests_claiming(peer) {
            assert!(disconnect(name));
        }
        assert!(disconnect(stranger_name));
    }

    /// Two guests claiming one name are two links under two names — neither the
    /// duplicate nor the restart of the other — and a full link under the name they
    /// claimed is a third, displacing neither; a dial never takes a guest's link for the
    /// peer it was looking for, whether by name, by an earlier route or by the address a
    /// guest advertised.
    #[test]
    fn guests_and_a_full_link_claiming_one_name_are_separate_links() {
        let peer = value::intern("guest-held-name@test");
        let address = "unix:/nonexistent/brood-test/guest-held-name.sock";
        let advertising = |incarnation| handshake::Authenticated {
            addr: address.to_string(),
            ..authenticated_as_guest(peer, incarnation)
        };
        let (first_stream, _first_far_end) = stream_pair();
        establish(advertising(5), first_stream, Role::Responder);
        let (second_stream, _second_far_end) = stream_pair();
        establish(advertising(6), second_stream, Role::Responder);
        let guests = guests_claiming(peer);
        assert_eq!(
            guests.len(),
            2,
            "a second guest claiming a name retired the first, or was refused"
        );
        assert_ne!(guests[0].0, guests[1].0);

        crate::core::sync::write(&DIAL_ROUTES).insert(address.to_string(), guests[0].0);
        assert_eq!(
            linked_peer_for(peer, address),
            None,
            "a dial reused a guest's link as the peer it was looking for"
        );

        let (full_stream, _full_far_end) = stream_pair();
        establish(authenticated_as(peer, 5), full_stream, Role::Responder);
        assert!(
            registered_link(peer).is_some() && registered_guest(peer).is_none(),
            "the full link registers under its own name"
        );
        assert_eq!(
            guests_claiming(peer).len(),
            2,
            "a full link displaced a guest that had claimed its name"
        );
        assert!(disconnect(peer));
        for (name, _) in guests {
            assert!(disconnect(name));
        }
        crate::core::sync::write(&DIAL_ROUTES).remove(address);
    }

    /// The guest namespace is the host's alone: a full link claiming a name in it is
    /// refused, and `node/start` will not take one, so every name in it is a guest link
    /// some host minted (ADR-425).
    #[test]
    fn no_full_peer_may_use_the_guest_namespace() {
        let squatter = value::intern("guest~0.0000000000000000~squatter@test");
        let (stream, _far_end) = stream_pair();
        establish(authenticated_as(squatter, 1), stream, Role::Responder);
        assert_eq!(
            registered_link(squatter),
            None,
            "a full link registered under a name in the guest namespace"
        );
        let refused = node_listen(
            value::intern("guest~started-by-hand@test"),
            "tcp:127.0.0.1:0",
            "a-long-enough-cookie-for-the-test".to_string(),
        );
        assert_eq!(
            refused.map_err(|error| error.kind()),
            Err(io::ErrorKind::InvalidInput),
            "node/start took a name in the guest namespace"
        );
    }

    /// What goes out over a guest link names the guest's pids the way the guest spells
    /// itself (ADR-425): read off the far end of the socket, a pid of the minted name is a
    /// pid of the claimed one, so the guest's own processes are still its own.
    #[test]
    fn a_guest_receives_its_own_pids_in_its_own_spelling() {
        let peer = value::intern("spelled-by-itself@test");
        let (guest_stream, guest_far_end) = stream_pair();
        establish(
            authenticated_as_guest(peer, 1),
            guest_stream,
            Role::Responder,
        );
        let (guest_name, _) = the_guest_claiming(peer);
        assert!(route(
            guest_name,
            Target::Pid(3),
            Message::Vector(vec![Message::Pid {
                node: guest_name,
                id: 4
            }]),
        ));
        guest_far_end
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        // The far end opens what the link's writer sealed (`authenticated_as`'s send key).
        let mut open = session::OpenKey::new([1; session::KEY_LEN]);
        let mut reader = std::io::BufReader::new(&guest_far_end);
        let msg = loop {
            match open.open(&mut reader).expect("the routed frame arrives") {
                Frame::Send { msg, .. } => break msg,
                _ => continue, // a heartbeat ping
            }
        };
        let Message::Vector(ref items) = msg else {
            panic!("the message arrives as sent")
        };
        assert!(
            matches!(items[0], Message::Pid { node, id: 4 } if node == peer),
            "a guest was sent its own pid under the name the host minted for it"
        );
        assert!(disconnect(guest_name));
    }

    /// A lane reaches the wire only on a link to a v11 peer (ADR-492): to a v10 peer, a
    /// priority and a latest send each go as the plain `Send` it knows, and to a v11 peer
    /// both keep their lane.
    #[test]
    fn a_lane_goes_on_the_wire_only_to_a_v11_peer() {
        let lanes_seen = |peer: &str, lanes: bool| {
            let peer = value::intern(peer);
            let (stream, far_end) = stream_pair();
            establish(
                handshake::Authenticated {
                    lanes,
                    ..authenticated_as(peer, 1)
                },
                stream,
                Role::Responder,
            );
            let latest = process::Lane::Latest(Box::new(":frame".to_string()));
            for lane in [process::Lane::Priority, latest] {
                assert!(route_in_lane(peer, Target::Pid(3), Message::Int(1), lane));
            }
            far_end
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut open = session::OpenKey::new([1; session::KEY_LEN]);
            let mut reader = std::io::BufReader::new(&far_end);
            let mut seen = Vec::new();
            while seen.len() < 2 {
                if let Frame::Send { lane, .. } = open.open(&mut reader).expect("a frame") {
                    seen.push(lane);
                }
            }
            assert!(disconnect(peer));
            seen
        };
        assert_eq!(
            lanes_seen("v10-lane-peer@test", false),
            vec![process::Lane::Normal, process::Lane::Normal]
        );
        assert_eq!(
            lanes_seen("v11-lane-peer@test", true),
            vec![
                process::Lane::Priority,
                process::Lane::Latest(Box::new(":frame".to_string()))
            ]
        );
    }

    /// A `send-latest` replaces its slot's frame in a link's writer queue while it is
    /// unwritten, its replacement written after everything queued before it; another
    /// target's slot, a plain frame, and a slot already written are left alone (ADR-492).
    #[test]
    fn a_send_latest_replaces_its_unwritten_frame_in_the_link_queue() {
        let (tx, rx) = mpsc::sync_channel::<Outbound>(16);
        let (stream, _far_end) = stream_pair();
        let sock = Arc::new(stream);
        let latest = LatestSlots::default();
        let payload = |byte: u8| Arc::new(vec![byte]);
        let frame_slot = latest_slot(&Target::Pid(1), ":frame");
        let other_slot = latest_slot(&Target::Pid(2), ":frame");
        assert!(enqueue_latest(
            &tx,
            &sock,
            &latest,
            frame_slot.clone(),
            payload(1)
        ));
        assert!(enqueue_to(&tx, &sock, payload(2)));
        assert!(enqueue_latest(&tx, &sock, &latest, other_slot, payload(3)));
        assert!(enqueue_latest(
            &tx,
            &sock,
            &latest,
            frame_slot.clone(),
            payload(4)
        ));
        let written = |count: usize| -> Vec<u8> {
            rx.try_iter()
                .take(count)
                .filter_map(|item| claim_outbound(&latest, item))
                .map(|bytes| bytes[0])
                .collect()
        };
        assert_eq!(written(4), vec![2, 3, 4]);
        assert!(crate::core::sync::lock(&latest).is_empty());
        // Written, the slot is free: the next one is queued and written in its turn.
        assert!(enqueue_latest(&tx, &sock, &latest, frame_slot, payload(5)));
        assert_eq!(written(1), vec![5]);
    }

    /// A peer that restarted (its old host vanished without a FIN, so the old link is
    /// still registered) and dials us again must get its new link — the old one reaches
    /// a process that no longer exists. The incarnation is what tells the two lives
    /// apart (same connector), so the new link is not refused as a duplicate; the old
    /// life's watchers get their node-down.
    #[test]
    fn a_restarted_peer_replaces_its_previous_life_and_fires_its_node_down() {
        let me = process::self_pid();
        let peer = value::intern("restarted-peer@test");
        let (old_life, _old_far_end) = stream_pair();
        establish(authenticated_as(peer, 1), old_life, Role::Responder);
        let (old_id, _) = registered_link(peer).expect("the first life registers");
        monitor_node(peer, me);
        // A pid monitor made over the old life.
        process::record_pending_remote(peer, 31, me, process::next_ref());
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "nothing fires while it is up"
        );

        // The peer comes back and dials us: the same connector (the peer) as before.
        let (new_life, _new_far_end) = stream_pair();
        establish(authenticated_as(peer, 2), new_life, Role::Responder);
        let (new_id, new_incarnation) = registered_link(peer).expect("the new life registers");
        assert_ne!(
            new_id, old_id,
            "the restarted peer's link must replace the old one"
        );
        assert_eq!(new_incarnation, 2);
        assert_eq!(
            process::mailbox_len(me),
            Some(2),
            "the old life's node monitor gets [:nodedown] and its pid monitor :noconnection"
        );
        demonitor_node(peer, me);
    }

    /// The other half of the same decision: a second link to the SAME life of a live
    /// peer — the shape two dials racing can produce — must not disturb the link that is
    /// up. Same connector, same incarnation: the registered link stays, and no monitor
    /// hears anything.
    #[test]
    fn a_second_link_to_the_same_life_leaves_the_live_link_alone() {
        let me = process::self_pid();
        let peer = value::intern("same-life-peer@test");
        let (first, _first_far_end) = stream_pair();
        establish(authenticated_as(peer, 7), first, Role::Responder);
        let registered = registered_link(peer).expect("the first link registers");
        monitor_node(peer, me);
        let mref = process::next_ref();
        process::record_pending_remote(peer, 41, me, mref);

        let (second, _second_far_end) = stream_pair();
        establish(authenticated_as(peer, 7), second, Role::Responder);
        assert_eq!(
            registered_link(peer),
            Some(registered),
            "a duplicate of a live link must lose to it"
        );
        assert_eq!(
            process::mailbox_len(me),
            Some(0),
            "no [:nodedown] and no :noconnection for a peer that never went away"
        );
        demonitor_node(peer, me);
        process::drop_pending_remote(peer, me, mref);
    }

    /// A connect that names a live peer by another spelling — `(node/connect "ed")`
    /// claims `ed@<this host>` for a node that calls itself `ed@127.0.0.1` — is
    /// answered from the existing link reached through the same address, without
    /// dialing. The address here is unreachable, so a dial would fail the call.
    #[test]
    fn a_connect_spelled_differently_reuses_the_link_its_address_reached() {
        let peer = value::intern("spelled-peer@127.0.0.1");
        let address = "unix:/nonexistent/brood-test/spelled-peer.sock";
        let (link, _far_end) = stream_pair();
        establish(authenticated_as(peer, 9), link, Role::Responder);
        crate::core::sync::write(&DIAL_ROUTES).insert(address.to_string(), peer);
        let other_spelling = value::intern("spelled-peer@some-host");
        assert_eq!(
            node_connect(other_spelling, address).ok(),
            Some(peer),
            "a second connect to a live peer must reuse its link, whatever the spelling"
        );
        // Once the link is gone, the route proves nothing: the dial is attempted.
        let id = registered_link(peer).expect("registered").0;
        drop_link(peer, id);
        assert!(
            node_connect(other_spelling, address).is_err(),
            "a route to a node that is no longer linked must not answer a connect"
        );
    }

    /// `proc/register`'s one refusal, decided under the NAMES lock: a dead pid binds
    /// nothing — and does not disturb a live holder's binding either.
    #[test]
    fn register_binds_only_live_pids() {
        let me = process::self_pid();
        let name = value::intern("register-refusals@test");
        assert!(matches!(
            register(name, u64::MAX - 77),
            Registration::DeadPid
        ));
        assert_eq!(whereis(name), None, "a dead pid's name was bound");
        assert!(matches!(register(name, me), Registration::Bound));
        assert!(matches!(register(name, me), Registration::Bound));
        assert!(matches!(
            register(name, u64::MAX - 77),
            Registration::DeadPid
        ));
        assert_eq!(
            whereis(name),
            Some(me),
            "a dead pid displaced a live holder"
        );
    }

    /// The pre-auth connection gate: slots are bounded at the cap, the
    /// over-count from a losing `try_acquire` is rolled back (so the live count
    /// never drifts above the cap), and a dropped slot frees capacity again.
    /// On its own counter: the process's is shared with every sibling test thread
    /// under `cargo test`, and a listener test's connect holds a real slot.
    #[test]
    fn handshake_slot_caps_in_flight_and_releases_on_drop() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        // Fill every slot.
        let held: Vec<HandshakeSlot> = (0..MAX_IN_FLIGHT_HANDSHAKES)
            .map(|_| HandshakeSlot::try_acquire_on(&COUNT).expect("under the cap"))
            .collect();
        assert_eq!(COUNT.load(Ordering::Acquire), MAX_IN_FLIGHT_HANDSHAKES);

        // One past the cap is shed, and the failed attempt rolled its count back.
        assert!(
            HandshakeSlot::try_acquire_on(&COUNT).is_none(),
            "cap must shed"
        );
        assert_eq!(
            COUNT.load(Ordering::Acquire),
            MAX_IN_FLIGHT_HANDSHAKES,
            "a shed attempt must not leak a slot"
        );

        // Dropping a held slot frees exactly one, which a fresh acquire can take.
        drop(held);
        assert_eq!(COUNT.load(Ordering::Acquire), 0);
        let slot = HandshakeSlot::try_acquire_on(&COUNT).expect("capacity freed");
        assert_eq!(COUNT.load(Ordering::Acquire), 1);
        drop(slot);
        assert_eq!(COUNT.load(Ordering::Acquire), 0);
    }

    /// KI-97 item 1: a peer that **trickles** must not outlive the handshake
    /// deadline. This is the case the socket's `SO_RCVTIMEO` cannot catch — every
    /// individual read succeeds well inside it, so the per-read bound restarts
    /// forever and a 4 KiB pre-auth frame could be dragged out for ~10 hours while
    /// holding a `HandshakeSlot`.
    ///
    /// The reader below is that attacker in miniature: one byte per call, always
    /// promptly, never an error. Against a bare `read_exact` it always wins;
    /// against `Deadline` it must be cut off with `TimedOut` having delivered only
    /// a fraction of what was asked for.
    #[test]
    fn a_trickling_peer_is_cut_off_at_the_handshake_deadline() {
        /// Always returns exactly one byte — the shape a per-read timeout can
        /// never reject.
        struct Trickle {
            served: usize,
        }
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_millis(1));
                self.served += 1;
                buf[0] = 0;
                Ok(1)
            }
        }

        let mut peer = Trickle { served: 0 };
        let budget = Duration::from_millis(50);
        let started = Instant::now();
        let mut guarded = Deadline::new(&mut peer, budget);

        // Ask for far more than the trickler will ever deliver in the budget.
        let mut buf = vec![0u8; MAX_HANDSHAKE_FRAME];
        let err = guarded
            .read_exact(&mut buf)
            .expect_err("a trickling peer must not satisfy read_exact past the deadline");
        assert_eq!(
            err.kind(),
            io::ErrorKind::TimedOut,
            "expected the deadline to fire, got: {err}"
        );

        let elapsed = started.elapsed();
        assert!(
            elapsed < budget * 10,
            "the deadline must cut the read off promptly, took {elapsed:?}"
        );
        // The point of the assertion: it was cut off mid-frame, not served.
        assert!(
            peer.served < MAX_HANDSHAKE_FRAME,
            "the trickler should never have completed the frame"
        );
    }

    /// The deadline must not interfere with a handshake that simply completes:
    /// reads and writes inside the budget pass through untouched.
    #[test]
    fn the_deadline_is_transparent_to_a_prompt_peer() {
        let mut peer = io::Cursor::new(b"brood".to_vec());
        let mut guarded = Deadline::new(&mut peer, Duration::from_secs(30));
        let mut buf = [0u8; 5];
        guarded.read_exact(&mut buf).expect("prompt read");
        assert_eq!(&buf, b"brood");

        let mut sink: Vec<u8> = Vec::new();
        let mut guarded = Deadline::new(&mut sink, Duration::from_secs(30));
        guarded.write_all(b"hi").expect("prompt write");
        guarded.flush().expect("prompt flush");
        assert_eq!(sink, b"hi");
    }

    /// A write is deadlined too. A peer that finishes its own side and then stops
    /// reading would otherwise park us in `write_all` against a full socket
    /// buffer, holding the same slot from the other direction.
    #[test]
    fn a_write_past_the_deadline_is_refused() {
        let mut sink: Vec<u8> = Vec::new();
        let mut guarded = Deadline::new(&mut sink, Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        let err = guarded
            .write_all(b"x")
            .expect_err("a write past the deadline must fail");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    /// Shedding at the cap is neither silent nor a flood vector: the first shed warns, an
    /// immediately following burst does not (rate-limited), and every shed is counted
    /// regardless.
    #[test]
    fn shedding_counts_every_connection_and_warns_at_most_once_per_interval() {
        assert_eq!(SHED_HANDSHAKES.load(Ordering::Relaxed), 0);
        note_shed_handshake();
        let after_first = LAST_SHED_WARN_MS.load(Ordering::Relaxed);
        // Not the sentinel any more ⇒ the first shed warned. Deliberately not
        // `!= 0`: `now_millis()` legitimately IS 0 in the first millisecond of
        // the process, which is exactly the case this sentinel exists for.
        assert_ne!(
            after_first, SHED_WARN_NEVER,
            "the first shed must warn, even at timestamp 0"
        );

        for _ in 0..50 {
            note_shed_handshake();
        }
        assert_eq!(
            SHED_HANDSHAKES.load(Ordering::Relaxed),
            51,
            "every shed must be counted, warned or not"
        );
        assert_eq!(
            LAST_SHED_WARN_MS.load(Ordering::Relaxed),
            after_first,
            "a burst must not re-warn inside the interval"
        );
    }

    /// A weak cookie is rejected *before* any identity/listener side effect
    /// (kernel audit guardrail): possession of the cookie is remote eval, and
    /// the HMAC itself accepts any key length — so `node_listen` is the gate.
    /// The runtime must remain a non-node afterwards so a corrected
    /// `node-start` can be retried.
    #[test]
    fn node_listen_rejects_a_short_cookie() {
        let name = crate::core::value::intern("weak@test");
        for weak in ["", "x", "hunter2", "123456789012345"] {
            let err = node_listen(name, "tcp:127.0.0.1:0", weak.to_string())
                .expect_err("a sub-16-byte cookie must be refused");
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(err.to_string().contains("cookie too short"), "got: {err}");
        }
        // No identity was published by the failed attempts.
        assert!(
            !crate::core::sync::read(&NODE).started,
            "must stay a non-node"
        );
    }

    /// A port-0 listener advertises the port the OS actually bound, and that address
    /// dials — peers learn it through the handshake and gossip.
    #[test]
    fn a_port_zero_listener_advertises_its_bound_port() {
        // The address list is process-wide and `cargo test` runs these tests as threads of
        // one process, so read what THIS listener recorded, not the list's first entry.
        let advertised = start_listener("tcp:127.0.0.1:0").expect("bind a port-0 listener");
        assert!(crate::core::sync::read(&LISTEN_ADDRS).contains(&advertised));
        let host_and_port = advertised
            .strip_prefix("tcp:127.0.0.1:")
            .unwrap_or_else(|| panic!("unexpected advertised address {advertised}"));
        let port: u16 = host_and_port.parse().expect("a numeric port");
        assert_ne!(
            port, 0,
            "advertised the requested port 0, not the bound one"
        );
        TcpStream::connect(("127.0.0.1", port)).expect("the advertised address dials");
    }

    /// A wildcard listener advertises the node's host, never `0.0.0.0` (which no peer
    /// on another machine can dial), with the bound port.
    #[test]
    fn a_wildcard_listener_advertises_the_node_host() {
        let advertised = start_listener("tcp:0.0.0.0:0").expect("bind a wildcard listener");
        assert!(crate::core::sync::read(&LISTEN_ADDRS).contains(&advertised));
        // The host is the node's (`localhost` unless a concurrent test started a node).
        let (host, port) = advertised
            .strip_prefix("tcp:")
            .and_then(|host_and_port| host_and_port.rsplit_once(':'))
            .unwrap_or_else(|| panic!("unexpected advertised address {advertised}"));
        assert_ne!(
            host, "0.0.0.0",
            "advertised the wildcard, which no peer can dial"
        );
        let port: u16 = port.parse().expect("a numeric port");
        assert_ne!(
            port, 0,
            "advertised the requested port 0, not the bound one"
        );
        TcpStream::connect((host, port)).expect("the advertised address dials");
    }
}

/// Robustness/fuzz surface: decode one length-prefixed wire frame from raw,
/// untrusted bytes under the pre-auth cap — must return Ok/Err, never panic
/// or over-allocate, on ANY input. Exercised by the `wire` fuzz target
/// (`crates/lisp/fuzz/fuzz_targets/wire.rs`).
#[doc(hidden)]
pub fn fuzz_decode_frame(bytes: &[u8]) {
    let mut r = std::io::Cursor::new(bytes);
    let _ = wire::read_frame_capped(&mut r, 1024 * 1024);
}
