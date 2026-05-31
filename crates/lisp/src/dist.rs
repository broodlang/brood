//! Distributed nodes: connect two Brood runtimes and route messages between
//! them. Two nodes on one machine speak over a **Unix-domain socket** addressed
//! by name (no port); across machines, over **TCP**. The handshake, framing,
//! heartbeat and teardown are identical over both — only the carrier ([`Stream`])
//! differs (ADR-068). Erlang-style distribution falls out of share-nothing +
//! copy-on-send — *the network is just a longer copy* (ADR-013, `concurrency.md`).
//!
//! **Slice 1 (this module):** node naming, an authenticated TCP handshake (a
//! shared cookie, like Erlang's — *not* real security yet), and
//! location-transparent [`send`](crate::process::send) to a remote process. A
//! process is addressed either by a [`Value::Pid`](crate::core::value::Value::Pid)
//! — which carries node identity, so the same value works locally or across the
//! link — or, to bootstrap before you hold a peer's pid, by a `{:name :node}`
//! registered-name address.
//!
//! **One node per OS process.** The node identity, connection table, name table
//! and symbol interner are process-global, so a "node" *is* the OS process; two
//! nodes are two `brood` processes (typically over loopback). Deferred to later
//! slices: remote `spawn`/code shipping, distributed monitors, node-down
//! detection, reconnect, and real auth/TLS.
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

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

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

/// Bound the read-side of a handshake so a peer that connects and then stalls
/// can't pin a thread forever (the steady-state reader has the timeout cleared —
/// it *should* block until the next message arrives).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout on dialer socket connect. Without this, `TcpStream::connect(addr)` blocks
/// at the kernel's TCP SYN timeout (minutes on Linux) when the peer's port is
/// silently dropping packets — fine for a healthy LAN, but on a flaky network the
/// dialer wedges. Several seconds is enough for a real LAN/WAN round-trip.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-write timeout on the steady-state writer socket. A peer that stops reading
/// (slowloris-style) drives its TCP receive window to zero; without this, our
/// `write_all` blocks forever, the writer thread is pinned, and the unbounded
/// `mpsc::channel` accumulates messages — a remote-controlled OOM. Generous so a
/// genuinely slow peer doesn't get torn down for an occasional slow drain.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

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
    Unix(UnixStream),
}

impl Stream {
    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.shutdown(how),
            Stream::Unix(s) => s.shutdown(how),
        }
    }
    fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_read_timeout(d),
            Stream::Unix(s) => s.set_read_timeout(d),
        }
    }
    fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_write_timeout(d),
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
            Stream::Unix(s) => s.read(buf),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            Stream::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
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
            Stream::Unix(ref s) => (&*s).read(buf),
        }
    }
}
impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match **self {
            Stream::Tcp(ref s) => (&*s).write(buf),
            Stream::Unix(ref s) => (&*s).write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match **self {
            Stream::Tcp(ref s) => (&*s).flush(),
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
    /// The writer thread's inbox (length-framed bytes).
    /// Outbound frames carry an `Arc<[u8]>` so liveness probes (one `ping` per
    /// tick, one `pong` per inbound `Ping`) reuse a single buffer per link
    /// instead of cloning a `Vec<u8>` each time.
    tx: Sender<Arc<[u8]>>,
    /// A handle to the socket, for `shutdown` — the single teardown lever.
    sock: Arc<Stream>,
    /// Millis (on the `START` clock) of the last inbound frame. The heartbeat
    /// thread reads this to decide liveness; the reader writes it.
    last_seen: Arc<AtomicU64>,
}

/// Source of per-connection generation ids.
static NEXT_LINK: AtomicU64 = AtomicU64::new(0);

/// Connected peer node-name → its connection.
static NODES: LazyLock<RwLock<HashMap<Symbol, Conn>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Locally registered name → local process id, so a peer can address a process by
/// a stable name before anyone holds its pid (`(register :echo (self))`).
static NAMES: LazyLock<RwLock<HashMap<Symbol, u64>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Node-name → pids that asked to watch it (`monitor-node`). Each gets a
/// `[:nodedown name]` message when a link to that node tears down.
static NODE_MONITORS: LazyLock<RwLock<HashMap<Symbol, Vec<u64>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// `(register name pid)` — bind a local name to a local process id.
pub(crate) fn register(name: Symbol, id: u64) {
    crate::core::sync::write(&NAMES).insert(name, id);
}

/// `(whereis name)` — the local pid registered under `name`, or `None`. Lets
/// callers test for an existing registration before re-`spawn`ing a server
/// they're about to register (idempotent bootstrap; used by `remote-spawn`).
pub(crate) fn whereis(name: Symbol) -> Option<u64> {
    crate::core::sync::read(&NAMES).get(&name).copied()
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
/// doesn't go stale. Without this, `(whereis :foo)` could return a dead pid
/// and `(spawn :foo …)` (named-spawn) would mistake the stale entry for
/// "already running" and never re-spawn the worker. Erlang's `register`
/// semantics: a name lives only as long as its process does.
pub(crate) fn unregister_dead_pid(pid: u64) {
    let mut names = crate::core::sync::write(&NAMES);
    names.retain(|_, &mut p| p != pid);
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

/// `(monitor (Pid remote_node remote_pid))` from the cross-node path: ship a
/// `Frame::Monitor` to the peer and record the pending remote watcher locally
/// (so net-split can fire `:noconnection` to the watcher even though the
/// monitor target lives elsewhere). If the peer link isn't up, deliver
/// `:noconnection` immediately — same shape an immediately-dead local target
/// gets (`:noproc` from `add_monitor`), just a different reason.
pub(crate) fn monitor_remote(target_node: Symbol, target_pid: u64, watcher_pid: u64, mref: u64) {
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Monitor {
        from_node: me,
        watcher_pid,
        target: target_pid,
        mref,
    }) {
        Ok(b) => Arc::from(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode Monitor for {}: {}",
                value::symbol_name(target_node),
                e
            );
            return;
        }
    };
    // Record the pending entry **before** consulting `NODES`, then take a
    // single `NODES` read lock that covers both the link presence check and
    // the channel send. This closes a race against `drop_link`/`handle_node_down`:
    //   • If we record before they run, they'll find our entry in
    //     `PENDING_REMOTE` and fire `:noconnection` to us — even if our send
    //     never made it onto the wire.
    //   • If they run first (`NODES` already empty here), we fall through to
    //     the explicit cleanup below, dropping our pending entry and firing
    //     `:noconnection` ourselves.
    // The pending entry can't be orphaned in either branch.
    process::record_pending_remote(target_node, target_pid, watcher_pid, mref);
    let sent = {
        let nodes = crate::core::sync::read(&NODES);
        match nodes.get(&target_node) {
            Some(conn) => {
                let _ = conn.tx.send(bytes);
                true
            }
            None => false,
        }
    };
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
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Demonitor {
        from_node: me,
        watcher_pid,
        mref,
    }) {
        Ok(b) => Arc::from(b),
        Err(_) => return, // best-effort
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&target_node) {
        let _ = conn.tx.send(bytes);
    }
}

// ---- cross-node links (ADR-067) — the symmetric cousin of monitor_remote ----

/// `(link remote-pid)`: record our half of the link, ship a `Frame::Link` so the
/// peer records its half, and — if the link to that node isn't up — fire an
/// immediate `:noconnection` to the local linker (same shape a monitor's
/// unreachable target gets). `local_pid` is the linker (self). Race-free against
/// net-split exactly as `monitor_remote`: record before consulting `NODES`.
pub(crate) fn link_remote(target_node: Symbol, target_pid: u64, local_pid: u64) {
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Link {
        from_node: me,
        from_pid: local_pid,
        to_pid: target_pid,
    }) {
        Ok(b) => Arc::from(b),
        Err(_) => return,
    };
    process::record_remote_link(local_pid, target_node, target_pid);
    let sent = {
        let nodes = crate::core::sync::read(&NODES);
        match nodes.get(&target_node) {
            Some(conn) => {
                let _ = conn.tx.send(bytes);
                true
            }
            None => false,
        }
    };
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
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Unlink {
        from_node: me,
        from_pid: local_pid,
        to_pid: target_pid,
    }) {
        Ok(b) => Arc::from(b),
        Err(_) => return,
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&target_node) {
        let _ = conn.tx.send(bytes);
    }
}

/// A local linked process `from_pid` died with `reason`: ship a link
/// `Frame::Exit` to its remote peer `target_pid` on `target_node`. Best-effort —
/// if the link is down the peer already learns via its own net-split handling.
/// Called from `links::notify_peers`.
pub(crate) fn send_link_exit(target_node: Symbol, target_pid: u64, from_pid: u64, reason: Message) {
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Exit {
        from_node: me,
        from_pid,
        to_pid: target_pid,
        reason,
        link: true,
    }) {
        Ok(b) => Arc::from(b),
        Err(_) => return,
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&target_node) {
        let _ = conn.tx.send(bytes);
    }
}

/// `(exit remote-pid reason)`: ship a non-link `Frame::Exit` routed straight to
/// the peer's `scheduler::exit` (kill-style, like the local builtin). Used for an
/// explicit remote exit and for a supervisor terminating a remote child.
pub(crate) fn exit_remote(target_node: Symbol, target_pid: u64, reason: Message) {
    let me = local_node();
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Exit {
        from_node: me,
        from_pid: 0, // unused for an explicit (non-link) exit
        to_pid: target_pid,
        reason,
        link: false,
    }) {
        Ok(b) => Arc::from(b),
        Err(_) => return,
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&target_node) {
        let _ = conn.tx.send(bytes);
    }
}

/// `(monitor-node name pid)` — deliver `[:nodedown name]` to `pid` when a link to
/// `name` goes down. Persistent (fires on each down) until the process exits.
/// If `name` isn't us and there's no current link, the node is effectively
/// already down and `[:nodedown]` is delivered immediately (Erlang's
/// `monitor_node` semantics).
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    crate::core::sync::write(&NODE_MONITORS)
        .entry(name)
        .or_default()
        .push(pid);
    if !is_local(name) && !crate::core::sync::read(&NODES).contains_key(&name) {
        process::deliver(pid, nodedown_msg(name));
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
pub(crate) fn route(node: Symbol, target: Target, msg: Message) {
    if is_local(node) {
        let id = match target {
            Target::Pid(id) => id,
            Target::Name(name) => match crate::core::sync::read(&NAMES).get(&name).copied() {
                Some(id) => id,
                None => return,
            },
        };
        process::deliver(id, msg);
        return;
    }
    // Remote: encode a Send frame and hand it to the peer's writer thread.
    let bytes: Arc<[u8]> = match frame_bytes(&Frame::Send { target, msg }) {
        Ok(b) => Arc::from(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode message for {}: {}",
                value::symbol_name(node),
                e
            );
            return;
        }
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&node) {
        let _ = conn.tx.send(bytes); // dropped if the writer has gone away
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
    // Guard against a second node-start, which would otherwise leak the previous
    // listener + acceptor thread.
    {
        let n = crate::core::sync::read(&NODE);
        if n.started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this runtime is already a node (node-start called twice)",
            ));
        }
    }
    // Publish identity, then bind the first listener. The acceptor reads identity
    // lazily (at accept time), so it's set before any peer can be served; if the
    // bind fails we roll the identity back, leaving the runtime a non-node so
    // node-start can be retried.
    set_identity(name, cookie);
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
        let path = path.to_string();
        prepare_unix_path(&path)?;
        let listener = UnixListener::bind(&path)?;
        spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Unix(s)));
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        let listener = TcpListener::bind(hostport)?;
        spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Tcp(s)));
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ));
    }
    Ok(())
}

/// Publish this runtime's node identity: name + cookie under the `NODE` lock,
/// then the lock-free name cache. `Release` so a reader on another core that
/// loads `LOCAL_NODE` with `Acquire` is guaranteed to see the cookie + name too;
/// the hot path (`local_node`) is the matched `Acquire`.
fn set_identity(name: Symbol, cookie: String) {
    {
        let mut n = crate::core::sync::write(&NODE);
        n.name = name;
        n.cookie = cookie;
        n.started = true;
    }
    LOCAL_NODE.store(name, Ordering::Release);
}

/// Roll back [`set_identity`] — used when the first listener's bind fails, so a
/// failed `node-start` leaves the runtime a non-node (retryable) rather than a
/// node with no listener.
fn clear_identity() {
    {
        let mut n = crate::core::sync::write(&NODE);
        n.name = *NONODE;
        n.cookie = String::new();
        n.started = false;
    }
    LOCAL_NODE.store(u32::MAX, Ordering::Release);
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

/// The accept loop, shared by both transports: pull the next link off `accept`
/// and hand each to a panic-isolated per-connection thread. A transient accept
/// error (EMFILE etc.) logs and re-loops with a tiny backoff rather than
/// burn-looping or killing the acceptor.
fn spawn_acceptor(accept: impl FnMut() -> io::Result<Stream> + Send + 'static) {
    let mut accept = accept;
    std::thread::spawn(move || loop {
        match accept() {
            Ok(stream) => {
                // Shed past the in-flight-handshake cap *before* spawning a thread
                // or reading a byte, so a flood of unauthenticated connections
                // can't exhaust threads/memory. Closing the socket is the whole
                // response — no thread, no log (a per-shed log would itself be a
                // flood vector under attack).
                let permit = match HandshakeSlot::try_acquire() {
                    Some(p) => p,
                    None => {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                };
                std::thread::spawn(move || {
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
            }
            Err(e) => {
                eprintln!("dist: accept error: {}", e);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    });
}

/// Ready a Unix-socket path for `bind`: create the parent directory (`0700`) and
/// clear a **stale** socket left by a crashed node. A path that still has a live
/// listener is refused (another node owns that name); a path that refuses a
/// connection is stale and gets unlinked so we can rebind. Best-effort against a
/// concurrent same-name start — a same-user dev footgun, not a security boundary
/// (the `0700` dir already gates other users).
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
            // Refused / no listener → a stale file from a dead node; clear it so
            // `bind` can recreate it.
            Err(_) => {
                let _ = std::fs::remove_file(p);
            }
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
    // Best-effort de-dup: if we already have a link to the named node, reuse it
    // instead of dialing a redundant one. (A genuine simultaneous-connect race is
    // still resolved by the tie-break in `establish`.)
    if crate::core::sync::read(&NODES).contains_key(&peer) {
        return Ok(peer);
    }
    let mut stream = dial(addr)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let peer = handshake(&mut stream, Role::Initiator)?;
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Initiator);
    Ok(peer)
}

/// Open the carrier for `addr`. Unix connects are local and effectively instant
/// (or refuse immediately); TCP uses `connect_timeout` per resolved address, so
/// a silently-dropping peer can't wedge the dialer at the kernel SYN timeout.
fn dial(addr: &str) -> io::Result<Stream> {
    if let Some(path) = addr.strip_prefix("unix:") {
        Ok(Stream::Unix(UnixStream::connect(path)?))
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        // `connect_timeout` requires a `SocketAddr`, so resolve here and try each
        // address in turn — same multi-A-record behaviour as `TcpStream::connect`
        // while bounding the wait per attempt.
        let mut last_err: Option<io::Error> = None;
        let stream = hostport.to_socket_addrs()?.find_map(|sa| {
            match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
                Ok(s) => Some(s),
                Err(e) => {
                    last_err = Some(e);
                    None
                }
            }
        });
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
    let peer = handshake(&mut stream, Role::Responder)?;
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Responder);
    Ok(())
}

/// Register the authenticated link and spawn its reader + writer threads —
/// resolving a duplicate against any existing link to the same peer first.
fn establish(peer: Symbol, stream: Stream, role: Role) {
    // Who initiated *this* connection (the tie-break key).
    let connector = match role {
        Role::Initiator => local_node(),
        Role::Responder => peer,
    };
    let sock = Arc::new(stream);
    let (tx, rx) = mpsc::channel::<Arc<[u8]>>();
    let last_seen = Arc::new(AtomicU64::new(now_millis()));
    let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);

    // Decide winner vs. any existing link, and register atomically under the lock.
    // Compare connectors by *name* (spelling) — interned ids differ per process,
    // but both ends share the names, so they pick the same physical link.
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
            _ => {
                // We win (or there was no existing link). Take over the slot; any
                // evicted link is torn down below, outside the lock.
                let old = nodes.remove(&peer);
                nodes.insert(
                    peer,
                    Conn {
                        id,
                        connector,
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

    // Writer: drain the channel onto the socket. A per-write timeout
    // (`WRITE_TIMEOUT`) prevents a slowloris peer from pinning the writer and
    // ballooning `rx` — a timeout is treated the same as an I/O error, fall
    // through to shutdown.
    let writer_sock = Arc::clone(&sock);
    let _ = writer_sock.set_write_timeout(Some(WRITE_TIMEOUT));
    std::thread::spawn(move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for bytes in rx {
                if (&*writer_sock).write_all(&bytes).is_err() {
                    let _ = writer_sock.shutdown(Shutdown::Both);
                    break;
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
    let pong: Arc<[u8]> = Arc::from(frame_bytes(&Frame::Pong).expect("encode Pong"));
    std::thread::spawn(move || {
        let mut r: &Stream = &reader_sock;
        // Loop until peer closes, protocol error, or a deliberate `shutdown`.
        // `peer` is the *authenticated* node name from the handshake — we use
        // it instead of the wire's `from_node` field on Monitor/Demonitor so a
        // malicious peer can't claim to be node X and inject `[:down …]`
        // deliveries to processes watching X. The `from_node` field stays in
        // the wire format for clean error paths (encode side still emits it)
        // but is *not consulted* on this side.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(frame) = read_frame(&mut r) {
                last_seen.store(now_millis(), Ordering::Release);
                match frame {
                    Frame::Send { target, msg } => deliver_inbound(target, msg),
                    Frame::Ping => {
                        let _ = reader_tx.send(Arc::clone(&pong));
                    }
                    // A peer asked to watch one of our local pids — re-use the
                    // shared `add_monitor` core with a `Watcher::Remote` so the
                    // alive-target / dead-target paths are exactly the local
                    // monitor's, just with a different delivery channel.
                    Frame::Monitor {
                        from_node: _wire_node,
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
                    Frame::Demonitor {
                        from_node: _wire_node,
                        watcher_pid,
                        mref,
                    } => process::drop_monitor(|w| {
                        matches!(*w, process::Watcher::Remote { node, pid, mref: r }
                                     if node == peer && pid == watcher_pid && r == mref)
                    }),
                    // A peer linked its `from_pid` to our local `to_pid` — record
                    // our half (keyed by the trusted connection `peer`, not the
                    // wire's `from_node`, same as the monitor handlers).
                    Frame::Link {
                        from_node: _wire_node,
                        from_pid,
                        to_pid,
                    } => process::record_remote_link(to_pid, peer, from_pid),
                    Frame::Unlink {
                        from_node: _wire_node,
                        from_pid,
                        to_pid,
                    } => process::drop_remote_link(to_pid, peer, from_pid),
                    // An exit signal for our local `to_pid`. A link death goes
                    // through the trap-or-propagate path; an explicit remote exit
                    // is routed straight to `scheduler::exit` (kill-style).
                    Frame::Exit {
                        from_node: _wire_node,
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
                    // Handshake-only frames in steady state: a peer that
                    // re-sends one after the link is up is malformed but harmless
                    // — keep reading.
                    Frame::Pong | Frame::Hello { .. } | Frame::Auth { .. } => {}
                }
            }
        }));
        drop_link(peer, id);
    });
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
            None => return,
        },
    };
    process::deliver(id, msg);
}

mod handshake;
mod heartbeat;
mod wire;

use heartbeat::ensure_heartbeat;
use wire::{frame_bytes, read_frame, Frame};

#[cfg(test)]
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
        assert_eq!(IN_FLIGHT_HANDSHAKES.load(Ordering::Acquire), MAX_IN_FLIGHT_HANDSHAKES);

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
}
