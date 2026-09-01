//! Distributed nodes: connect two Brood runtimes and route messages between
//! them. Two nodes on one machine speak over a **Unix-domain socket** addressed
//! by name (no port); across machines, over **TCP**. The handshake, framing,
//! heartbeat and teardown are identical over both — only the carrier ([`Stream`])
//! differs (ADR-068). Erlang-style distribution falls out of share-nothing +
//! copy-on-send — *the network is just a longer copy* (ADR-013, `concurrency.md`).
//!
//! **Slice 1 (this module):** node naming, an authenticated TCP handshake (a
//! shared cookie, like Erlang's), and
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
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, LazyLock, RwLock};
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
    /// The address a *third* node should dial to reach this peer (`"unix:PATH"` /
    /// `"tcp:HOST:PORT"`, or empty if the peer didn't advertise one), learned from
    /// the peer's authenticated `Hello`. We gossip this to other peers so the
    /// cluster meshes (ADR-088).
    addr: String,
    /// The writer thread's inbox (length-framed bytes). **Bounded**
    /// (`WRITER_QUEUE_CAP`): see [`Conn::enqueue`] — a stalled peer can't balloon
    /// it into an OOM. Outbound frames carry an `Arc<[u8]>` so liveness probes
    /// (one `ping` per tick, one `pong` per inbound `Ping`) reuse a single buffer
    /// per link instead of cloning a `Vec<u8>` each time.
    tx: SyncSender<Arc<[u8]>>,
    /// A handle to the socket, for `shutdown` — the single teardown lever.
    sock: Arc<Stream>,
    /// Millis (on the `START` clock) of the last inbound frame. The heartbeat
    /// thread reads this to decide liveness; the reader writes it.
    last_seen: Arc<AtomicU64>,
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
    fn writer_handles(&self) -> (SyncSender<Arc<[u8]>>, Arc<Stream>) {
        (self.tx.clone(), self.sock.clone())
    }
}

/// Hand `bytes` to a link's writer thread, severing the link if its queue is full.
///
/// Free-standing so a caller can run it **without holding `NODES`**: the `Err` arm
/// issues a `shutdown` syscall, and that must not run under the `NODES` lock or it
/// delays every concurrent link registration and teardown for the syscall's
/// duration — the rule [`broadcast_peer_table`] documents and follows, which
/// `route` and `send_frame` used to violate by calling `Conn::enqueue` straight
/// out of a `read(&NODES).get(..)`.
fn enqueue_to(tx: &SyncSender<Arc<[u8]>>, sock: &Arc<Stream>, bytes: Arc<[u8]>) -> bool {
    match tx.try_send(bytes) {
        Ok(()) => true,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            false
        }
    }
}

/// Source of per-connection generation ids.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

/// Connected peer node-name → its connection.
static NODES: LazyLock<RwLock<HashMap<Symbol, Conn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Locally registered name → local process id, so a peer can address a process by
/// a stable name before anyone holds its pid (`(proc/register :echo (self))`).
static NAMES: LazyLock<RwLock<HashMap<Symbol, u64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

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
/// default — Erlang's behaviour), connecting to one node auto-connects you to
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

/// `(proc/register name pid)` — bind a local name to a local process id.
pub(crate) fn register(name: Symbol, id: u64) {
    crate::core::sync::write(&NAMES).insert(name, id);
}

/// `(proc/whereis name)` — the local pid registered under `name`, or `None`. Lets
/// callers test for an existing registration before re-`spawn`ing a server
/// they're about to register (idempotent bootstrap; used by `remote-spawn`).
pub(crate) fn whereis(name: Symbol) -> Option<u64> {
    crate::core::sync::read(&NAMES).get(&name).copied()
}

/// `(proc/unregister name)` — drop `name`'s binding, returning whether one existed.
///
/// The inverse of [`register`], which had none: a name could be bound to a pid and never
/// released except by that process dying. A service that wants to hand its name to a
/// replacement, or step down without exiting, needs this.
pub(crate) fn unregister(name: Symbol) -> bool {
    crate::core::sync::write(&NAMES).remove(&name).is_some()
}

/// The name `pid` is registered under, if any — the reverse of [`whereis`].
/// Used by the scheduler's death reporter to name a crashed process
/// (`process ticker (pid 6) died: …`) instead of only its opaque pid. O(n) over
/// the (small) `NAMES` table, and only on the cold death path, so the linear
/// scan is fine. Must be read *before* `unregister_dead_pid` clears the entry.
pub(crate) fn name_for_pid(pid: u64) -> Option<Symbol> {
    crate::core::sync::read(&NAMES)
        .iter()
        .find_map(|(&name, &p)| if p == pid { Some(name) } else { None })
}

/// Remove every `NAMES` entry pointing at `pid` — called from
/// `process::deregister` when a process dies, so a name registered under it
/// doesn't go stale. Without this, `(proc/whereis :foo)` could return a dead pid
/// and `(spawn :foo …)` (named-spawn) would mistake the stale entry for
/// "already running" and never re-spawn the worker. Erlang's `register`
/// semantics: a name lives only as long as its process does.
///
/// Also sweeps `NODE_MONITORS` so that dead pids don't accumulate as
/// permanent watcher entries: without this, every future `fire_nodedown`
/// for a peer the dead process watched would iterate and attempt delivery
/// to a non-existent pid.
pub(crate) fn unregister_dead_pid(pid: u64) {
    let mut names = crate::core::sync::write(&NAMES);
    names.retain(|_, &mut p| p != pid);
    // Prune the dead pid from every NODE_MONITORS watcher list.
    let mut monitors = crate::core::sync::write(&NODE_MONITORS);
    for watchers in monitors.values_mut() {
        watchers.retain(|&w| w != pid);
    }
    monitors.retain(|_, v| !v.is_empty());
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
/// each other. Lock-ordering vs `deregister` (which holds REGISTRY, then
/// NAMES, then MONITORS *sequentially*) is safe: deregister never holds
/// REGISTRY while reaching for NAMES, so the NAMES → REGISTRY nesting
/// here can't form a cycle.
///
/// `spawner` is **fallible** — if creating the process errors (e.g. a
/// type-check or heap-promotion failure), we propagate without inserting
/// into NAMES, so a failed spawn leaves no stale entry behind.
pub(crate) fn spawn_or_get<E>(
    name: Symbol,
    spawner: impl FnOnce() -> Result<u64, E>,
) -> Result<u64, E> {
    let mut names = crate::core::sync::write(&NAMES);
    if let Some(&existing) = names.get(&name) {
        if process::is_alive(existing) {
            return Ok(existing);
        }
        // Stale (the process registered under this name has died); drop and
        // fall through to a fresh spawn.
        names.remove(&name);
    }
    let pid = spawner()?;
    names.insert(name, pid);
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
fn send_frame(target_node: Symbol, frame: &Frame) -> bool {
    let bytes: Arc<[u8]> = match encode_payload(frame) {
        Ok(b) => Arc::from(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode control frame for {}: {}",
                value::symbol_name(target_node),
                e
            );
            return false;
        }
    };
    // Snapshot the writer handles, then release NODES before enqueueing — see
    // [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let handles = crate::core::sync::read(&NODES)
        .get(&target_node)
        .map(Conn::writer_handles);
    match handles {
        Some((tx, sock)) => enqueue_to(&tx, &sock, bytes),
        None => false,
    }
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
    // The pending entry can't be orphaned in either branch.
    process::record_pending_remote(target_node, target_pid, watcher_pid, mref);
    let sent = send_frame(
        target_node,
        &Frame::Monitor {
            watcher_pid,
            target: target_pid,
            mref,
        },
    );
    if !sent {
        process::drop_pending_remote(target_node, watcher_pid, mref);
        process::fire_noconnection(target_node, target_pid, watcher_pid, mref);
    }
}

/// `(demonitor mref)` for a monitor that was set up against a remote pid:
/// ship a `Frame::Demonitor` and forget the pending entry locally. Best
/// effort, like the local demonitor — the peer drops the matching watcher
/// from its `MONITORS` table.
pub(crate) fn demonitor_remote(target_node: Symbol, watcher_pid: u64, mref: u64) {
    process::drop_pending_remote(target_node, watcher_pid, mref);
    let _ = send_frame(target_node, &Frame::Demonitor { watcher_pid, mref });
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
        &Frame::Down {
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
    let sent = send_frame(
        target_node,
        &Frame::Link {
            from_pid: local_pid,
            to_pid: target_pid,
        },
    );
    if !sent {
        // No link to that node: the target is unreachable. Fire `:noconnection`
        // to the linker (this also drops the half-entry we just recorded).
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
        &Frame::Unlink {
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
        &Frame::Exit {
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
        &Frame::Exit {
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
/// already down and `[:nodedown]` is delivered immediately (Erlang's
/// `monitor_node` semantics).
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    let name = qualify_node_name(name);
    // Registration and the liveness check must be atomic w.r.t. `fire_nodedown`
    // (which reads NODE_MONITORS then delivers). Holding the write lock across
    // both prevents the race where fire_nodedown sees a new watcher AND our
    // own fallback also fires.
    //
    // Always register — monitor_node is persistent (Erlang semantics): fires on
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
/// `unregister_dead_pid` handles the death case automatically.
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

/// Connected peer node names (for `(nodes)`).
pub(crate) fn connected_nodes() -> Vec<Symbol> {
    crate::core::sync::read(&NODES).keys().copied().collect()
}

/// `(disconnect name)` — tear the link to peer `name` down now, *without* exiting
/// this process. Shuts the socket down (so the peer's reader hits EOF and fires
/// its own node-down) and runs `drop_link` on our side, which removes the `NODES`
/// entry and fires `[:nodedown name]` to our monitors. Same teardown the reader
/// takes on a clean peer exit, just triggered deliberately — Erlang's
/// `disconnect_node/1`. Returns `true` if a link existed, `false` if there was
/// nothing connected under `name`. Our own reader will also hit EOF and call
/// `drop_link(name, id)`, but the generation-id guard makes the second call a
/// no-op, so `[:nodedown]` fires exactly once.
pub(crate) fn disconnect(peer: Symbol) -> bool {
    let conn = crate::core::sync::read(&NODES)
        .get(&peer)
        .map(|c| (Arc::clone(&c.sock), c.id));
    match conn {
        Some((sock, id)) => {
            let _ = sock.shutdown(Shutdown::Both);
            drop_link(peer, id);
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

/// Deliver `msg` to `target` on `node`, location-transparently: a local node
/// delivers in-process; a remote one forwards over the link. Unknown name,
/// unknown/disconnected node, or a dead pid is a silent no-op (Erlang semantics).
///
/// Returns whether a route existed at all: `true` for any local target (even a
/// dead pid — process liveness stays Erlang-silent) and for a remote node with
/// a live link; `false` when the node is **unknown/disconnected** (the message
/// was dropped on the floor). `send` surfaces that as a catchable
/// `:noconnection` error when the sending process opted in via
/// `(proc/flag :send-errors true)` — so a caller can queue-and-retry instead
/// of silently losing messages until the reconnect (the dist self-healing
/// seam); every other caller ignores it.
/// Warn, once per name, that a message addressed to a **registered name no process holds**
/// was dropped.
///
/// The drop itself is correct and stays correct: Erlang semantics, and legitimate — the name
/// may simply be gone, or never have existed. What this fixes is the **silence**. `send` is
/// fire-and-forget, so the sender has already moved on and cannot be told; the receiving node
/// is the only party that knows the message died, and until now it said nothing to anyone.
///
/// That silence is not hypothetical: it cost **KI-36** twelve days and three sightings. A test
/// node opened its dist listener before registering the name its peer would immediately address,
/// so the peer's message landed in this exact hole — and the only symptom available was a 20 s
/// deadline expiring with no information anywhere. One line here would have named it instantly.
///
/// **Default-on, deliberately.** A flag-gated trace is worth nothing for this class, because
/// nobody arms a flag before the bug they have not diagnosed yet — the lesson of KI-39, whose
/// retroactive self-reporting also failed to fire. Silence it with `BROOD_NO_DROP_WARN=1`.
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

pub(crate) fn route(node: Symbol, target: Target, msg: Message) -> bool {
    if is_local(node) {
        let id = match target {
            Target::Pid(id) => id,
            Target::Name(name) => match crate::core::sync::read(&NAMES).get(&name).copied() {
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
        process::deliver(id, msg);
        return true;
    }
    // Remote: encode a Send frame and hand it to the peer's writer thread.
    let bytes: Arc<[u8]> = match encode_payload(&Frame::Send { target, msg }) {
        Ok(b) => Arc::from(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode message for {}: {}",
                value::symbol_name(node),
                e
            );
            return true; // a link exists; the payload was the problem
        }
    };
    // Snapshot the writer handles, then release NODES before enqueueing — see
    // [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let handles = crate::core::sync::read(&NODES)
        .get(&node)
        .map(Conn::writer_handles);
    match handles {
        Some((tx, sock)) => {
            let _ = enqueue_to(&tx, &sock, bytes); // severs the link if the writer is gone/stalled
            true
        }
        None => false,
    }
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
    // Guardrail (kernel audit 2026-06-03): the cookie is the *entire* trust
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
    start_listener(addr)
}

/// Bind one listener for `addr` and spawn its accept loop. Identity-agnostic — the
/// per-connection handshake reads `NODE` at accept time — so it serves both the
/// first listener (`node_listen`) and any added later (`node_also_listen`).
fn start_listener(addr: &str) -> io::Result<()> {
    if let Some(path) = addr.strip_prefix("unix:") {
        bind_unix_listener(path)?;
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        let listener = TcpListener::bind(hostport)?;
        spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Tcp(s)));
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ));
    }
    // Record only after a successful bind, so a failed listener doesn't leave a
    // dead address we'd advertise to peers.
    crate::core::sync::write(&LISTEN_ADDRS).push(addr.to_string());
    Ok(())
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
/// socket is the whole response. What was wrong was the **silence**: hitting the
/// cap means *every further inbound link is refused*, i.e. inbound distribution
/// is effectively down, and the node said nothing to anyone. That is the KI-36
/// lesson (a silent drop cost twelve days) applied to the one place left that
/// still dropped without a word.
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
struct HandshakeSlot;
impl HandshakeSlot {
    /// Take a slot, or `None` if the cap is already reached (caller sheds the
    /// connection). The over-count from a losing `fetch_add` is immediately
    /// rolled back, so the gate can't drift above the cap under contention.
    fn try_acquire() -> Option<Self> {
        if IN_FLIGHT_HANDSHAKES.fetch_add(1, Ordering::AcqRel) >= MAX_IN_FLIGHT_HANDSHAKES {
            IN_FLIGHT_HANDSHAKES.fetch_sub(1, Ordering::AcqRel);
            None
        } else {
            Some(HandshakeSlot)
        }
    }
}
impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        IN_FLIGHT_HANDSHAKES.fetch_sub(1, Ordering::AcqRel);
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
    spawn_bg("dist-acceptor", move || loop {
        match accept() {
            Ok(stream) => {
                // Shed past the in-flight-handshake cap *before* spawning a thread
                // or reading a byte, so a flood of unauthenticated connections
                // can't exhaust threads/memory. Closing the socket is the whole
                // response — but it is no longer a *silent* one: reaching the cap
                // means inbound links are being refused outright, which the node
                // now reports (rate-limited — see `note_shed_handshake`).
                let permit = match HandshakeSlot::try_acquire() {
                    Some(p) => p,
                    None => {
                        note_shed_handshake();
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                };
                // `spawn_bg`, not `thread::spawn`: a refused thread here used to panic
                // *inside the accept loop*, unwinding the acceptor and closing the
                // listener permanently. Now the connection is simply refused and the
                // node keeps accepting.
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
use handshake::{handshake, Role};

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
    // Pre-dial de-dup: if we already have a link to the named node, reuse it
    // without dialing. The caller may supply a stale/wrong symbol (e.g. from
    // gossip lag), so we do a second check with the *authenticated* name after
    // the handshake too.
    if crate::core::sync::read(&NODES).contains_key(&peer) {
        return Ok(peer);
    }
    let mut stream = dial(addr)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // Deadlined like the accept side: a malicious or wedged *listener* can
    // trickle at a dialer just as easily, and this call is made from a scheduler
    // worker, so an unbounded hold here wedges a worker rather than a slot.
    let (peer, peer_addr, session) = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Initiator)?
    };
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
                                    // Always pass to `establish` — even when we already have a link under the
                                    // authenticated name. `establish` has its own symmetric tie-break (both sides
                                    // compare connectors by name and reach the same decision). The losing side
                                    // closes its own socket and returns; the winning side replaces the link. A
                                    // short-circuit `stream.shutdown` here would skip the tie-break on our end
                                    // while the peer still runs `establish` on theirs — they might win, register
                                    // our doomed socket, and later fire a spurious `[:nodedown]` when the reader
                                    // hits the EOF our shutdown sent.
    establish(peer, peer_addr, stream, Role::Initiator, session);
    Ok(peer)
}

/// Bound on a **name resolution**, which `connect_timeout` does not cover.
///
/// `to_socket_addrs` is a blocking libc call with no timeout of its own, and it runs on
/// whichever thread dialled — for `node/connect` that is a scheduler worker, which the
/// scheduler cannot preempt while it sits in a syscall (ADR-059). An unreachable DNS
/// server takes the resolver's own timeout (commonly tens of seconds, and longer with
/// retries across several `nameserver` lines), so a few reconnect attempts against a bad
/// name were enough to wedge a meaningful slice of the ~nproc pool. KI-97 item 2.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve `hostport` with a wall-clock bound, off the calling thread.
///
/// The resolve happens on a throwaway thread and the caller waits on a channel, so a
/// hung resolver costs *that* thread rather than the worker we were called on. The
/// thread is deliberately left to finish on its own if we time out: there is no way to
/// cancel a blocking `getaddrinfo`, and detaching it is what keeps the caller bounded.
/// That is safe because it touches nothing after the send — a closed channel is simply
/// a dropped result — and the rate is bounded by the caller (a user-initiated
/// `node/connect`, or `reconnect/watch`'s exponential backoff), not by inbound traffic.
fn resolve_timeout(hostport: &str) -> io::Result<Vec<std::net::SocketAddr>> {
    // A literal `IP:port` needs no resolver at all — `to_socket_addrs` would parse it
    // without touching DNS. Answer it inline: that is the overwhelmingly common address
    // (every `127.0.0.1:port` in the suite, and most real deployments), and taking the
    // fast path keeps a thread spawn off the dial path entirely.
    if let Ok(sa) = hostport.parse::<std::net::SocketAddr>() {
        return Ok(vec![sa]);
    }
    let (tx, rx) = mpsc::channel();
    let owned = hostport.to_string();
    if std::thread::Builder::new()
        .name("dist-resolve".into())
        .spawn(move || {
            let _ = tx.send(owned.to_socket_addrs().map(|it| it.collect::<Vec<_>>()));
        })
        .is_err()
    {
        // Out of threads (EAGAIN under load). Falling back to an inline resolve is
        // strictly better than failing the dial: it is exactly the old behaviour, so
        // connectivity is preserved, and the unbounded wait it risks is the lesser of
        // the two — refusing to connect because the machine is briefly thread-starved
        // would be a new failure mode introduced by a hardening change.
        return hostport.to_socket_addrs().map(|it| it.collect());
    }
    match rx.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(res) => res,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("resolving {hostport} took longer than {RESOLVE_TIMEOUT:?}"),
        )),
        // The resolver thread vanished without answering (a panic in getaddrinfo's
        // wrapper, say). Report it rather than hanging on a channel nobody will send to.
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(format!(
            "resolver thread for {hostport} ended without a result"
        ))),
    }
}

/// Open the carrier for `addr`. Unix connects are local and effectively instant
/// (or refuse immediately); TCP bounds **both** halves — the name resolution via
/// [`resolve_timeout`] and then `connect_timeout` per resolved address — so neither a
/// wedged resolver nor a silently-dropping peer can pin the dialing thread.
fn dial(addr: &str) -> io::Result<Stream> {
    if let Some(path) = addr.strip_prefix("unix:") {
        dial_unix(path)
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        // `connect_timeout` requires a `SocketAddr`, so resolve here and try each
        // address in turn — same multi-A-record behaviour as `TcpStream::connect`
        // while bounding the wait per attempt.
        let mut last_err: Option<io::Error> = None;
        let stream =
            resolve_timeout(hostport)?
                .into_iter()
                .find_map(
                    |sa| match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            last_err = Some(e);
                            None
                        }
                    },
                );
        Ok(Stream::Tcp(stream.ok_or_else(|| {
            last_err.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "no addresses resolved")
            })
        })?))
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
    let (peer, peer_addr, session) = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Responder)?
    };
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
    establish(peer, peer_addr, stream, Role::Responder, session);
    Ok(())
}

/// Register the authenticated link and spawn its reader + writer threads —
/// resolving a duplicate against any existing link to the same peer first.
/// `peer_addr` is how a third node should dial this peer (for mesh gossip).
fn establish(peer: Symbol, peer_addr: String, stream: Stream, role: Role, session: Session) {
    // Who initiated *this* connection (the tie-break key).
    let connector = match role {
        Role::Initiator => local_node(),
        Role::Responder => peer,
    };
    let sock = Arc::new(stream);
    let (tx, rx) = mpsc::sync_channel::<Arc<[u8]>>(WRITER_QUEUE_CAP);
    let last_seen = Arc::new(AtomicU64::new(now_millis()));
    let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);

    // Decide winner vs. any existing link, and register atomically under the lock.
    // Compare connectors by *name* (spelling) — interned ids differ per process,
    // but both ends share the names, so they pick the same physical link.
    // `was_new` distinguishes a brand-new peer (gossip the cluster about it) from
    // a reconnect/duplicate-replacement (peers already know this name). Assigned
    // on the registering path; the losing path diverges (`return`).
    let was_new;
    let evicted: Option<Conn> = {
        let mut nodes = crate::core::sync::write(&NODES);
        match nodes.get(&peer) {
            Some(existing)
                if value::symbol_name(connector) >= value::symbol_name(existing.connector) =>
            {
                // The existing link wins (its connector sorts first, or it's the
                // same initiator = a plain duplicate). We lose: close our socket
                // and don't register or spawn.
                let _ = sock.shutdown(Shutdown::Both);
                return;
            }
            other => {
                // We win (or there was no existing link). Take over the slot; any
                // evicted link is torn down below, outside the lock.
                was_new = other.is_none();
                let old = nodes.remove(&peer);
                nodes.insert(
                    peer,
                    Conn {
                        id,
                        connector,
                        addr: peer_addr,
                        tx: tx.clone(),
                        sock: Arc::clone(&sock),
                        last_seen: Arc::clone(&last_seen),
                    },
                );
                old
            }
        }
    };
    if let Some(old) = evicted {
        let _ = old.sock.shutdown(Shutdown::Both); // its reader unblocks, no-ops on the new id
    }

    ensure_heartbeat();

    // The link is authenticated and registered: split the session into its two
    // directional ciphers (ADR-089). The writer owns the send cipher, the reader
    // the receive cipher — neither shares crypto state, which is exactly why this
    // per-direction-AEAD scheme fits the reader/writer thread split (a single TLS
    // connection couldn't be driven from both). A tie-break loser returned above,
    // dropping `session` unused.
    let Session {
        send: mut seal,
        recv: open,
    } = session;

    // Writer: pull each plaintext frame payload off the channel, **seal** it, and
    // write the ciphertext. A per-write timeout (`WRITE_TIMEOUT`) prevents a
    // slowloris peer from pinning the writer and ballooning `rx`; a timeout or a
    // seal failure is treated like any I/O error — fall through to shutdown.
    let writer_sock = Arc::clone(&sock);
    if let Err(e) = writer_sock.set_write_timeout(Some(WRITE_TIMEOUT)) {
        eprintln!(
            "dist: warning: could not set write timeout on link to {}: {e}",
            value::symbol_name(peer)
        );
    }
    spawn_bg("dist-writer", move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            for payload in rx {
                match seal.seal(&payload) {
                    Ok(framed) if (&*writer_sock).write_all(&framed).is_ok() => {}
                    _ => {
                        let _ = writer_sock.shutdown(Shutdown::Both);
                        break;
                    }
                }
            }
        }));
    });

    // Reader: every inbound frame refreshes liveness; a `Ping` is answered with a
    // `Pong`. On EOF/error (incl. a `shutdown` from the writer or the heartbeat)
    // it runs `drop_link`, which removes the entry iff it's still this generation.
    let reader_sock = Arc::clone(&sock);
    let reader_tx = tx;
    // One shared Pong buffer per reader; sending is an `Arc::clone` (atomic
    // incr), not a `Vec` copy.
    let pong: Arc<[u8]> = Arc::from(encode_payload(&Frame::Pong).expect("encode Pong"));
    spawn_bg("dist-reader", move || {
        let mut r: &Stream = &reader_sock;
        // The receive cipher, owned solely by this reader (no lock needed). Each
        // `open` authenticates + decrypts one frame; a tag failure (a tampered,
        // forged, replayed, or reordered frame) ends the loop and tears the link
        // down — closing ADR-081's post-handshake injection hole.
        let mut open = open;
        // Loop until peer closes, protocol error, or a deliberate `shutdown`.
        // `peer` is the *authenticated* node name from the handshake — every
        // process-coupling frame (Monitor/Demonitor/Link/Unlink/Exit) is keyed
        // to it, never to wire-supplied data, so a malicious peer can't claim to
        // be node X and inject `[:down …]` / link-exit deliveries to processes
        // coupled with X. (These frames carry no `from_node` field at all — see
        // the security note on the `Frame` enum.)
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(frame) = open.open(&mut r) {
                last_seen.store(now_millis(), Ordering::Release);
                match frame {
                    Frame::Send { target, msg } => deliver_inbound(target, msg),
                    Frame::Ping => {
                        // Bounded queue: if we can't even enqueue a Pong, the writer
                        // is stalled (peer not draining) or gone — sever and let
                        // `drop_link` below deregister, rather than buffer.
                        if reader_tx.try_send(Arc::clone(&pong)).is_err() {
                            let _ = reader_sock.shutdown(Shutdown::Both);
                            break;
                        }
                    }
                    // A peer asked to watch one of our local pids — re-use the
                    // shared `add_monitor` core with a `Watcher::Remote` so the
                    // alive-target / dead-target paths are exactly the local
                    // monitor's, just with a different delivery channel.
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
                    // Peer dropped a remote monitor — same `drop_monitor` the
                    // local `demonitor` uses, with a predicate matching the
                    // Remote variant identity (node + pid + mref).
                    Frame::Demonitor { watcher_pid, mref } => process::drop_monitor(|w| {
                        matches!(*w, process::Watcher::Remote { node, pid, mref: r }
                                     if node == peer && pid == watcher_pid && r == mref)
                    }),
                    // A monitor we asked the peer to keep just fired. Retire our
                    // `PENDING_REMOTE` entry (the one-shot has delivered — a later
                    // node-down must not fire it again, KI-96), then deliver the
                    // `[:down …]`. The dying pid is node-qualified by the
                    // authenticated `peer`, never by wire data.
                    Frame::Down {
                        watcher_pid,
                        mref,
                        target_pid,
                        reason,
                    } => process::deliver_remote_down(peer, watcher_pid, mref, target_pid, reason),
                    // A peer linked its `from_pid` to our local `to_pid` — record
                    // our half (keyed by the trusted connection `peer`, not the
                    // wire's `from_node`, same as the monitor handlers).
                    Frame::Link { from_pid, to_pid } => {
                        // `to_pid` is wire data: the peer may name a pid that is dead or
                        // never existed. Recording that would leak an entry forever (see
                        // `record_remote_link`), so on a dead target we tell the peer
                        // instead — its linked process gets `:noproc`, exactly what a
                        // LOCAL `link` to a dead pid delivers.
                        if !process::record_remote_link(to_pid, peer, from_pid) {
                            send_link_exit(
                                peer,
                                from_pid,
                                to_pid,
                                Message::Keyword(value::intern(pk::NOPROC)),
                            );
                        }
                    }
                    Frame::Unlink { from_pid, to_pid } => {
                        process::drop_remote_link(to_pid, peer, from_pid)
                    }
                    // An exit signal for our local `to_pid`. A link death goes
                    // through the trap-or-propagate path; an explicit remote exit
                    // is routed straight to `scheduler::exit` (kill-style).
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
                    // Cluster-mesh gossip: the peer is telling us about other
                    // nodes it knows. Dial any we're not already connected to,
                    // so connecting to one member joins the whole mesh (ADR-088).
                    Frame::Peers { peers } => mesh_consider(peers),
                    // Handshake-only frames in steady state: a peer that
                    // re-sends one after the link is up is malformed but harmless
                    // — keep reading.
                    Frame::Pong | Frame::Hello { .. } | Frame::Auth { .. } => {}
                }
            }
        }));
        drop_link(peer, id);
    });

    // A brand-new peer just joined: tell the cluster. Send the new peer our other
    // peers (so it dials them) and tell our existing peers about the newcomer (so
    // they dial it). Both directions fall out of one broadcast (ADR-088). Skipped
    // for a reconnect/duplicate (the name was already known cluster-wide) and when
    // meshing is disabled.
    if was_new && mesh_enabled() {
        broadcast_peer_table();
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
        tx: SyncSender<Arc<[u8]>>,
        sock: Arc<Stream>,
    }
    let snaps: Vec<PeerSnap> = {
        let nodes = crate::core::sync::read(&NODES);
        nodes
            .iter()
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
        if let Ok(bytes) = encode_payload(&Frame::Peers { peers }) {
            if s.tx.try_send(Arc::from(bytes)).is_err() {
                let _ = s.sock.shutdown(Shutdown::Both);
            }
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
    let connected: HashSet<Symbol> = {
        let nodes = crate::core::sync::read(&NODES);
        nodes.keys().copied().collect()
    };
    let mut to_dial: Vec<(Symbol, String)> = Vec::new();
    {
        let mut pending = crate::core::sync::write(&PENDING_DIALS);
        for (name, addr) in peers {
            if name == me || addr.is_empty() || connected.contains(&name) || pending.contains(&name)
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
    let removed = {
        let mut nodes = crate::core::sync::write(&NODES);
        match nodes.get(&peer) {
            Some(c) if c.id == id => {
                nodes.remove(&peer);
                true
            }
            _ => false,
        }
    };
    if removed {
        fire_nodedown(peer);
    }
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
    process::handle_node_down(peer);
    // Cross-node links over the dropped link fire `:noconnection` to their local
    // peers (ADR-067), mirroring the monitor `:noconnection`-on-net-split above.
    process::handle_link_node_down(peer);
}

/// An inbound `Send` from a peer: resolve the target locally and deliver.
fn deliver_inbound(target: Target, msg: Message) {
    let id = match target {
        Target::Pid(id) => id,
        Target::Name(name) => match crate::core::sync::read(&NAMES).get(&name).copied() {
            Some(id) => id,
            None => {
                warn_dropped_to_unregistered_name(name, "inbound");
                return;
            }
        },
    };
    process::deliver(id, msg);
}

mod handshake;
mod heartbeat;
mod session;
pub(crate) mod wire;

use heartbeat::ensure_heartbeat;
use session::Session;
use wire::{encode_payload, Frame};

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    /// The pre-auth connection gate: slots are bounded at the cap, the
    /// over-count from a losing `try_acquire` is rolled back (so the live count
    /// never drifts above the cap), and a dropped slot frees capacity again.
    /// Under nextest each test runs in its own process, so the global counter
    /// starts clean at 0.
    #[test]
    fn handshake_slot_caps_in_flight_and_releases_on_drop() {
        // Fill every slot.
        let held: Vec<HandshakeSlot> = (0..MAX_IN_FLIGHT_HANDSHAKES)
            .map(|_| HandshakeSlot::try_acquire().expect("under the cap"))
            .collect();
        assert_eq!(
            IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire),
            MAX_IN_FLIGHT_HANDSHAKES
        );

        // One past the cap is shed, and the failed attempt rolled its count back.
        assert!(HandshakeSlot::try_acquire().is_none(), "cap must shed");
        assert_eq!(
            IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire),
            MAX_IN_FLIGHT_HANDSHAKES,
            "a shed attempt must not leak a slot"
        );

        // Dropping a held slot frees exactly one, which a fresh acquire can take.
        drop(held);
        assert_eq!(IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire), 0);
        let s = HandshakeSlot::try_acquire().expect("capacity freed");
        assert_eq!(IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire), 1);
        drop(s);
        assert_eq!(IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire), 0);
    }

    /// KI-97 item 2: a name resolution is bounded, and the bound is enforced off the
    /// calling thread.
    ///
    /// `connect_timeout` covers the connect but not the lookup, and `to_socket_addrs` is a
    /// blocking libc call with no timeout — on a scheduler worker, which cannot be
    /// preempted mid-syscall (ADR-059), an unreachable DNS server pinned a worker for the
    /// resolver's own timeout. There is no way to make a real resolver hang on demand
    /// here, so this asserts the two properties that matter and can be checked: a normal
    /// name still resolves, and a bad one fails rather than hanging.
    #[test]
    fn resolving_is_bounded_and_still_works() {
        let started = Instant::now();
        let ok = resolve_timeout("127.0.0.1:9").expect("a literal address must resolve");
        assert!(!ok.is_empty(), "expected at least one address");
        assert!(
            started.elapsed() < RESOLVE_TIMEOUT,
            "a literal address must not go anywhere near the bound"
        );

        // A syntactically valid but unresolvable name: the point is that it RETURNS.
        let started = Instant::now();
        let bad = resolve_timeout("invalid.invalid.:9");
        assert!(
            bad.is_err(),
            "an unresolvable name must be an error, not a hang"
        );
        assert!(
            started.elapsed() < RESOLVE_TIMEOUT * 4,
            "resolution must be bounded, took {:?}",
            started.elapsed()
        );
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

    /// Shedding at the cap is no longer silent, and no longer a flood vector
    /// either: the first shed warns, an immediately following burst does not
    /// (rate-limited), and every shed is counted regardless.
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
