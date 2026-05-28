//! Distributed nodes: connect two Brood runtimes over TCP and route messages
//! between them. Erlang-style distribution falls out of share-nothing +
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
use std::io::{self, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use crate::core::value::{self, Symbol};
use crate::process::{self, Message};

/// Hard ceiling on a single wire frame (bytes). A peer can otherwise put any
/// `u32` in the length prefix and make us allocate it sight unseen — including
/// random bytes from a port scan or a stray HTTP request hitting the port. Cap
/// it so a bad/oversized frame is rejected, not OOM'd. 64 MiB is far above any
/// real message.
const MAX_FRAME: usize = 64 * 1024 * 1024;

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

// ----- node identity ---------------------------------------------------------

struct NodeIdentity {
    name: Symbol,
    cookie: String,
    started: bool,
}

/// The name a pid carries before `node-start` runs: every such pid is local.
static NONODE: LazyLock<Symbol> = LazyLock::new(|| value::intern("nonode"));

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
    sock: Arc<TcpStream>,
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
pub(crate) fn monitor_remote(
    target_node: Symbol,
    target_pid: u64,
    watcher_pid: u64,
    mref: u64,
) {
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

/// `(monitor-node name pid)` — deliver `[:nodedown name]` to `pid` when a link to
/// `name` goes down. Persistent (fires on each down) until the process exits.
/// If `name` isn't us and there's no current link, the node is effectively
/// already down and `[:nodedown]` is delivered immediately (Erlang's
/// `monitor_node` semantics).
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    crate::core::sync::write(&NODE_MONITORS).entry(name).or_default().push(pid);
    if !is_local(name) && !crate::core::sync::read(&NODES).contains_key(&name) {
        process::deliver(pid, nodedown_msg(name));
    }
}

/// The `[:nodedown <name>]` message a downed link delivers to its watchers.
fn nodedown_msg(name: Symbol) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("nodedown")),
        Message::Keyword(name),
    ])
}

/// Connected peer node names (for `(nodes)`).
pub(crate) fn connected_nodes() -> Vec<Symbol> {
    crate::core::sync::read(&NODES).keys().copied().collect()
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
            eprintln!("dist: cannot encode message for {}: {}", value::symbol_name(node), e);
            return;
        }
    };
    if let Some(conn) = crate::core::sync::read(&NODES).get(&node) {
        let _ = conn.tx.send(bytes); // dropped if the writer has gone away
    }
}

// ----- connection lifecycle --------------------------------------------------

/// `(node-start name "host:port" cookie)` — name this runtime, then listen for
/// peers. Each accepted connection is authenticated (cookie) and, on success,
/// gets reader + writer threads.
pub(crate) fn node_start(name: Symbol, addr: &str, cookie: String) -> io::Result<()> {
    // Bind first (it can fail on a bad/taken address) — but guard against a second
    // node-start, which would otherwise leak the previous listener + acceptor thread.
    {
        let n = crate::core::sync::read(&NODE);
        if n.started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this runtime is already a node (node-start called twice)",
            ));
        }
    }
    let listener = TcpListener::bind(addr)?;
    {
        let mut n = crate::core::sync::write(&NODE);
        n.name = name;
        n.cookie = cookie;
        n.started = true;
    }
    // `Release` so a reader on another core that loads with `Acquire` is
    // guaranteed to see the `NODE` lock's write (cookie + name) too. The hot
    // path (`local_node`) is the matched `Acquire`.
    LOCAL_NODE.store(name, Ordering::Release); // publish for the lock-free hot path
    std::thread::spawn(move || {
        // `flatten()` silently drops accept errors — wrap each iteration so a
        // transient EMFILE just logs and re-loops with a tiny backoff instead
        // of burn-looping the acceptor.
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    std::thread::spawn(move || {
                        // Catch a panic in the per-connection thread so one bad
                        // peer doesn't take down the runtime via thread-panic
                        // unwind (the rest of the dist surface assumes its
                        // background threads stay alive).
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Err(e) = accept(stream) {
                                eprintln!("dist: incoming connection failed: {}", e);
                            }
                        }));
                    });
                }
                Err(e) => {
                    eprintln!("dist: accept error: {}", e);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    });
    Ok(())
}

// `Role` + the four-step `handshake` live in `dist::handshake`; only the link
// lifecycle uses them, and they keep the cookie/nonce/MAC plumbing self-
// contained.
use handshake::{handshake, Role};

/// `(connect "name@host:port")` — dial a peer and complete the client handshake.
/// Returns the peer's (authoritative) node name on success.
pub(crate) fn connect(spec: &str) -> io::Result<Symbol> {
    let (claimed_name, addr) = spec
        .split_once('@')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected name@host:port"))?;
    // Refuse to dial ourselves — it would race through the handshake and form a
    // tie-break loser in the same process; cleaner to reject up front.
    if claimed_name == value::symbol_name(local_node()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot connect to self ({claimed_name})"),
        ));
    }
    // Best-effort de-dup: if we already have a link to the named node, reuse it
    // instead of dialing a redundant one. (A genuine simultaneous-connect race is
    // still resolved by the tie-break in `establish`.) `intern_existing` keeps
    // the interner from growing for names we ultimately don't use.
    if let Some(claimed) = value::intern_existing(claimed_name) {
        if crate::core::sync::read(&NODES).contains_key(&claimed) {
            return Ok(claimed);
        }
    }
    // `connect_timeout` requires a `SocketAddr`, so resolve here and try each
    // address in turn — gives us the same multi-A-record behaviour as
    // `TcpStream::connect(spec)` while bounding the wait per attempt.
    let mut last_err: Option<io::Error> = None;
    let stream = addr.to_socket_addrs()?.find_map(|sa| {
        match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
            Ok(s) => Some(s),
            Err(e) => {
                last_err = Some(e);
                None
            }
        }
    });
    let mut stream = stream.ok_or_else(|| {
        last_err.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "no addresses resolved")
        })
    })?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let peer = handshake(&mut stream, Role::Initiator)?;
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Initiator);
    Ok(peer)
}

/// Server side of the handshake: drive the v2 exchange, then start the link
/// threads. See [`handshake`] for the protocol.
fn accept(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let peer = handshake(&mut stream, Role::Responder)?;
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Responder);
    Ok(())
}

/// Register the authenticated link and spawn its reader + writer threads —
/// resolving a duplicate against any existing link to the same peer first.
fn establish(peer: Symbol, stream: TcpStream, role: Role) {
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
        let mut r: &TcpStream = &reader_sock;
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
