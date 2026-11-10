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
    /// Whether the peer speaks the current protocol (v12), so a send in a lane goes as
    /// `FRAME_SEND_LANE` (ADR-492) and a record crosses (ADR-542). A v10 peer is sent every
    /// message as the plain `Send` it knows, and no record.
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

/// Source of per-connection generation ids.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

/// Connected peer node-name → its connection.
static NODES: LazyLock<RwLock<HashMap<Symbol, Conn>>> =
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

/// Node-name → pids that asked to watch it (`monitor-node`). Each gets a
/// `[:nodedown name]` message when a link to that node tears down.
static NODE_MONITORS: LazyLock<RwLock<HashMap<Symbol, Vec<u64>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The addresses this node listens on, in registration order (`"unix:PATH"` /
/// `"tcp:HOST:PORT"`). One entry per `node-start`/`node-also-listen` listener.
/// Read by [`advertised_addr`] to tell a peer how others should dial us.
static LISTEN_ADDRS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

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

// `Role` + the four-step `handshake` live in `dist::handshake`; only the link
// lifecycle uses them, and they keep the cookie/nonce/MAC plumbing self-
// contained.
use handshake::{Offer, Role, handshake};

pub(crate) mod guest;
mod handshake;
mod heartbeat;
mod session;
pub(crate) mod wire;

mod admission;
mod couplings;
mod dialing;
mod link_registration;
mod link_threads;
mod listener;
mod mesh;
mod names;
mod outbound;
mod routing;
mod transport;

use admission::*;
pub(crate) use couplings::*;
pub(crate) use dialing::*;
use link_registration::*;
use link_threads::*;
pub(crate) use listener::*;
use mesh::*;
pub(crate) use names::*;
pub(crate) use outbound::*;
pub(crate) use routing::*;
use transport::*;

use heartbeat::ensure_heartbeat;
use session::Session;
use wire::{Frame, encode_payload};

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests;

/// Robustness/fuzz surface: decode one length-prefixed wire frame from raw,
/// untrusted bytes under the pre-auth cap — must return Ok/Err, never panic
/// or over-allocate, on ANY input. Exercised by the `wire` fuzz target
/// (`crates/lisp/fuzz/fuzz_targets/wire.rs`).
#[doc(hidden)]
pub fn fuzz_decode_frame(bytes: &[u8]) {
    let mut r = std::io::Cursor::new(bytes);
    let _ = wire::read_frame_capped(&mut r, 1024 * 1024);
}
