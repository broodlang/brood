//! TCP sockets (ADR-062) on one **reactor thread** (ADR-143), delivering to
//! process mailboxes (ADR-059).
//!
//! A socket never blocks a worker — and no longer costs a thread. One reactor
//! thread runs a `mio` poll loop that multiplexes **every** socket the runtime
//! owns: plaintext streams, TLS streams (client and server), and listeners.
//! Inbound data, accepted connections, and closes are delivered to the owning
//! process's mailbox; the Brood side just `receive`s. Shapes:
//!
//! - a stream delivers `[:tcp sock data]` per chunk, then `[:tcp-closed sock]`;
//! - a listener delivers `[:tcp-accept lsock client]` per connection;
//! - a TLS failure delivers `[:tcp-error sock msg]`.
//!
//! Ownership: `tcp-connect` makes an **active** stream — reads start at once,
//! delivering to the connecting process. An **accepted** stream is **passive** —
//! announced via `[:tcp-accept …]` but not read until `tcp-controlling-process`
//! assigns it an owner. This is the Erlang `gen_tcp` handoff: no inbound bytes
//! are lost to the acceptor before a per-connection handler takes over.
//!
//! **Writes are queued** (ADR-143): `tcp-send` lowers its iolist to bytes,
//! hands them to the reactor, and returns; the reactor flushes as the socket
//! accepts them. `tcp-close` flushes what is queued (bounded by [`LINGER`])
//! before closing, so `tcp-send` + `tcp-close` can never truncate a response —
//! the old blocking-write model's documented footgun. A slow/stuck peer is
//! bounded by [`OUT_CAP`] per socket: past it the connection is dropped rather
//! than buffering without bound. Write failures surface as `[:tcp-closed …]`
//! (the reactor discovers them after `tcp-send` has returned).
//!
//! A socket is a `u64` id into a control-plane registry, surfaced as the scalar
//! handle `Value::Socket(id)` (the GC never traces or moves it). Valid across
//! this runtime's processes; not node-portable.
//!
//! **TEXT MODE (default) vs BINARY MODE** — the flag governs ONLY the inbound
//! decode (ADR-141): text mode delivers UTF-8 strings (a multi-byte character
//! split across a read boundary is carried to the next read via
//! [`chunk_payload`]; only a genuinely non-UTF-8 run becomes U+FFFD); binary
//! mode (`tcp-set-binary`) delivers byte-faithful first-class **`bytes`**
//! values. Outbound is mode-independent: string leaves are always UTF-8, raw
//! bytes ride as `bytes` values. TLS streams honor the flag exactly like
//! plaintext ones — including `tls-request` responses.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mio::net::{TcpListener as MioListener, TcpStream as MioStream};
use mio::{Events, Interest, Poll, Token, Waker};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

use crate::core::value;
use crate::process::{chunk_flush, chunk_payload, sink_pair, MailboxSink, Message};

// ---- tunables ----

/// How long a passively-accepted socket may sit **unclaimed** (announced via
/// `[:tcp-accept …]` but never handed an owner with `tcp-controlling-process`)
/// before the reactor drops it. Without this, a peer that opens connections an
/// application never accepts would leak an fd + a registry entry per connection
/// forever — a DoS surface for any server built on this mechanism.
const ACCEPT_REAP_AFTER: Duration = Duration::from_secs(30);

/// How long a TLS connection may take to complete its handshake before the
/// reactor drops it. A peer that opens a TLS connection — server-accepted
/// (`tls-listen`) or the client half of `tls-request` — then stalls mid-handshake
/// holds an fd the application **cannot** reclaim: it never sees the socket until
/// the handshake finishes, so no app-level read timeout can intervene. This is the
/// reactor's own bound on that window (handshakes complete in milliseconds; 30 s is
/// generous), the slow-loris / broken-peer guard the app can't provide itself.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-socket outbound queue cap. `tcp-send` is asynchronous (the reactor
/// flushes as the peer accepts bytes), so a stuck reader would otherwise grow
/// the queue without bound; past this the connection is dropped and the owner
/// sees `[:tcp-closed …]`. 16 MiB comfortably covers response bodies while
/// bounding a slow-reader DoS.
const OUT_CAP: usize = 16 * 1024 * 1024;

/// How long a closing socket may keep flushing queued outbound bytes before
/// the reactor gives up and drops it. Bounds `tcp-close` after a large
/// `tcp-send` to a slow peer.
const LINGER: Duration = Duration::from_secs(5);

/// The reactor's poll timeout — the cadence of reaper/linger housekeeping when
/// no IO is happening. Purely a housekeeping tick: IO readiness wakes the poll
/// immediately, commands wake it via the `Waker`.
const TICK: Duration = Duration::from_millis(1000);

const WAKER_TOKEN: Token = Token(0);

// ---- control plane: the id → socket registry the builtins talk to ----

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Stream,
    TlsStream,
    Listener,
    TlsListener,
}

/// The control-plane entry for one socket id: what the builtins need for
/// validation and bookkeeping. The data plane (fd, rustls state, queues,
/// carries) lives with the reactor; commands cross via [`Cmd`].
struct Ctl {
    kind: Kind,
    /// The green-process pid that owns this socket — the process whose death
    /// closes it (`close_process_sockets`). Updated by `controlling_process`.
    owner: u64,
    /// Inbound decode mode (ADR-141) — shared with the reactor, read per chunk.
    binary: Arc<AtomicBool>,
    /// Where inbound messages go — shared with the reactor's sink, retargeted
    /// by `controlling_process`.
    subscriber: Arc<AtomicU64>,
    /// Whether reads have been started (an active connect, or a claimed accept).
    /// Gates TLS `tcp-send` (a TLS connection exists only once claimed).
    claimed: bool,
    /// The local port, cached at creation so `tcp-local-port` never blocks.
    port: Option<u16>,
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Ctl>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Socket ids double as reactor poll tokens, so 0 is reserved for the waker.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Ctl>> {
    REGISTRY.lock().expect("socket registry mutex")
}

// ---- commands into the reactor ----

enum Cmd {
    /// A connected plaintext stream (from `tcp-connect`): start reading at once.
    Stream {
        id: u64,
        stream: MioStream,
        sink: MailboxSink,
        binary: Arc<AtomicBool>,
    },
    /// A plaintext listener.
    Listen {
        id: u64,
        listener: MioListener,
        sink: MailboxSink,
        subscriber: Arc<AtomicU64>,
    },
    /// A TLS listener (accepted connections become passive TLS streams).
    TlsListen {
        id: u64,
        listener: MioListener,
        sink: MailboxSink,
        subscriber: Arc<AtomicU64>,
        config: Arc<ServerConfig>,
    },
    /// A one-shot TLS client exchange (from `tls-request`): handshake, send
    /// `request`, stream the response, `[:tcp-closed]` at EOF.
    TlsClient {
        id: u64,
        stream: MioStream,
        sink: MailboxSink,
        binary: Arc<AtomicBool>,
        server_name: ServerName<'static>,
        request: Vec<u8>,
        config: Arc<ClientConfig>,
    },
    /// Start reading a passive (accepted) stream — the claim half of
    /// `tcp-controlling-process` (the subscriber cell is retargeted control-side).
    Claim { id: u64 },
    /// Queue outbound bytes.
    Send { id: u64, bytes: Vec<u8> },
    /// Arm/disarm an established stream's idle timeout (`ms` = 0 disarms).
    SetIdle { id: u64, ms: u64 },
    /// Flush queued outbound (bounded by [`LINGER`]) and close.
    Close { id: u64 },
}

struct Reactor {
    tx: Sender<Cmd>,
    waker: Waker,
}

impl Reactor {
    fn cmd(&self, cmd: Cmd) {
        // The reactor thread lives for the process; a send can only fail if it
        // panicked, in which case every socket is dead anyway.
        let _ = self.tx.send(cmd);
        let _ = self.waker.wake();
    }
}

/// The reactor singleton, started on first socket use.
fn reactor() -> &'static Reactor {
    static R: OnceLock<Reactor> = OnceLock::new();
    R.get_or_init(|| {
        let poll = Poll::new().expect("net reactor: mio poll");
        let waker = Waker::new(poll.registry(), WAKER_TOKEN).expect("net reactor: waker");
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("brood-net-reactor".into())
            .spawn(move || reactor_loop(poll, rx))
            .expect("spawn net reactor thread");
        Reactor { tx, waker }
    })
}

// ---- message builders (off-heap; symbols are a global interner) ----

fn tcp_data_msg(id: u64, payload: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp")),
        Message::Socket(id),
        payload,
    ])
}

fn tcp_closed_msg(id: u64) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-closed")),
        Message::Socket(id),
    ])
}

fn tcp_accept_msg(lid: u64, cid: u64) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-accept")),
        Message::Socket(lid),
        Message::Socket(cid),
    ])
}

fn tcp_error_msg(id: u64, msg: &str) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-error")),
        Message::Socket(id),
        Message::Str(msg.to_string()),
    ])
}

// ---- the data plane: per-socket reactor state ----

/// Outbound queue: chunks + a head offset (the first chunk may be part-written).
struct OutQ {
    chunks: std::collections::VecDeque<Vec<u8>>,
    head_off: usize,
    total: usize,
}

impl OutQ {
    fn new() -> OutQ {
        OutQ {
            chunks: std::collections::VecDeque::new(),
            head_off: 0,
            total: 0,
        }
    }
    fn push(&mut self, bytes: Vec<u8>) {
        self.total += bytes.len();
        self.chunks.push_back(bytes);
    }
    fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
    /// Write as much as the sink accepts. Ok(true) = fully drained.
    fn flush_into(&mut self, w: &mut impl Write) -> std::io::Result<bool> {
        while let Some(front) = self.chunks.front() {
            match w.write(&front[self.head_off..]) {
                Ok(n) => {
                    self.head_off += n;
                    self.total -= n;
                    if self.head_off >= front.len() {
                        self.chunks.pop_front();
                        self.head_off = 0;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(true)
    }
}

/// One plaintext stream's reactor state.
struct PlainConn {
    stream: MioStream,
    sink: MailboxSink,
    binary: Arc<AtomicBool>,
    carry: Vec<u8>,
    out: OutQ,
    /// Reads started (active connect or claimed accept).
    reading: bool,
    /// The read side has ended (EOF/error) and `[:tcp-closed]` was emitted.
    read_done: bool,
    /// `Some(when-accepted)` while passive & unclaimed — the reaper's stamp.
    accepted_at: Option<Instant>,
    /// `Some(deadline)` once `Close` arrived: flush until then, then drop.
    closing: Option<Instant>,
    registered: bool,
    /// Opt-in idle bound (`tcp-set-idle-timeout`, default off). When `Some`, the
    /// reactor drops the connection if no bytes move in either direction for this
    /// long — slow-loris protection a raw-TCP server can arm on a connection it
    /// accepts. Off by default so a legitimately long-idle stream (SSE, long-poll,
    /// the editor daemon) is never reaped.
    idle: Option<Duration>,
    /// Last time bytes moved (inbound read or outbound `Send`) — the idle stamp.
    last_activity: Instant,
}

/// One TLS stream's reactor state — the same machine drives a server
/// connection (accepted via `tls-listen`) and a one-shot client exchange
/// (`tls-request`); rustls's `Connection` deref-target covers both.
struct TlsConn {
    stream: MioStream,
    conn: rustls::Connection,
    sink: MailboxSink,
    binary: Arc<AtomicBool>,
    carry: Vec<u8>,
    read_done: bool,
    closing: Option<Instant>,
    registered: bool,
    /// Plaintext bytes handed to the rustls writer since it was last fully
    /// flushed to the socket — the TLS counterpart of the plaintext `OutQ.total`.
    /// rustls's writer buffers without bound, so a stuck TLS reader would grow
    /// its `sendable_tls` unboundedly; when this exceeds [`OUT_CAP`] while the
    /// socket is backed up, the connection is dropped (the same slow-reader
    /// bound the plaintext path enforces). Reset to 0 once `wants_write()` is
    /// false (everything drained).
    pending_out: usize,
    /// `tls-request` semantics: errors emit `[:tcp-error]` instead of
    /// `[:tcp-closed]`, and a missing close_notify at EOF is tolerated.
    one_shot: bool,
    /// `Some(deadline)` while the handshake is still in progress; cleared to
    /// `None` the first tick after `is_handshaking()` goes false. The reactor
    /// drops the connection if the handshake hasn't completed by then
    /// ([`HANDSHAKE_TIMEOUT`]).
    handshake_deadline: Option<Instant>,
    /// Opt-in idle bound (`tcp-set-idle-timeout`, default off) — see
    /// [`PlainConn::idle`]. Applies once the handshake is complete.
    idle: Option<Duration>,
    /// Last time bytes moved (inbound plaintext or outbound `Send`).
    last_activity: Instant,
}

/// A passive accepted TLS connection: raw materials until claimed.
struct TlsPending {
    stream: MioStream,
    config: Arc<ServerConfig>,
    sink: MailboxSink,
    binary: Arc<AtomicBool>,
    accepted_at: Instant,
}

// The `Tls` variant (rustls state) is much larger than the others, but the
// reactor holds exactly one `Rx` per live socket and a TLS connection genuinely
// needs that state inline — boxing would just add an indirection on the hot
// read/write path for no real saving.
#[allow(clippy::large_enum_variant)]
enum Rx {
    Plain(PlainConn),
    Tls(TlsConn),
    TlsPending(TlsPending),
    Listener {
        listener: MioListener,
        sink: MailboxSink,
        subscriber: Arc<AtomicU64>,
        registered: bool,
    },
    TlsListener {
        listener: MioListener,
        sink: MailboxSink,
        subscriber: Arc<AtomicU64>,
        config: Arc<ServerConfig>,
        registered: bool,
    },
}

// ---- the reactor loop ----

fn reactor_loop(mut poll: Poll, rx: Receiver<Cmd>) {
    let registry = poll
        .registry()
        .try_clone()
        .expect("net reactor: registry clone");
    let mut events = Events::with_capacity(1024);
    let mut conns: HashMap<u64, Rx> = HashMap::new();
    // Accepted connections are staged here during event handling (a listener's
    // event handler can't insert into `conns` while it holds a `conns` borrow)
    // and inserted + announced right after.
    let mut accepted: Vec<(u64, Rx, Message, MailboxSink)> = Vec::new();

    loop {
        if let Err(e) = poll.poll(&mut events, Some(TICK)) {
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // A dead poll means no socket can ever progress again; nothing
            // useful to do beyond stopping the thread.
            return;
        }

        for event in events.iter() {
            let token = event.token();
            if token == WAKER_TOKEN {
                continue; // commands drained below
            }
            let id = token.0 as u64;
            let readable = event.is_readable();
            let writable = event.is_writable();
            let remove = match conns.get_mut(&id) {
                Some(rx) => drive(id, rx, readable, writable, &registry, &mut accepted),
                None => false,
            };
            if remove {
                teardown(id, &mut conns, &registry);
            }
            // Park the staged accepts, then announce them — the entry must be
            // in place before the owner can react to `[:tcp-accept …]`.
            for (cid, entry, msg, lsink) in accepted.drain(..) {
                conns.insert(cid, entry);
                lsink.emit(msg);
            }
        }

        // Commands (registrations, sends, claims, closes).
        while let Ok(cmd) = rx.try_recv() {
            handle_cmd(cmd, &mut conns, &registry);
        }

        // Housekeeping: reap unclaimed accepts, expire lingering closes.
        housekeep(&mut conns, &registry);
    }
}

/// Drive one socket's readiness. Returns true when the entry must be removed.
fn drive(
    id: u64,
    rx: &mut Rx,
    readable: bool,
    writable: bool,
    registry: &mio::Registry,
    accepted: &mut Vec<(u64, Rx, Message, MailboxSink)>,
) -> bool {
    match rx {
        Rx::Listener {
            listener,
            sink,
            subscriber,
            ..
        } => {
            if readable {
                accept_ready(id, listener, sink, subscriber, None, accepted);
            }
            false
        }
        Rx::TlsListener {
            listener,
            sink,
            subscriber,
            config,
            ..
        } => {
            if readable {
                accept_ready(
                    id,
                    listener,
                    sink,
                    subscriber,
                    Some(config.clone()),
                    accepted,
                );
            }
            false
        }
        Rx::Plain(c) => drive_plain(id, c, readable, writable, registry),
        Rx::Tls(c) => drive_tls(id, c, readable, writable, registry),
        Rx::TlsPending(_) => false,
    }
}

/// Accept every waiting connection on a ready listener; new sockets are
/// registered control-side and announced, then parked passive in the reactor.
fn accept_ready(
    lid: u64,
    listener: &mut MioListener,
    sink: &MailboxSink,
    subscriber: &Arc<AtomicU64>,
    tls: Option<Arc<ServerConfig>>,
    out: &mut Vec<(u64, Rx, Message, MailboxSink)>,
) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let cid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let owner = subscriber.load(Ordering::Acquire);
                let binary = Arc::new(AtomicBool::new(false));
                let (csink, ccell) = sink_pair(owner);
                let port = stream.local_addr().ok().map(|a| a.port());
                reg().insert(
                    cid,
                    Ctl {
                        kind: if tls.is_some() {
                            Kind::TlsStream
                        } else {
                            Kind::Stream
                        },
                        owner,
                        binary: binary.clone(),
                        subscriber: ccell,
                        claimed: false,
                        port,
                    },
                );
                let entry = match &tls {
                    Some(config) => Rx::TlsPending(TlsPending {
                        stream,
                        config: config.clone(),
                        sink: csink,
                        binary,
                        accepted_at: Instant::now(),
                    }),
                    None => Rx::Plain(PlainConn {
                        stream,
                        sink: csink,
                        binary,
                        carry: Vec::new(),
                        out: OutQ::new(),
                        reading: false,
                        read_done: false,
                        accepted_at: Some(Instant::now()),
                        closing: None,
                        registered: false,
                        idle: None,
                        last_activity: Instant::now(),
                    }),
                };
                out.push((cid, entry, tcp_accept_msg(lid, cid), sink.clone()));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Desired poll interests for a plaintext connection; `None` = deregister.
fn plain_interests(c: &PlainConn) -> Option<Interest> {
    let want_read = c.reading && !c.read_done;
    let want_write = !c.out.is_empty();
    match (want_read, want_write) {
        (true, true) => Some(Interest::READABLE.add(Interest::WRITABLE)),
        (true, false) => Some(Interest::READABLE),
        (false, true) => Some(Interest::WRITABLE),
        (false, false) => None,
    }
}

fn sync_plain_registration(id: u64, c: &mut PlainConn, registry: &mio::Registry) {
    match plain_interests(c) {
        Some(interests) => {
            let res = if c.registered {
                registry.reregister(&mut c.stream, Token(id as usize), interests)
            } else {
                registry.register(&mut c.stream, Token(id as usize), interests)
            };
            if res.is_ok() {
                c.registered = true;
            }
        }
        None => {
            if c.registered {
                let _ = registry.deregister(&mut c.stream);
                c.registered = false;
            }
        }
    }
}

/// Returns true when the connection should be torn down.
fn drive_plain(
    id: u64,
    c: &mut PlainConn,
    readable: bool,
    writable: bool,
    registry: &mio::Registry,
) -> bool {
    if writable || !c.out.is_empty() {
        let before = c.out.total;
        match c.out.flush_into(&mut c.stream) {
            Ok(_) => {}
            Err(_) => {
                if c.reading && !c.read_done {
                    c.sink.emit(tcp_closed_msg(id));
                    c.read_done = true;
                }
                return true;
            }
        }
        // Outbound bytes actually left the queue — count it as activity so a large
        // response draining to a slow reader isn't idle-reaped mid-send.
        if c.out.total < before {
            c.last_activity = Instant::now();
        }
        if c.out.is_empty() && c.closing.is_some() {
            if c.reading && !c.read_done {
                c.sink.emit(tcp_closed_msg(id));
                c.read_done = true;
            }
            return true;
        }
    }
    if readable && c.reading && !c.read_done {
        let mut buf = [0u8; 65536];
        loop {
            match c.stream.read(&mut buf) {
                Ok(0) => {
                    if let Some(p) = chunk_flush(&mut c.carry) {
                        c.sink.emit(tcp_data_msg(id, p));
                    }
                    c.sink.emit(tcp_closed_msg(id));
                    c.read_done = true;
                    break;
                }
                Ok(n) => {
                    c.last_activity = Instant::now();
                    let bin = c.binary.load(Ordering::Acquire);
                    if let Some(p) = chunk_payload(&mut c.carry, &buf[..n], bin) {
                        c.sink.emit(tcp_data_msg(id, p));
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    c.sink.emit(tcp_closed_msg(id));
                    c.read_done = true;
                    break;
                }
            }
        }
    }
    // NOTE deliberately no auto-teardown on read EOF: a peer half-close leaves
    // the write side usable (Erlang semantics — the request-then-FIN client
    // still gets its response). The entry lives until an explicit `Close`, a
    // write failure, the OUT_CAP breach, the linger deadline, or owner death.
    if c.read_done && c.out.is_empty() && c.closing.is_some() {
        return true;
    }
    sync_plain_registration(id, c, registry);
    false
}

fn tls_interests(c: &TlsConn) -> Option<Interest> {
    let want_read = !c.read_done;
    let want_write = c.conn.wants_write();
    match (want_read, want_write) {
        (true, true) => Some(Interest::READABLE.add(Interest::WRITABLE)),
        (true, false) => Some(Interest::READABLE),
        (false, true) => Some(Interest::WRITABLE),
        (false, false) => None,
    }
}

fn sync_tls_registration(id: u64, c: &mut TlsConn, registry: &mio::Registry) {
    match tls_interests(c) {
        Some(interests) => {
            let res = if c.registered {
                registry.reregister(&mut c.stream, Token(id as usize), interests)
            } else {
                registry.register(&mut c.stream, Token(id as usize), interests)
            };
            if res.is_ok() {
                c.registered = true;
            }
        }
        None => {
            if c.registered {
                let _ = registry.deregister(&mut c.stream);
                c.registered = false;
            }
        }
    }
}

/// Finish a TLS connection: emit the right terminal message once. Returns true.
fn tls_finish(id: u64, c: &mut TlsConn, error: Option<String>) -> bool {
    if let Some(p) = chunk_flush(&mut c.carry) {
        c.sink.emit(tcp_data_msg(id, p));
    }
    if !c.read_done {
        match error {
            Some(msg) if c.one_shot => c.sink.emit(tcp_error_msg(id, &msg)),
            _ => c.sink.emit(tcp_closed_msg(id)),
        }
        c.read_done = true;
    }
    // Best-effort close_notify + flush.
    if !c.conn.is_handshaking() {
        c.conn.send_close_notify();
    }
    while c.conn.wants_write() {
        match c.conn.write_tls(&mut c.stream) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    true
}

/// Returns true when the connection should be torn down.
fn drive_tls(
    id: u64,
    c: &mut TlsConn,
    readable: bool,
    writable: bool,
    registry: &mio::Registry,
) -> bool {
    // Outbound: flush pending TLS records (handshake output + app data).
    if writable || c.conn.wants_write() {
        while c.conn.wants_write() {
            match c.conn.write_tls(&mut c.stream) {
                Ok(0) => return tls_finish(id, c, Some("tls: connection closed".into())),
                // Bytes left for the peer — outbound progress counts as activity so
                // a large response draining to a slow reader isn't idle-reaped.
                Ok(_) => c.last_activity = Instant::now(),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return tls_finish(id, c, Some(format!("tls: {e}"))),
            }
        }
        // Fully drained to the socket → the OUT_CAP accounting resets (rustls's
        // buffer is empty again).
        if !c.conn.wants_write() {
            c.pending_out = 0;
        }
        if c.closing.is_some() && !c.conn.wants_write() {
            return tls_finish(id, c, None);
        }
    }
    if readable && !c.read_done {
        loop {
            match c.conn.read_tls(&mut c.stream) {
                Ok(0) => {
                    // Peer closed the TCP connection. One-shot clients tolerate
                    // a missing close_notify (many servers just drop).
                    return tls_finish(id, c, None);
                }
                Ok(_) => match c.conn.process_new_packets() {
                    Ok(io) => {
                        let n = io.plaintext_bytes_to_read();
                        if n > 0 {
                            let mut buf = vec![0u8; n];
                            let mut got = 0;
                            while got < n {
                                match c.conn.reader().read(&mut buf[got..]) {
                                    Ok(0) => break,
                                    Ok(m) => got += m,
                                    Err(_) => break,
                                }
                            }
                            if got > 0 {
                                c.last_activity = Instant::now();
                                let bin = c.binary.load(Ordering::Acquire);
                                if let Some(p) = chunk_payload(&mut c.carry, &buf[..got], bin) {
                                    c.sink.emit(tcp_data_msg(id, p));
                                }
                            }
                        }
                        if io.peer_has_closed() {
                            return tls_finish(id, c, None);
                        }
                    }
                    Err(e) => return tls_finish(id, c, Some(format!("tls: {e}"))),
                },
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof && c.one_shot => {
                    return tls_finish(id, c, None);
                }
                Err(e) => return tls_finish(id, c, Some(format!("tls: {e}"))),
            }
        }
    }
    // Handshake done → retire its deadline so the reactor stops watching it. (An
    // established connection may legitimately idle for a long time — the idle
    // bound below is opt-in, never this.) Start the idle clock from *here*, not
    // from creation: the handshake window must not count against an idle bound
    // armed at claim time.
    if c.handshake_deadline.is_some() && !c.conn.is_handshaking() {
        c.handshake_deadline = None;
        c.last_activity = Instant::now();
    }
    sync_tls_registration(id, c, registry);
    false
}

fn handle_cmd(cmd: Cmd, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    match cmd {
        Cmd::Stream {
            id,
            stream,
            sink,
            binary,
        } => {
            let mut c = PlainConn {
                stream,
                sink,
                binary,
                carry: Vec::new(),
                out: OutQ::new(),
                reading: true,
                read_done: false,
                accepted_at: None,
                closing: None,
                registered: false,
                idle: None,
                last_activity: Instant::now(),
            };
            sync_plain_registration(id, &mut c, registry);
            conns.insert(id, Rx::Plain(c));
        }
        Cmd::Listen {
            id,
            mut listener,
            sink,
            subscriber,
        } => {
            let ok = registry
                .register(&mut listener, Token(id as usize), Interest::READABLE)
                .is_ok();
            conns.insert(
                id,
                Rx::Listener {
                    listener,
                    sink,
                    subscriber,
                    registered: ok,
                },
            );
        }
        Cmd::TlsListen {
            id,
            mut listener,
            sink,
            subscriber,
            config,
        } => {
            let ok = registry
                .register(&mut listener, Token(id as usize), Interest::READABLE)
                .is_ok();
            conns.insert(
                id,
                Rx::TlsListener {
                    listener,
                    sink,
                    subscriber,
                    config,
                    registered: ok,
                },
            );
        }
        Cmd::TlsClient {
            id,
            stream,
            sink,
            binary,
            server_name,
            request,
            config,
        } => {
            match ClientConnection::new(config, server_name) {
                Ok(mut conn) => {
                    // The request is buffered as plaintext now; rustls emits it
                    // once the handshake completes.
                    let _ = conn.writer().write_all(&request);
                    let mut c = TlsConn {
                        stream,
                        conn: rustls::Connection::Client(conn),
                        sink,
                        binary,
                        carry: Vec::new(),
                        read_done: false,
                        closing: None,
                        registered: false,
                        pending_out: 0,
                        one_shot: true,
                        handshake_deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
                        idle: None,
                        last_activity: Instant::now(),
                    };
                    sync_tls_registration(id, &mut c, registry);
                    conns.insert(id, Rx::Tls(c));
                }
                Err(e) => {
                    sink.emit(tcp_error_msg(id, &format!("tls: {e}")));
                    reg().remove(&id);
                }
            }
        }
        Cmd::Claim { id } => {
            match conns.remove(&id) {
                Some(Rx::Plain(mut c)) => {
                    c.reading = true;
                    c.accepted_at = None;
                    // Start the idle clock from establishment, not from accept/arm:
                    // any wait while passive & unclaimed must not count against an
                    // idle bound armed before the claim.
                    c.last_activity = Instant::now();
                    sync_plain_registration(id, &mut c, registry);
                    conns.insert(id, Rx::Plain(c));
                }
                Some(Rx::TlsPending(p)) => match ServerConnection::new(p.config) {
                    Ok(conn) => {
                        let mut c = TlsConn {
                            stream: p.stream,
                            conn: rustls::Connection::Server(conn),
                            sink: p.sink,
                            binary: p.binary,
                            carry: Vec::new(),
                            read_done: false,
                            closing: None,
                            registered: false,
                            pending_out: 0,
                            one_shot: false,
                            handshake_deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
                            idle: None,
                            last_activity: Instant::now(),
                        };
                        sync_tls_registration(id, &mut c, registry);
                        conns.insert(id, Rx::Tls(c));
                    }
                    Err(e) => {
                        p.sink.emit(tcp_error_msg(id, &format!("tls: {e}")));
                        reg().remove(&id);
                    }
                },
                Some(other) => {
                    conns.insert(id, other);
                }
                None => {}
            };
        }
        Cmd::Send { id, bytes } => {
            let remove = match conns.get_mut(&id) {
                Some(Rx::Plain(c)) => {
                    if c.out.total + bytes.len() > OUT_CAP {
                        // A stuck reader: drop the connection rather than
                        // buffer without bound. Notify the current subscriber
                        // regardless of whether reads were started (an unclaimed
                        // accepted socket write-bombed here would otherwise drop
                        // silently — Finding 5).
                        if !c.read_done {
                            c.sink.emit(tcp_closed_msg(id));
                            c.read_done = true;
                        }
                        true
                    } else {
                        c.out.push(bytes);
                        c.last_activity = Instant::now();
                        // Try at once — the common case is a writable socket,
                        // and edge-triggered polls only fire on transitions.
                        drive_plain(id, c, false, true, registry)
                    }
                }
                Some(Rx::Tls(c)) => {
                    // Bound the plaintext handed to rustls (whose writer buffers
                    // without limit): once we're backed up (`wants_write`) and
                    // past OUT_CAP, drop rather than grow `sendable_tls` forever.
                    if c.conn.wants_write() && c.pending_out + bytes.len() > OUT_CAP {
                        if !c.read_done {
                            if c.one_shot {
                                c.sink
                                    .emit(tcp_error_msg(id, "tls: outbound buffer overflow"));
                            } else {
                                c.sink.emit(tcp_closed_msg(id));
                            }
                            c.read_done = true;
                        }
                        true
                    } else {
                        c.pending_out += bytes.len();
                        c.last_activity = Instant::now();
                        let _ = c.conn.writer().write_all(&bytes);
                        drive_tls(id, c, false, true, registry)
                    }
                }
                _ => false,
            };
            if remove {
                teardown(id, conns, registry);
            }
        }
        Cmd::SetIdle { id, ms } => {
            let idle = if ms == 0 {
                None
            } else {
                Some(Duration::from_millis(ms))
            };
            match conns.get_mut(&id) {
                Some(Rx::Plain(c)) => {
                    c.idle = idle;
                    c.last_activity = Instant::now();
                }
                Some(Rx::Tls(c)) => {
                    c.idle = idle;
                    c.last_activity = Instant::now();
                }
                // A listener or a not-yet-claimed accept: nothing to arm (an
                // unclaimed accept is already bounded by ACCEPT_REAP_AFTER).
                _ => {}
            }
        }
        Cmd::Close { id } => {
            let remove = match conns.get_mut(&id) {
                Some(Rx::Plain(c)) => {
                    if c.out.is_empty() {
                        true
                    } else {
                        c.closing = Some(Instant::now() + LINGER);
                        drive_plain(id, c, false, true, registry)
                    }
                }
                Some(Rx::Tls(c)) => {
                    c.closing = Some(Instant::now() + LINGER);
                    if !c.conn.wants_write() {
                        tls_finish(id, c, None)
                    } else {
                        drive_tls(id, c, false, true, registry)
                    }
                }
                Some(Rx::TlsPending(_))
                | Some(Rx::Listener { .. })
                | Some(Rx::TlsListener { .. }) => true,
                None => false,
            };
            if remove {
                teardown(id, conns, registry);
            }
        }
    }
}

/// Remove a connection from the reactor + poll (control-side entry is the
/// caller's business — `close` already removed it; reaped/errored sockets
/// remove it here).
fn teardown(id: u64, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    if let Some(rx) = conns.remove(&id) {
        match rx {
            Rx::Plain(mut c) => {
                if c.registered {
                    let _ = registry.deregister(&mut c.stream);
                }
            }
            Rx::Tls(mut c) => {
                if c.registered {
                    let _ = registry.deregister(&mut c.stream);
                }
            }
            Rx::TlsPending(_) => {}
            Rx::Listener {
                mut listener,
                registered,
                ..
            }
            | Rx::TlsListener {
                mut listener,
                registered,
                ..
            } => {
                if registered {
                    let _ = registry.deregister(&mut listener);
                }
            }
        }
    }
    reg().remove(&id);
}

fn housekeep(conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    let now = Instant::now();
    // Silent drops (unclaimed accepts, expired lingers — no owner is waiting on a
    // terminal message, or already got one from the `Close` path).
    let mut doomed: Vec<u64> = Vec::new();
    // A stalled TLS handshake: the owner (if any) IS waiting, so emit the terminal
    // message via `tls_finish` before teardown.
    let mut handshake_timeouts: Vec<u64> = Vec::new();
    // Opt-in idle-timeout reaps: the owner armed this and IS waiting, so it gets a
    // terminal message too (plaintext `[:tcp-closed]`, TLS via `tls_finish`).
    let mut idle_reaps: Vec<u64> = Vec::new();
    for (&id, rx) in conns.iter() {
        match rx {
            Rx::Plain(c) => {
                if let Some(t) = c.accepted_at {
                    if now.duration_since(t) >= ACCEPT_REAP_AFTER {
                        doomed.push(id);
                    }
                }
                if let Some(deadline) = c.closing {
                    if now >= deadline {
                        doomed.push(id);
                    }
                } else if let Some(idle) = c.idle {
                    // Established (claimed, still reading) and gone quiet in both
                    // directions past its armed bound.
                    if c.reading && !c.read_done && now.duration_since(c.last_activity) >= idle {
                        idle_reaps.push(id);
                    }
                }
            }
            Rx::TlsPending(p) => {
                if now.duration_since(p.accepted_at) >= ACCEPT_REAP_AFTER {
                    doomed.push(id);
                }
            }
            Rx::Tls(c) => {
                if let Some(deadline) = c.closing {
                    if now >= deadline {
                        doomed.push(id);
                    }
                } else if let Some(hd) = c.handshake_deadline {
                    if now >= hd {
                        handshake_timeouts.push(id);
                    }
                } else if let Some(idle) = c.idle {
                    // Handshake already done (deadline cleared) and idle past bound.
                    if !c.read_done && now.duration_since(c.last_activity) >= idle {
                        idle_reaps.push(id);
                    }
                }
            }
            _ => {}
        }
    }
    for id in handshake_timeouts {
        if let Some(Rx::Tls(c)) = conns.get_mut(&id) {
            tls_finish(id, c, Some("tls: handshake timed out".into()));
        }
        teardown(id, conns, registry);
    }
    for id in idle_reaps {
        match conns.get_mut(&id) {
            Some(Rx::Plain(c)) => {
                if !c.read_done {
                    c.sink.emit(tcp_closed_msg(id));
                    c.read_done = true;
                }
            }
            Some(Rx::Tls(c)) => {
                tls_finish(id, c, None);
            }
            _ => {}
        }
        teardown(id, conns, registry);
    }
    for id in doomed {
        teardown(id, conns, registry);
    }
}

// ---- the primitive operations (control plane) ----

/// `(tcp-connect host port)` — blocking connect (name resolution + TCP on the
/// calling thread, as before); reads start at once, delivering to `subscriber`.
pub fn connect(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    let std_stream = std::net::TcpStream::connect((host, port))?;
    std_stream.set_nonblocking(true)?;
    let local = std_stream.local_addr().ok().map(|a| a.port());
    let stream = MioStream::from_std(std_stream);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(false));
    let (sink, cell) = sink_pair(subscriber);
    reg().insert(
        id,
        Ctl {
            kind: Kind::Stream,
            owner: subscriber,
            binary: binary.clone(),
            subscriber: cell,
            claimed: true,
            port: local,
        },
    );
    reactor().cmd(Cmd::Stream {
        id,
        stream,
        sink,
        binary,
    });
    Ok(id)
}

/// `(tcp-listen host port)` — bind; connections are announced as
/// `[:tcp-accept lid client]` to `subscriber`. Port 0 = OS-assigned.
pub fn listen(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    let std_listener = std::net::TcpListener::bind((host, port))?;
    let local = std_listener.local_addr()?.port();
    std_listener.set_nonblocking(true)?;
    let listener = MioListener::from_std(std_listener);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (sink, cell) = sink_pair(subscriber);
    reg().insert(
        id,
        Ctl {
            kind: Kind::Listener,
            owner: subscriber,
            binary: Arc::new(AtomicBool::new(false)),
            subscriber: cell.clone(),
            claimed: true,
            port: Some(local),
        },
    );
    reactor().cmd(Cmd::Listen {
        id,
        listener,
        sink,
        subscriber: cell,
    });
    Ok(id)
}

/// `(tcp-controlling-process sock pid)` — make `pid` the owner of `sock`'s
/// inbound data. For a passive (just-accepted) socket this **starts** reads;
/// for an already-active socket it retargets delivery.
pub fn controlling_process(id: u64, pid: u64) -> std::io::Result<()> {
    let claim = {
        let mut reg = reg();
        match reg.get_mut(&id) {
            Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream) => {
                ctl.subscriber.store(pid, Ordering::Release);
                ctl.owner = pid;
                let was_claimed = ctl.claimed;
                ctl.claimed = true;
                !was_claimed
            }
            Some(_) => {
                return Err(invalid(
                    "tcp-controlling-process: socket is a listener, not a stream",
                ))
            }
            None => return Err(bad_socket()),
        }
    };
    if claim {
        reactor().cmd(Cmd::Claim { id });
    }
    Ok(())
}

/// `(tcp-set-binary sock on)` — switch `sock`'s **inbound decode** between text
/// mode (default: UTF-8 strings) and binary mode (byte-faithful `bytes`
/// values). Outbound is unaffected (ADR-141). Takes effect for the next
/// inbound chunk. Errors if `sock` is gone or a listener.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let reg = reg();
    match reg.get(&id) {
        Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream) => {
            ctl.binary.store(on, Ordering::Release);
            Ok(())
        }
        Some(_) => Err(invalid(
            "tcp-set-binary: socket is a listener, not a stream",
        )),
        None => Err(bad_socket()),
    }
}

/// `(tcp-set-idle-timeout sock ms)` — arm (or, with `ms` 0, disarm) an idle
/// timeout on an established stream. The reactor drops the connection if no bytes
/// move in **either** direction for `ms` milliseconds, delivering `[:tcp-closed]`
/// (or `[:tcp-error]` for a one-shot TLS client). **Off by default** — arm it on a
/// connection accepting untrusted input (slow-loris protection the reactor applies
/// even if the app forgets to close); leave it off for a legitimately long-idle
/// stream (SSE, long-poll, the editor daemon). No-op if the socket is already
/// gone by the time the reactor applies it; errors now if it's a listener.
pub fn set_idle_timeout(id: u64, ms: u64) -> std::io::Result<()> {
    {
        let reg = reg();
        match reg.get(&id) {
            Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream) => {}
            Some(_) => {
                return Err(invalid(
                    "tcp-set-idle-timeout: socket is a listener, not a stream",
                ))
            }
            None => return Err(bad_socket()),
        }
    }
    reactor().cmd(Cmd::SetIdle { id, ms });
    Ok(())
}

/// `(tcp-send sock data)` — queue `data` for the reactor to write (ADR-143:
/// asynchronous; a write failure surfaces later as `[:tcp-closed …]`, and a
/// queue past [`OUT_CAP`] drops the connection). Erroring cases the caller can
/// know now — unknown socket, a listener, an unclaimed TLS stream — still
/// error synchronously.
pub fn send(id: u64, data: &[u8]) -> std::io::Result<()> {
    {
        let reg = reg();
        match reg.get(&id) {
            Some(ctl) if ctl.kind == Kind::Stream => {}
            Some(ctl) if ctl.kind == Kind::TlsStream => {
                if !ctl.claimed {
                    return Err(invalid(
                        "tcp-send: TLS socket not yet claimed (tcp-controlling-process)",
                    ));
                }
            }
            Some(_) => return Err(invalid("tcp-send: socket is a listener, not a stream")),
            None => return Err(bad_socket()),
        }
    }
    reactor().cmd(Cmd::Send {
        id,
        bytes: data.to_vec(),
    });
    Ok(())
}

/// `(tcp-close sock)` — flush queued outbound (bounded by [`LINGER`]), then
/// close; stops a listener's accepts. Idempotent.
pub fn close(id: u64) {
    let known = reg().remove(&id).is_some();
    if known {
        reactor().cmd(Cmd::Close { id });
    }
}

/// Close every socket owned by green-process `pid` (scheduler `deregister`):
/// a dead owner never leaks fds or registry slots.
pub fn close_process_sockets(pid: u64) {
    let doomed: Vec<u64> = {
        let mut reg = reg();
        let ids: Vec<u64> = reg
            .iter()
            .filter_map(|(&id, ctl)| if ctl.owner == pid { Some(id) } else { None })
            .collect();
        for id in &ids {
            reg.remove(id);
        }
        ids
    };
    for id in doomed {
        reactor().cmd(Cmd::Close { id });
    }
}

/// The local port `sock` is bound to.
pub fn local_port(id: u64) -> Option<u16> {
    reg().get(&id).and_then(|ctl| ctl.port)
}

// ---- TLS configuration + entry points ----

/// The shared client TLS config (Mozilla roots via webpki-roots), built once.
fn tls_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    })
    .clone()
}

/// A client config trusting exactly the given PEM CA/certificate — for private
/// CAs and for talking to a `tls-self-signed` dev server (also what makes the
/// TLS loop testable end-to-end in-tree).
fn tls_config_with_ca(ca_pem: &str) -> std::io::Result<Arc<ClientConfig>> {
    let mut rd = ca_pem.as_bytes();
    let certs = rustls_pemfile::certs(&mut rd)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid(&format!("tls: bad CA PEM: {e}")))?;
    if certs.is_empty() {
        return Err(invalid("tls: no certificates in CA PEM"));
    }
    let mut roots = RootCertStore::empty();
    for cert in certs {
        roots
            .add(cert)
            .map_err(|e| invalid(&format!("tls: bad CA certificate: {e}")))?;
    }
    Ok(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// `(tls-request host port request [ca-pem])` — one HTTPS exchange: handshake,
/// send `request` (already-flattened iolist bytes), stream the response as
/// `[:tcp id data]` … `[:tcp-closed id]` (or `[:tcp-error id msg]`). Returns
/// the id immediately; the blocking name-resolution + connect happens on a
/// short-lived helper thread, then the exchange rides the reactor. The socket
/// honors `tcp-set-binary` like any other (set it right after this returns —
/// nothing can arrive before the request is sent). `ca_pem` (private CAs, dev
/// certs) replaces the Mozilla roots as the trust anchor for this request.
pub fn tls_request(
    host: &str,
    port: u16,
    request: Vec<u8>,
    ca_pem: Option<String>,
    subscriber: u64,
) -> std::io::Result<u64> {
    let config = match ca_pem {
        Some(pem) => tls_config_with_ca(&pem)?,
        None => tls_config(),
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(false));
    let (sink, cell) = sink_pair(subscriber);
    reg().insert(
        id,
        Ctl {
            kind: Kind::TlsStream,
            owner: subscriber,
            binary: binary.clone(),
            subscriber: cell,
            claimed: true,
            port: None,
        },
    );
    let host = host.to_string();
    std::thread::Builder::new()
        .name("brood-tls-connect".into())
        .spawn(move || {
            let server_name = match ServerName::try_from(host.clone()) {
                Ok(n) => n,
                Err(_) => {
                    sink.emit(tcp_error_msg(id, "tls: invalid server name"));
                    reg().remove(&id);
                    return;
                }
            };
            match std::net::TcpStream::connect((host.as_str(), port)) {
                Ok(std_stream) => {
                    if std_stream.set_nonblocking(true).is_err() {
                        sink.emit(tcp_error_msg(id, "tls: could not configure socket"));
                        reg().remove(&id);
                        return;
                    }
                    let stream = MioStream::from_std(std_stream);
                    reactor().cmd(Cmd::TlsClient {
                        id,
                        stream,
                        sink,
                        binary,
                        server_name,
                        request,
                        config,
                    });
                }
                Err(e) => {
                    sink.emit(tcp_error_msg(id, &e.to_string()));
                    reg().remove(&id);
                }
            }
        })
        .expect("spawn tls connect thread");
    Ok(id)
}

/// Build a rustls `ServerConfig` from a PEM certificate chain + private key (the app
/// supplies them; reading files/secrets is Brood-side policy).
fn build_server_config(cert_pem: &str, key_pem: &str) -> std::io::Result<Arc<ServerConfig>> {
    let mut cert_rd = cert_pem.as_bytes();
    let certs = rustls_pemfile::certs(&mut cert_rd)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid(&format!("tls: bad certificate PEM: {e}")))?;
    if certs.is_empty() {
        return Err(invalid("tls: no certificates in cert PEM"));
    }
    let mut key_rd = key_pem.as_bytes();
    let key = rustls_pemfile::private_key(&mut key_rd)
        .map_err(|e| invalid(&format!("tls: bad key PEM: {e}")))?
        .ok_or_else(|| invalid("tls: no private key in key PEM"))?;
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map(Arc::new)
        .map_err(|e| invalid(&format!("tls: {e}")))
}

/// `(tls-self-signed names)` — generate a self-signed certificate + private key (PEM)
/// for the given DNS `names` (e.g. `["localhost"]`). For zero-config dev TLS: pair it
/// with `tls-listen`. Not for production.
pub fn tls_self_signed(names: Vec<String>) -> std::io::Result<(String, String)> {
    let ck = rcgen::generate_simple_self_signed(names)
        .map_err(|e| invalid(&format!("tls: self-signed cert generation failed: {e}")))?;
    Ok((ck.cert.pem(), ck.signing_key.serialize_pem()))
}

/// `(tls-listen host port cert-pem key-pem)` — bind a TLS listener. Accepted
/// connections are announced via `[:tcp-accept lid client]` just like
/// `tcp-listen`; each accepted socket transparently decrypts inbound to
/// `[:tcp id data]` and encrypts `tcp-send`. Port 0 = OS-assigned.
pub fn tls_listen(
    host: &str,
    port: u16,
    cert_pem: &str,
    key_pem: &str,
    subscriber: u64,
) -> std::io::Result<u64> {
    let config = build_server_config(cert_pem, key_pem)?;
    let std_listener = std::net::TcpListener::bind((host, port))?;
    let local = std_listener.local_addr()?.port();
    std_listener.set_nonblocking(true)?;
    let listener = MioListener::from_std(std_listener);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (sink, cell) = sink_pair(subscriber);
    reg().insert(
        id,
        Ctl {
            kind: Kind::TlsListener,
            owner: subscriber,
            binary: Arc::new(AtomicBool::new(false)),
            subscriber: cell.clone(),
            claimed: true,
            port: Some(local),
        },
    );
    reactor().cmd(Cmd::TlsListen {
        id,
        listener,
        sink,
        subscriber: cell,
        config,
    });
    Ok(id)
}

fn invalid(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
}

fn bad_socket() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no such socket (already closed?)",
    )
}
