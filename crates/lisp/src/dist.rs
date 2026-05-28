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
use std::io::{self, Cursor, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Once, RwLock};
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

/// How often the (single, shared) heartbeat thread probes each link with a `Ping`
/// and checks liveness. Idle-gated: a `Ping` is a 5-byte frame, only sent on the
/// tick, never per message.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// A link with no inbound frame (data, `Ping`, or `Pong`) for this long is
/// declared **down**: we `shutdown` its socket, which tears it down and fires
/// `[:nodedown name]` to its watchers. Several heartbeat intervals, so a single
/// dropped probe doesn't flap a healthy link.
const DOWN_AFTER: Duration = Duration::from_secs(6);

/// Monotonic clock base, so `last_seen` can live in an `AtomicU64` of millis.
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
    match LOCAL_NODE.load(Ordering::Relaxed) {
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
    NAMES.write().unwrap().insert(name, id);
}

/// `(monitor-node name pid)` — deliver `[:nodedown name]` to `pid` when a link to
/// `name` goes down. Persistent (fires on each down) until the process exits.
/// If `name` isn't us and there's no current link, the node is effectively
/// already down and `[:nodedown]` is delivered immediately (Erlang's
/// `monitor_node` semantics).
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    NODE_MONITORS.write().unwrap().entry(name).or_default().push(pid);
    if !is_local(name) && !NODES.read().unwrap().contains_key(&name) {
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
    NODES.read().unwrap().keys().copied().collect()
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
            Target::Name(name) => match NAMES.read().unwrap().get(&name).copied() {
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
    if let Some(conn) = NODES.read().unwrap().get(&node) {
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
        let n = NODE.read().unwrap();
        if n.started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this runtime is already a node (node-start called twice)",
            ));
        }
    }
    let listener = TcpListener::bind(addr)?;
    {
        let mut n = NODE.write().unwrap();
        n.name = name;
        n.cookie = cookie;
        n.started = true;
    }
    LOCAL_NODE.store(name, Ordering::Relaxed); // publish for the lock-free hot path
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                if let Err(e) = accept(stream) {
                    eprintln!("dist: incoming connection failed: {}", e);
                }
            });
        }
    });
    Ok(())
}

/// Which end opened a connection — the tie-break for a duplicate keeps the link
/// initiated by the smaller node name, so both ends need to know who that is.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// We dialed (`connect`) — the initiator is us.
    Initiator,
    /// We accepted — the initiator is the peer.
    Responder,
}

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
        if NODES.read().unwrap().contains_key(&claimed) {
            return Ok(claimed);
        }
    }
    let mut stream = TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // Client speaks first: send our Hello, then read the peer's.
    let (name, cookie) = {
        let n = NODE.read().unwrap();
        (n.name, n.cookie.clone())
    };
    write_frame(&mut stream, &Frame::Hello { node: name, cookie })?;
    let peer = match read_frame(&mut stream)? {
        Frame::Hello { node, cookie } => {
            check_cookie(&cookie)?;
            node
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello")),
    };
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Initiator);
    Ok(peer)
}

/// Server side of the handshake: read the peer's Hello, authenticate, reply with
/// ours, then start the link threads.
fn accept(mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let peer = match read_frame(&mut stream)? {
        Frame::Hello { node, cookie } => {
            check_cookie(&cookie)?;
            node
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello")),
    };
    let (name, cookie) = {
        let n = NODE.read().unwrap();
        (n.name, n.cookie.clone())
    };
    write_frame(&mut stream, &Frame::Hello { node: name, cookie })?;
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(peer, stream, Role::Responder);
    Ok(())
}

/// Compare a peer's presented cookie to ours (shared-secret equality). A
/// placeholder for real auth (documented in `concurrency.md`).
fn check_cookie(presented: &str) -> io::Result<()> {
    if presented == NODE.read().unwrap().cookie {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "node cookie mismatch",
        ))
    }
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
        let mut nodes = NODES.write().unwrap();
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

    // Writer: drain the channel onto the socket. On a write error, `shutdown` so
    // the reader unblocks and runs the single teardown path.
    let writer_sock = Arc::clone(&sock);
    std::thread::spawn(move || {
        for bytes in rx {
            if (&*writer_sock).write_all(&bytes).is_err() {
                let _ = writer_sock.shutdown(Shutdown::Both);
                break;
            }
        }
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
        while let Ok(frame) = read_frame(&mut r) {
            last_seen.store(now_millis(), Ordering::Relaxed);
            match frame {
                Frame::Send { target, msg } => deliver_inbound(target, msg),
                Frame::Ping => {
                    let _ = reader_tx.send(Arc::clone(&pong));
                }
                Frame::Pong | Frame::Hello { .. } => {}
            }
        }
        drop_link(peer, id);
    });
}

/// Remove a link from `NODES` **iff** it's still this generation (so an evicted or
/// replaced link can't tear down its successor), and fire node-down watchers.
fn drop_link(peer: Symbol, id: u64) {
    let removed = {
        let mut nodes = NODES.write().unwrap();
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

/// Deliver `[:nodedown name]` to every process that called `(monitor-node name)`.
fn fire_nodedown(peer: Symbol) {
    let watchers = NODE_MONITORS.read().unwrap().get(&peer).cloned();
    if let Some(watchers) = watchers {
        let msg = nodedown_msg(peer);
        for w in watchers {
            process::deliver(w, msg.clone());
        }
    }
}

/// An inbound `Send` from a peer: resolve the target locally and deliver.
fn deliver_inbound(target: Target, msg: Message) {
    let id = match target {
        Target::Pid(id) => id,
        Target::Name(name) => match NAMES.read().unwrap().get(&name).copied() {
            Some(id) => id,
            None => return,
        },
    };
    process::deliver(id, msg);
}

// ----- liveness (heartbeat) --------------------------------------------------

static HEARTBEAT_STARTED: Once = Once::new();

/// Start the single shared heartbeat thread once, on the first established link.
fn ensure_heartbeat() {
    HEARTBEAT_STARTED.call_once(|| {
        std::thread::spawn(heartbeat_loop);
    });
}

/// Probe every link each interval: if it's been silent past `DOWN_AFTER`, declare
/// it down (shutdown → the reader runs `drop_link` → `[:nodedown]`); otherwise
/// send a `Ping` (the peer answers `Pong`, refreshing its `last_seen`). One thread
/// for all links; a `Ping` is sent only on the tick, never per message.
fn heartbeat_loop() {
    // One shared Ping buffer for every link, every tick: each send is an
    // `Arc::clone` (atomic incr), not a `Vec` copy.
    let ping: Arc<[u8]> = Arc::from(frame_bytes(&Frame::Ping).expect("encode Ping"));
    let down_after = DOWN_AFTER.as_millis() as u64;
    loop {
        std::thread::sleep(HEARTBEAT_INTERVAL);
        let now = now_millis();
        // Snapshot under the lock, then act without holding it (shutdown/send can block).
        let links: Vec<(Arc<TcpStream>, Sender<Arc<[u8]>>, u64)> = {
            let nodes = NODES.read().unwrap();
            nodes
                .values()
                .map(|c| (Arc::clone(&c.sock), c.tx.clone(), c.last_seen.load(Ordering::Relaxed)))
                .collect()
        };
        for (sock, tx, last) in links {
            if now.saturating_sub(last) > down_after {
                let _ = sock.shutdown(Shutdown::Both); // dead peer → tear down via the reader
            } else {
                let _ = tx.send(Arc::clone(&ping));
            }
        }
    }
}

// ----- wire frames -----------------------------------------------------------

enum Frame {
    /// Handshake: who I am + my cookie.
    Hello { node: Symbol, cookie: String },
    /// Route `msg` to `target` on the receiving node.
    Send { target: Target, msg: Message },
    /// Liveness probe; the peer answers with `Pong`.
    Ping,
    /// Reply to a `Ping`. (Receiving any frame refreshes liveness; these two carry
    /// no payload, just keep an idle link demonstrably alive.)
    Pong,
}

const FRAME_HELLO: u8 = 0;
const FRAME_SEND: u8 = 1;
const FRAME_PING: u8 = 2;
const FRAME_PONG: u8 = 3;
const TARGET_PID: u8 = 0;
const TARGET_NAME: u8 = 1;

/// Encode a frame with its `[u32 len][payload]` length prefix, ready to write.
fn frame_bytes(frame: &Frame) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    encode_frame(&mut payload, frame)?;
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&frame_bytes(frame)?)
}

/// Read one length-prefixed frame, rejecting an over-large prefix before
/// allocating for it.
fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME}-byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    decode_frame(&mut Cursor::new(buf))
}

fn encode_frame(w: &mut Vec<u8>, frame: &Frame) -> io::Result<()> {
    match frame {
        Frame::Hello { node, cookie } => {
            w.push(FRAME_HELLO);
            put_sym(w, *node);
            put_str(w, cookie);
        }
        Frame::Send { target, msg } => {
            w.push(FRAME_SEND);
            encode_target(w, target);
            encode_msg(w, msg)?;
        }
        Frame::Ping => w.push(FRAME_PING),
        Frame::Pong => w.push(FRAME_PONG),
    }
    Ok(())
}

fn decode_frame(r: &mut Cursor<Vec<u8>>) -> io::Result<Frame> {
    match get_u8(r)? {
        FRAME_HELLO => Ok(Frame::Hello {
            node: get_sym(r)?,
            cookie: get_str(r)?,
        }),
        FRAME_SEND => Ok(Frame::Send {
            target: decode_target(r)?,
            msg: decode_msg(r)?,
        }),
        FRAME_PING => Ok(Frame::Ping),
        FRAME_PONG => Ok(Frame::Pong),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown frame tag {t}"),
        )),
    }
}

fn encode_target(w: &mut Vec<u8>, target: &Target) {
    match target {
        Target::Pid(id) => {
            w.push(TARGET_PID);
            w.extend_from_slice(&id.to_be_bytes()); // u64
        }
        Target::Name(s) => {
            w.push(TARGET_NAME);
            put_sym(w, *s);
        }
    }
}

fn decode_target(r: &mut Cursor<Vec<u8>>) -> io::Result<Target> {
    match get_u8(r)? {
        TARGET_PID => Ok(Target::Pid(get_u64(r)?)),
        TARGET_NAME => Ok(Target::Name(get_sym(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown target tag {t}"),
        )),
    }
}

// ----- Message codec (symbols travel by name) --------------------------------

const M_NIL: u8 = 0;
const M_FALSE: u8 = 1;
const M_TRUE: u8 = 2;
const M_INT: u8 = 3;
const M_FLOAT: u8 = 4;
const M_STR: u8 = 5;
const M_SYM: u8 = 6;
const M_KEYWORD: u8 = 7;
const M_LIST: u8 = 8;
const M_VECTOR: u8 = 9;
const M_MAP: u8 = 10;
const M_REF: u8 = 11;
const M_PID: u8 = 12;
/// A serialised closure (ADR-033 closure-as-data path). Body and optionals'
/// defaults are S-expression forms — already messages — so the wire encoding
/// is a flat record: name?, params, optionals, rest?, body, doc?, captured.
/// The receiver's `closure_from_message` chains captured frees onto its own
/// global scope; free globals re-resolve there (Erlang's "the module must be
/// loaded on both nodes").
const M_CLOSURE: u8 = 13;

fn encode_msg(w: &mut Vec<u8>, m: &Message) -> io::Result<()> {
    match m {
        Message::Nil => w.push(M_NIL),
        Message::Bool(false) => w.push(M_FALSE),
        Message::Bool(true) => w.push(M_TRUE),
        Message::Int(n) => {
            w.push(M_INT);
            w.extend_from_slice(&n.to_be_bytes());
        }
        Message::Float(f) => {
            w.push(M_FLOAT);
            w.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Message::Str(s) => {
            w.push(M_STR);
            put_str(w, s);
        }
        Message::Sym(s) => {
            w.push(M_SYM);
            put_sym(w, *s);
        }
        Message::Keyword(s) => {
            w.push(M_KEYWORD);
            put_sym(w, *s);
        }
        Message::List(items) => {
            w.push(M_LIST);
            put_u32(w, items.len() as u32);
            for it in items {
                encode_msg(w, it)?;
            }
        }
        Message::Vector(items) => {
            w.push(M_VECTOR);
            put_u32(w, items.len() as u32);
            for it in items {
                encode_msg(w, it)?;
            }
        }
        Message::Map(entries) => {
            w.push(M_MAP);
            put_u32(w, entries.len() as u32);
            for (k, v) in entries {
                encode_msg(w, k)?;
                encode_msg(w, v)?;
            }
        }
        Message::Ref(n) => {
            w.push(M_REF);
            w.extend_from_slice(&n.to_be_bytes());
        }
        Message::Pid { node, id } => {
            w.push(M_PID);
            put_sym(w, *node);
            w.extend_from_slice(&id.to_be_bytes());
        }
        Message::Closure(c) => {
            w.push(M_CLOSURE);
            encode_closure(w, c)?;
        }
    }
    Ok(())
}

/// Wire form of a `ClosureMsg`. Same field order as the struct; symbols travel
/// by name (separate runtimes have independent interners — see [`put_sym`]).
/// Two callouts:
///   - Symbol/string optionals carry a 1-byte `0`/`1` tag, then the value
///     when present. Cheap and unambiguous in a stream codec.
///   - Body/optional-default *forms* are already `Message`s (S-expression
///     data), so they recurse through [`encode_msg`] — code travels exactly
///     like any other data.
fn encode_closure(w: &mut Vec<u8>, c: &crate::process::ClosureMsg) -> io::Result<()> {
    put_opt_sym(w, c.name);
    put_u32(w, c.params.len() as u32);
    for &s in &c.params {
        put_sym(w, s);
    }
    put_u32(w, c.optionals.len() as u32);
    for (s, m) in &c.optionals {
        put_sym(w, *s);
        encode_msg(w, m)?;
    }
    put_opt_sym(w, c.rest);
    put_u32(w, c.body.len() as u32);
    for m in &c.body {
        encode_msg(w, m)?;
    }
    put_opt_str(w, c.doc.as_deref());
    put_u32(w, c.captured.len() as u32);
    for (s, m) in &c.captured {
        put_sym(w, *s);
        encode_msg(w, m)?;
    }
    Ok(())
}

fn decode_msg(r: &mut Cursor<Vec<u8>>) -> io::Result<Message> {
    Ok(match get_u8(r)? {
        M_NIL => Message::Nil,
        M_FALSE => Message::Bool(false),
        M_TRUE => Message::Bool(true),
        M_INT => Message::Int(get_i64(r)?),
        M_FLOAT => Message::Float(f64::from_bits(get_u64(r)?)),
        M_STR => Message::Str(get_str(r)?),
        M_SYM => Message::Sym(get_sym(r)?),
        M_KEYWORD => Message::Keyword(get_sym(r)?),
        M_LIST => {
            let n = get_u32(r)? as usize;
            let mut items = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                items.push(decode_msg(r)?);
            }
            Message::List(items)
        }
        M_VECTOR => {
            let n = get_u32(r)? as usize;
            let mut items = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                items.push(decode_msg(r)?);
            }
            Message::Vector(items)
        }
        M_MAP => {
            let n = get_u32(r)? as usize;
            let mut entries = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                let k = decode_msg(r)?;
                let v = decode_msg(r)?;
                entries.push((k, v));
            }
            Message::Map(entries)
        }
        M_REF => Message::Ref(get_u64(r)?),
        M_PID => Message::Pid {
            node: get_sym(r)?,
            id: get_u64(r)?,
        },
        M_CLOSURE => Message::Closure(Box::new(decode_closure(r)?)),
        t => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown message tag {t}"),
            ))
        }
    })
}

/// Inverse of [`encode_closure`]. Each `Vec`'s length is bounded by the
/// frame's remaining bytes (via [`prealloc`]) so a tiny frame claiming a huge
/// count can't trigger a large allocation up front — the decode loop fails
/// cleanly on EOF instead.
fn decode_closure(r: &mut Cursor<Vec<u8>>) -> io::Result<crate::process::ClosureMsg> {
    let name = get_opt_sym(r)?;
    let n = get_u32(r)? as usize;
    let mut params = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        params.push(get_sym(r)?);
    }
    let n = get_u32(r)? as usize;
    let mut optionals = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        let s = get_sym(r)?;
        let m = decode_msg(r)?;
        optionals.push((s, m));
    }
    let rest = get_opt_sym(r)?;
    let n = get_u32(r)? as usize;
    let mut body = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        body.push(decode_msg(r)?);
    }
    let doc = get_opt_str(r)?;
    let n = get_u32(r)? as usize;
    let mut captured = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        let s = get_sym(r)?;
        let m = decode_msg(r)?;
        captured.push((s, m));
    }
    Ok(crate::process::ClosureMsg {
        name,
        params,
        optionals,
        rest,
        body,
        doc,
        captured,
    })
}

// ----- byte helpers ----------------------------------------------------------

fn put_u32(w: &mut Vec<u8>, n: u32) {
    w.extend_from_slice(&n.to_be_bytes());
}

fn put_str(w: &mut Vec<u8>, s: &str) {
    put_u32(w, s.len() as u32);
    w.extend_from_slice(s.as_bytes());
}

/// A symbol is encoded **by name** — separate runtimes have independent
/// interners, so the id is meaningless across the wire.
fn put_sym(w: &mut Vec<u8>, s: Symbol) {
    put_str(w, &value::symbol_name(s));
}

/// `Option<Symbol>` as a `0`/`1` presence tag + the symbol's name when set.
/// One byte cheaper than encoding `nil` as a sentinel name, and unambiguous
/// in a stream codec.
fn put_opt_sym(w: &mut Vec<u8>, s: Option<Symbol>) {
    match s {
        Some(s) => {
            w.push(1);
            put_sym(w, s);
        }
        None => w.push(0),
    }
}

/// `Option<&str>` with the same `0`/`1` tag shape as [`put_opt_sym`].
fn put_opt_str(w: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => {
            w.push(1);
            put_str(w, s);
        }
        None => w.push(0),
    }
}

fn get_opt_sym(r: &mut Cursor<Vec<u8>>) -> io::Result<Option<Symbol>> {
    match get_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(get_sym(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad Option<Symbol> tag {t}"),
        )),
    }
}

fn get_opt_str(r: &mut Cursor<Vec<u8>>) -> io::Result<Option<String>> {
    match get_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(get_str(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad Option<String> tag {t}"),
        )),
    }
}

/// Bytes left in the frame buffer. Used to bound allocations by what the frame
/// could actually contain — a count/length field is attacker-controlled, but the
/// buffer is already capped at [`MAX_FRAME`], so an element can't be smaller than
/// one byte and `n` items need at least `n` bytes.
fn remaining(r: &Cursor<Vec<u8>>) -> usize {
    (r.get_ref().len() as u64).saturating_sub(r.position()) as usize
}

/// A safe pre-allocation size for a claimed count of `n` items: never reserve
/// more than the frame's remaining bytes can hold, so a tiny frame claiming a
/// huge count can't trigger a giant up-front allocation (the decode loop then
/// fails cleanly on EOF).
fn prealloc(r: &Cursor<Vec<u8>>, n: usize) -> usize {
    n.min(remaining(r))
}

fn get_u8(r: &mut Cursor<Vec<u8>>) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn get_u32(r: &mut Cursor<Vec<u8>>) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn get_u64(r: &mut Cursor<Vec<u8>>) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

fn get_i64(r: &mut Cursor<Vec<u8>>) -> io::Result<i64> {
    Ok(get_u64(r)? as i64)
}

fn get_str(r: &mut Cursor<Vec<u8>>) -> io::Result<String> {
    let n = get_u32(r)? as usize;
    // A string can't be longer than the bytes left in the frame; reject before
    // allocating, so a small frame claiming a huge length can't OOM us.
    if n > remaining(r) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "string length exceeds frame",
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad utf8"))
}

/// Read a symbol name and re-intern it into *this* runtime's interner.
fn get_sym(r: &mut Cursor<Vec<u8>>) -> io::Result<Symbol> {
    Ok(value::intern(&get_str(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a frame (with its length prefix) and decode it back.
    fn read_full(frame: &Frame) -> Frame {
        let bytes = frame_bytes(frame).unwrap();
        read_frame(&mut Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn hello_roundtrips() {
        let f = Frame::Hello {
            node: value::intern("alpha"),
            cookie: "s3cret".to_string(),
        };
        match read_full(&f) {
            Frame::Hello { node, cookie } => {
                assert_eq!(value::symbol_name(node), "alpha");
                assert_eq!(cookie, "s3cret");
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn send_with_rich_message_roundtrips() {
        // A message exercising symbols/keywords/pids/maps/nesting — the symbol
        // fields must survive as *names* (re-interned on decode).
        let msg = Message::Vector(vec![
            Message::Keyword(value::intern("pong")),
            Message::Pid {
                node: value::intern("beta"),
                id: 7,
            },
            Message::Map(vec![(
                Message::Keyword(value::intern("status")),
                Message::Sym(value::intern("ok")),
            )]),
            Message::Int(-42),
            Message::Str("hi".to_string()),
        ]);
        let f = Frame::Send {
            target: Target::Name(value::intern("echo")),
            msg,
        };
        match read_full(&f) {
            Frame::Send { target, msg } => {
                match target {
                    Target::Name(s) => assert_eq!(value::symbol_name(s), "echo"),
                    _ => panic!("wrong target"),
                }
                match msg {
                    Message::Vector(items) => {
                        assert!(matches!(&items[0], Message::Keyword(k) if value::symbol_name(*k) == "pong"));
                        assert!(matches!(&items[1], Message::Pid { node, id } if value::symbol_name(*node) == "beta" && *id == 7));
                    }
                    _ => panic!("wrong message"),
                }
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn pid_id_survives_above_u32() {
        // The local id is u64 end-to-end (the scheduler counter is u64); a value
        // past u32::MAX must round-trip, not truncate.
        let big = (u32::MAX as u64) + 12345;
        let f = Frame::Send {
            target: Target::Pid(big),
            msg: Message::Pid {
                node: value::intern("n"),
                id: big,
            },
        };
        match read_full(&f) {
            Frame::Send {
                target: Target::Pid(t),
                msg: Message::Pid { id, .. },
            } => {
                assert_eq!(t, big);
                assert_eq!(id, big);
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn oversized_length_prefix_is_rejected_not_allocated() {
        // A 4-byte prefix claiming ~4 GiB must error, never `vec![0; 4e9]`.
        let mut bytes = (u32::MAX).to_be_bytes().to_vec();
        bytes.push(M_NIL); // a token byte of "payload"
        match read_frame(&mut Cursor::new(bytes)) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("oversized frame should be rejected"),
        }
    }

    #[test]
    fn closure_roundtrips_through_the_wire() {
        // A `ClosureMsg` exercising every optional + every list — the kind a
        // real `(fn (a &optional (b 10) &) … )` would serialise to. Captures
        // are stand-ins for free locals copied from the sender's frame; on
        // the receiver they chain onto its global scope.
        use crate::process::ClosureMsg;
        let c = ClosureMsg {
            name: Some(value::intern("worker")),
            params: vec![value::intern("a"), value::intern("b")],
            optionals: vec![(value::intern("c"), Message::Int(10))],
            rest: Some(value::intern("xs")),
            // (a body of `(+ a b c)` — just the *message* form of it)
            body: vec![Message::List(vec![
                Message::Sym(value::intern("+")),
                Message::Sym(value::intern("a")),
                Message::Sym(value::intern("b")),
                Message::Sym(value::intern("c")),
            ])],
            doc: Some("add three".to_string()),
            captured: vec![(value::intern("seed"), Message::Int(42))],
        };
        let f = Frame::Send {
            target: Target::Pid(1),
            msg: Message::Closure(Box::new(c)),
        };
        match read_full(&f) {
            Frame::Send {
                msg: Message::Closure(c),
                ..
            } => {
                assert_eq!(value::symbol_name(c.name.unwrap()), "worker");
                assert_eq!(c.params.len(), 2);
                assert_eq!(value::symbol_name(c.params[0]), "a");
                assert_eq!(c.optionals.len(), 1);
                assert!(matches!(&c.optionals[0].1, Message::Int(10)));
                assert_eq!(value::symbol_name(c.rest.unwrap()), "xs");
                assert_eq!(c.body.len(), 1);
                assert_eq!(c.doc.as_deref(), Some("add three"));
                assert_eq!(c.captured.len(), 1);
                assert!(matches!(&c.captured[0].1, Message::Int(42)));
            }
            other => panic!("wrong frame after round-trip: {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn closure_with_all_options_absent_roundtrips() {
        // The minimal case: no name, no rest, no doc, no optionals, no captures —
        // a global-capturing `(fn (x) x)`. Each Option's 0/1 tag has to survive
        // cleanly, otherwise decoding would mis-align.
        use crate::process::ClosureMsg;
        let c = ClosureMsg {
            name: None,
            params: vec![value::intern("x")],
            optionals: vec![],
            rest: None,
            body: vec![Message::Sym(value::intern("x"))],
            doc: None,
            captured: vec![],
        };
        let f = Frame::Send {
            target: Target::Pid(1),
            msg: Message::Closure(Box::new(c)),
        };
        match read_full(&f) {
            Frame::Send {
                msg: Message::Closure(c),
                ..
            } => {
                assert!(c.name.is_none());
                assert!(c.rest.is_none());
                assert!(c.doc.is_none());
                assert!(c.optionals.is_empty());
                assert!(c.captured.is_empty());
                assert_eq!(c.params.len(), 1);
                assert_eq!(c.body.len(), 1);
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn bogus_collection_count_errors_without_huge_alloc() {
        // A tiny frame whose list claims billions of elements: prealloc is bounded
        // by the remaining bytes, and decoding fails cleanly on EOF (no OOM).
        let mut payload = vec![FRAME_SEND];
        encode_target(&mut payload, &Target::Pid(1));
        payload.push(M_LIST);
        payload.extend_from_slice(&u32::MAX.to_be_bytes()); // claims 4 billion items
        // …but no item bytes follow.
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&payload);
        match read_frame(&mut Cursor::new(framed)) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
            Ok(_) => panic!("a list claiming more items than bytes should fail"),
        }
    }
}
