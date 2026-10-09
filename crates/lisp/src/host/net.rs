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
//! **Reads are flow-controlled** too, with no API change: each `[:tcp sock data]`
//! chunk carries a credit against its socket's [`InboundFlow`], returned when the
//! owner's mailbox lets go of the message. Past [`INBOUND_HIGH_WATER`] undelivered
//! the reactor stops reading that socket, so the kernel buffer fills and TCP slows
//! the peer; it reads again once the owner has taken it below
//! [`INBOUND_LOW_WATER`]. A peer can no longer grow its owner's mailbox without
//! bound by sending faster than the owner consumes.
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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;
use web_time::Instant;

use mio::net::{TcpListener as MioListener, TcpStream as MioStream};
use mio::{Events, Interest, Poll, Token, Waker};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

use crate::core::value;
use crate::process::{
    chunk_flush, chunk_payload, deliver_credited, sink_pair, CreditSource, DeliveryCredit,
    MailboxSink, Message,
};

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

/// Inbound backpressure: how much a reading socket may have delivered to its owner's
/// mailbox and not yet had taken before the reactor stops reading it. Reads used to
/// post every chunk the moment the socket was readable, so a peer sending faster than
/// its owner consumed grew that mailbox without bound — a memory DoS on any server.
/// Past this mark the socket's read interest is dropped; the kernel receive buffer
/// fills and TCP flow control slows the peer. Measured in [`InboundFlow`] cost (payload
/// bytes plus [`MESSAGE_COST_OVERHEAD`] per chunk), so the bound holds for a peer that
/// sends many tiny segments as well as for one that sends large ones. Overshoot is at
/// most one read (64 KiB).
const INBOUND_HIGH_WATER: usize = 1024 * 1024;

/// Reading resumes once the outstanding cost falls below this. Well under the high-water
/// mark so a consumer taking one chunk at a time does not flip the socket's registration
/// per chunk.
const INBOUND_LOW_WATER: usize = 256 * 1024;

/// What one delivered chunk costs beyond its payload bytes — roughly a queued message's
/// envelope plus its three-element vector. Without it a peer dribbling one-byte segments
/// could queue a million messages under a one-megabyte byte budget.
const MESSAGE_COST_OVERHEAD: usize = 256;

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
    crate::core::sync::lock(&REGISTRY)
}

/// True once the reactor thread has exited — a caught panic, or a fatal `poll` error.
/// Nothing restarts it (the mio `Poll`, every fd's registration and all TLS state died
/// with the thread), so the flag makes the death **loud and terminal** instead of
/// silent: before it existed, `Reactor::cmd` discarded the channel error, `tcp-send`
/// kept returning `Ok(())` into a dead channel, and every socket-owning process parked
/// in `receive` forever with no diagnostic anywhere.
static REACTOR_DOWN: AtomicBool = AtomicBool::new(false);

fn reactor_dead_err() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "net reactor thread has died; sockets are unavailable for the rest of this run",
    )
}

/// Gate for every socket operation that would otherwise queue a command into a dead
/// reactor and report success.
fn reactor_up() -> std::io::Result<()> {
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        return Err(reactor_dead_err());
    }
    Ok(())
}

/// The reactor thread's exit hook: mark the reactor dead, say so once on stderr, and
/// fail every registered socket at its owner — an `[:tcp-error …]` naming the cause,
/// then the terminal `[:tcp-closed …]`, so a receive loop driven by either terminates
/// instead of hanging. Ordering matters: the flag is set (SeqCst) **before** the
/// registry drain, and the socket creators re-check it after inserting, so an entry
/// can never slip in behind the sweep and hang its owner.
fn reactor_died(panic: Option<Box<dyn std::any::Any + Send>>) {
    REACTOR_DOWN.store(true, Ordering::SeqCst);
    let why = panic
        .as_ref()
        .map(|p| {
            p.downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked".to_string())
        })
        .unwrap_or_else(|| "fatal poll error".to_string());
    eprintln!(
        "brood: net reactor thread died ({why}); all sockets are being closed and \
         every subsequent tcp-*/tls-* operation will error"
    );
    let entries: Vec<(u64, u64)> = reg()
        .drain()
        .map(|(id, ctl)| (id, ctl.subscriber.load(Ordering::Acquire)))
        .collect();
    for (id, pid) in entries {
        let (sink, _) = sink_pair(pid);
        sink.emit(tcp_error_msg(id, "net reactor died"));
        sink.emit(tcp_closed_msg(id));
    }
}

// ---- commands into the reactor ----

enum Cmd {
    /// A connected plaintext stream (from `tcp-connect`): start reading at once.
    Stream {
        id: u64,
        stream: MioStream,
        sink: MailboxSink,
        subscriber: Arc<AtomicU64>,
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
        subscriber: Arc<AtomicU64>,
        binary: Arc<AtomicBool>,
        server_name: ServerName<'static>,
        request: Vec<u8>,
        config: Arc<ClientConfig>,
    },
    /// Start reading a passive (accepted) stream — the claim half of
    /// `tcp-controlling-process` (the subscriber cell is retargeted control-side).
    Claim { id: u64 },
    /// A claimed stream changed owner (`tcp-controlling-process` again): give it a fresh
    /// [`InboundFlow`], resuming it if the old one held it paused.
    Retarget { id: u64 },
    /// The owner consumed enough of a paused stream's chunks — read it again
    /// ([`InboundFlow::release`]).
    Resume { id: u64 },
    /// Queue outbound bytes.
    Send { id: u64, bytes: Vec<u8> },
    /// Arm/disarm an established stream's idle timeout (`ms` = 0 disarms).
    SetIdle { id: u64, ms: u64 },
    /// Flush queued outbound (bounded by [`LINGER`]) and close.
    Close { id: u64 },
    /// Test-only: panic the reactor thread, to exercise the death hook
    /// (`reactor_died`). Debug builds only; see [`die_for_test`].
    #[cfg(debug_assertions)]
    DieForTest,
}

struct Reactor {
    tx: Sender<Cmd>,
    waker: Waker,
}

impl Reactor {
    fn cmd(&self, cmd: Cmd) {
        // A send fails only once the reactor thread has died; `reactor_died` has
        // then already marked the runtime state (`REACTOR_DOWN`) and failed every
        // socket at its owner, and the public entry points gate on `reactor_up()`,
        // so there is nothing further to report here.
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
            .spawn(move || {
                // The reactor multiplexes EVERY socket in the runtime on this one
                // thread, so its death must be observed, not swallowed: catch a
                // panic (a `poll` error returns normally) and run the death hook —
                // mark the reactor down, fail every socket at its owner, and make
                // subsequent socket ops error instead of silently succeeding.
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    reactor_loop(poll, rx)
                }));
                reactor_died(r.err());
            })
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

// ---- inbound flow control ----

/// One reading socket's inbound budget: the cost of the `[:tcp sock data]` chunks it has
/// delivered to its owner's mailbox that the owner has not yet taken. Every data chunk
/// carries a [`DeliveryCredit`] against this, returned when the message leaves the
/// mailbox by any route — a `receive` takes it (selective or not), the owner dies, the
/// pid is already gone at delivery. The reactor pauses reading at
/// [`INBOUND_HIGH_WATER`]; the release that brings the total below
/// [`INBOUND_LOW_WATER`] sends [`Cmd::Resume`].
///
/// `paused` is the hand-off between the two threads. Exactly one side clears it: the
/// releaser (which then sends `Resume`) or the reactor's own re-check in
/// [`pause_if_full`](InboundFlow::pause_if_full) (which then keeps reading). Both sides
/// write one atomic and read the other, so all four accesses are `SeqCst`.
///
/// `tcp-controlling-process` on an already-reading socket gives it a fresh flow
/// ([`Cmd::Retarget`]): chunks still queued at the previous owner are not the new
/// owner's to consume, and must not hold its socket paused.
struct InboundFlow {
    id: u64,
    outstanding: AtomicUsize,
    paused: AtomicBool,
}

impl InboundFlow {
    fn new(id: u64) -> Arc<InboundFlow> {
        Arc::new(InboundFlow {
            id,
            outstanding: AtomicUsize::new(0),
            paused: AtomicBool::new(false),
        })
    }

    /// Charge one chunk of `bytes` payload and build the credit its message carries.
    fn charge(self: &Arc<Self>, bytes: usize) -> DeliveryCredit {
        let cost = bytes + MESSAGE_COST_OVERHEAD;
        self.outstanding.fetch_add(cost, Ordering::SeqCst);
        DeliveryCredit::new(self.clone(), cost)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Reactor side, after delivering a chunk: true when reading must stop. A consumer
    /// may drain below the low-water mark between our load and our store — it saw
    /// `paused` false and sent nothing — so re-check after publishing the pause and take
    /// it back ourselves if so.
    fn pause_if_full(&self) -> bool {
        if self.outstanding.load(Ordering::SeqCst) < INBOUND_HIGH_WATER {
            return false;
        }
        self.paused.store(true, Ordering::SeqCst);
        if self.outstanding.load(Ordering::SeqCst) < INBOUND_LOW_WATER {
            // Whichever side wins the swap, reading continues: if the releaser won, its
            // `Resume` arrives later and finds a socket already reading (harmless).
            self.paused.swap(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Detach this flow from its socket (ownership moved): its remaining credits return
    /// here harmlessly and never send a `Resume`. Returns whether the socket was paused.
    fn retire(&self) -> bool {
        self.paused.swap(false, Ordering::SeqCst)
    }
}

impl CreditSource for InboundFlow {
    fn release(&self, cost: usize) {
        let before = self.outstanding.fetch_sub(cost, Ordering::SeqCst);
        if before - cost < INBOUND_LOW_WATER
            && self.paused.load(Ordering::SeqCst)
            && self.paused.swap(false, Ordering::SeqCst)
            && !REACTOR_DOWN.load(Ordering::SeqCst)
        {
            reactor().cmd(Cmd::Resume { id: self.id });
        }
    }
}

/// Deliver one inbound data chunk (`bytes` long on the wire) to the socket's current
/// owner, charged against its flow.
fn emit_data(
    id: u64,
    subscriber: &AtomicU64,
    flow: &Arc<InboundFlow>,
    payload: Message,
    bytes: usize,
) {
    let credit = flow.charge(bytes);
    deliver_credited(
        subscriber.load(Ordering::Acquire),
        tcp_data_msg(id, payload),
        credit,
    );
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
    /// The sink's subscriber cell — data chunks are delivered to it directly, with a
    /// flow-control credit ([`emit_data`]).
    subscriber: Arc<AtomicU64>,
    /// Inbound backpressure: chunks delivered and not yet taken ([`InboundFlow`]).
    flow: Arc<InboundFlow>,
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
    /// See [`PlainConn::subscriber`].
    subscriber: Arc<AtomicU64>,
    /// See [`PlainConn::flow`].
    flow: Arc<InboundFlow>,
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
    subscriber: Arc<AtomicU64>,
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
    // The mode every socket accepted here starts in, read once per readiness rather than
    // per connection. An accepted socket is already reading by the time its owner gets
    // `[:tcp-accept …]`, so `tcp-set-binary` on the *stream* cannot close the window: a
    // client that sends immediately can have its first chunk decoded as text before the
    // call lands, and raw bytes then come back as U+FFFD-riddled UTF-8 (KI-102). Setting
    // the mode on the LISTENER has no window, because it is fixed before any connection
    // exists — the shape `gen_tcp:listen(Port, [binary])` has in Erlang.
    let inherited = reg()
        .get(&lid)
        .map(|c| c.binary.load(Ordering::Acquire))
        .unwrap_or(false);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let cid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let owner = subscriber.load(Ordering::Acquire);
                let binary = Arc::new(AtomicBool::new(inherited));
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
                        subscriber: ccell.clone(),
                        claimed: false,
                        port,
                    },
                );
                let entry = match &tls {
                    Some(config) => Rx::TlsPending(TlsPending {
                        stream,
                        config: config.clone(),
                        sink: csink,
                        subscriber: ccell,
                        binary,
                        accepted_at: Instant::now(),
                    }),
                    None => Rx::Plain(PlainConn {
                        stream,
                        sink: csink,
                        subscriber: ccell,
                        flow: InboundFlow::new(cid),
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
            // A connection that died between the readiness event and `accept` — the peer
            // reset it, or the kernel refused it. That is a fact about ONE connection, not
            // about the listener, so skip it and keep draining: the registration is
            // edge-triggered, so breaking here would strand every connection still queued
            // in the backlog until some *later* arrival happened to re-arm us. Silently.
            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionAborted => continue,
            Err(e) => {
                // Anything else ends this drain (a genuinely broken listener, or fd
                // exhaustion where continuing would spin). Say so — the old code returned
                // here with no trace at all, so a listener that had stopped accepting was
                // indistinguishable from one nobody was connecting to. Rate-limiting is
                // unnecessary: reaching this breaks the loop, so it prints at most once per
                // readiness event.
                eprintln!("net: accept on listener {lid} failed ({e}); drain stopped");
                break;
            }
        }
    }
}

/// Desired poll interests for a plaintext connection; `None` = deregister.
fn plain_interests(c: &PlainConn) -> Option<Interest> {
    // A paused socket (inbound backpressure) drops read interest; `Cmd::Resume` re-reads
    // it explicitly, since the edge that would have announced buffered data is spent.
    let want_read = c.reading && !c.read_done && !c.flow.is_paused();
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
    if readable && c.reading && !c.read_done && !c.flow.is_paused() {
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
                        emit_data(id, &c.subscriber, &c.flow, p, n);
                        // Inbound backpressure: stop before WouldBlock. Whatever is left
                        // stays in the kernel buffer (and then the peer's), and
                        // `Cmd::Resume` reads it — in order, EOF included — later.
                        if c.flow.pause_if_full() {
                            break;
                        }
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
    // See `plain_interests` — a paused stream drops read interest.
    let want_read = !c.read_done && !c.flow.is_paused();
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
    if readable && !c.read_done && !c.flow.is_paused() {
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
                                    emit_data(id, &c.subscriber, &c.flow, p, got);
                                }
                            }
                        }
                        if io.peer_has_closed() {
                            return tls_finish(id, c, None);
                        }
                        // Inbound backpressure (see `drive_plain`): every decrypted byte
                        // has been delivered, so stopping here leaves only ciphertext in
                        // the kernel buffer for `Cmd::Resume` to read later.
                        if c.flow.pause_if_full() {
                            break;
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
            subscriber,
            binary,
        } => {
            let mut c = PlainConn {
                stream,
                sink,
                subscriber,
                flow: InboundFlow::new(id),
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
            subscriber,
            binary,
            server_name,
            request,
            config,
        } => {
            match ClientConnection::new(config, server_name) {
                Ok(mut conn) => {
                    // Buffer the whole request as plaintext now; rustls emits it once the
                    // handshake completes. rustls's writer defaults to a 64 KiB buffer cap,
                    // so without lifting it `write_all` silently buffers only the first
                    // ~64 KiB of a larger request and drops the rest (the result is ignored)
                    // — the peer then waits forever for a body that never fully arrives. The
                    // request is already fully in memory, so `None` (no cap) just holds one
                    // copy of it.
                    conn.set_buffer_limit(None);
                    let _ = conn.writer().write_all(&request);
                    let mut c = TlsConn {
                        stream,
                        conn: rustls::Connection::Client(conn),
                        sink,
                        subscriber,
                        flow: InboundFlow::new(id),
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
                    Ok(mut conn) => {
                        // rustls caps its outbound buffer at 64 KiB by default, and
                        // `Cmd::Send` ignores a short `write_all`, so once a slow reader
                        // backed the socket up every byte past that cap was DROPPED,
                        // silently, mid-stream. The bound that belongs here is ours:
                        // `pending_out` against `OUT_CAP` drops the connection loudly
                        // instead (the client path lifts the cap for the same reason).
                        conn.set_buffer_limit(None);
                        let mut c = TlsConn {
                            stream: p.stream,
                            conn: rustls::Connection::Server(conn),
                            sink: p.sink,
                            subscriber: p.subscriber,
                            flow: InboundFlow::new(id),
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
        Cmd::Retarget { id } => {
            let was_paused = match conns.get_mut(&id) {
                Some(Rx::Plain(c)) => {
                    let was_paused = c.flow.retire();
                    c.flow = InboundFlow::new(id);
                    was_paused
                }
                Some(Rx::Tls(c)) => {
                    let was_paused = c.flow.retire();
                    c.flow = InboundFlow::new(id);
                    was_paused
                }
                _ => false,
            };
            if was_paused {
                resume_reading(id, conns, registry);
            }
        }
        Cmd::Resume { id } => resume_reading(id, conns, registry),
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
        #[cfg(debug_assertions)]
        Cmd::DieForTest => panic!("net reactor: test-induced death (Cmd::DieForTest)"),
    }
}

/// Read a stream whose inbound flow is no longer paused. Registration is edge-triggered
/// and the socket stopped reading before `WouldBlock`, so data already buffered in the
/// kernel will never raise a new edge — read it now, as if readiness had fired; the
/// drive then re-registers read interest for whatever arrives after. A stale `Resume`
/// (socket gone, or paused again since) does nothing: the release that next brings the
/// flow below its low-water mark sends another.
fn resume_reading(id: u64, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    let remove = match conns.get_mut(&id) {
        Some(Rx::Plain(c)) if !c.flow.is_paused() => {
            // Waiting on our own owner is not idleness: don't let the time spent paused
            // count against an armed idle timeout.
            c.last_activity = Instant::now();
            drive_plain(id, c, true, false, registry)
        }
        Some(Rx::Tls(c)) if !c.flow.is_paused() => {
            c.last_activity = Instant::now();
            drive_tls(id, c, true, false, registry)
        }
        _ => false,
    };
    if remove {
        teardown(id, conns, registry);
    }
}

/// Test-only trigger for the reactor death hook: panics the reactor thread from a
/// command, exactly like a real bug in the event loop would. Debug builds only —
/// integration tests use it to prove the death is loud (sockets failed at their
/// owners, subsequent ops erroring) rather than a silent hang.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn die_for_test() {
    reactor().cmd(Cmd::DieForTest);
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
                    // Established and gone quiet in both directions past its armed bound.
                    //
                    // `read_done` (the peer sent EOF — a half-close) counts as quiet, and
                    // deliberately so: the old condition required `!c.read_done`, which
                    // excluded a half-closed stream from the only reap that could collect
                    // it. Nothing else covered that state either — `accepted_at` is cleared
                    // once the owner claims the connection, and `closing` is only set by an
                    // explicit close — so a peer that shut down its write half while the
                    // owner never closed the socket left an entry (and its fd) for the life
                    // of the runtime. KI-97 item 4.
                    //
                    // Still gated on nothing being queued outbound: a half-close is a
                    // legitimate "I am done sending, you may still reply", and reaping a
                    // connection with unwritten data would discard that reply.
                    //
                    // A stream paused by inbound backpressure is not idle either: bytes
                    // are waiting on its OWNER, not on the peer, and reaping it would
                    // truncate a stream the owner is still reading.
                    let quiet = now.duration_since(c.last_activity) >= idle;
                    if quiet
                        && c.out.is_empty()
                        && (c.reading || c.read_done)
                        && !c.flow.is_paused()
                    {
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
                    // Not while paused by inbound backpressure (see the plaintext arm).
                    if !c.read_done
                        && !c.flow.is_paused()
                        && now.duration_since(c.last_activity) >= idle
                    {
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

/// How long a `tcp-connect` may stall its worker on the TCP handshake.
///
/// This is the one blocking hole in an otherwise mio-based socket layer, and the
/// caller is a *green process* multiplexed onto ~nproc worker threads — so an
/// unbounded connect does not merely stall the caller, it removes a worker from the
/// pool and with it every other green process scheduled there. Left to the OS, a
/// blackholed host costs ~2 minutes of that. Bounded here so the failure is a prompt,
/// catchable error instead of a silent scheduler brownout. Name resolution below is
/// still blocking (`ToSocketAddrs` has no timeout in std); it is bounded in practice
/// by the resolver's own timeout, and is noted rather than hidden.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// `(tcp-connect host port)` — connect (name resolution + a time-bounded TCP
/// handshake on the calling thread); reads start at once, delivering to `subscriber`.
pub fn connect(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    reactor_up()?;
    // Resolve first so we can use `connect_timeout`, which needs a concrete addr.
    // Try each resolved address in turn, as `TcpStream::connect` does.
    let mut addrs = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?.peekable();
    let mut last_err: Option<std::io::Error> = None;
    let std_stream = loop {
        let Some(addr) = addrs.next() else {
            return Err(last_err.unwrap_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("could not resolve any address for {host}:{port}"),
                )
            }));
        };
        match std::net::TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => break s,
            Err(e) => last_err = Some(e),
        }
    };
    std_stream.set_nonblocking(true)?;
    // Disable Nagle so a large request body isn't paced one delayed-ACK per record
    // (see the TLS client path for the measured impact).
    let _ = std_stream.set_nodelay(true);
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
            subscriber: cell.clone(),
            claimed: true,
            port: local,
        },
    );
    reactor().cmd(Cmd::Stream {
        id,
        stream,
        sink,
        subscriber: cell,
        binary,
    });
    // Closes the race with `reactor_died`'s sweep: the flag is set before the sweep
    // drains the registry, so an entry inserted after the sweep observes it here and
    // withdraws instead of hanging its owner on a socket nothing will ever drive.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
    Ok(id)
}

/// `(tcp-listen host port)` — bind; connections are announced as
/// `[:tcp-accept lid client]` to `subscriber`. Port 0 = OS-assigned.
pub fn listen(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    reactor_up()?;
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
    // See `connect` — closes the race with `reactor_died`'s sweep.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
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
    // A first claim starts reads; a later one moves an already-reading stream to a new
    // owner, whose inbound budget starts fresh (see `InboundFlow`).
    reactor().cmd(if claim {
        Cmd::Claim { id }
    } else {
        Cmd::Retarget { id }
    });
    Ok(())
}

/// `(tcp-set-binary sock on)` — switch `sock`'s **inbound decode** between text
/// mode (default: UTF-8 strings) and binary mode (byte-faithful `bytes`
/// values). Outbound is unaffected (ADR-141).
///
/// On a **stream** this takes effect for the next inbound chunk, which means it cannot be
/// used to make an *accepted* connection binary reliably: the socket is already reading
/// when its owner is handed `[:tcp-accept …]`, so a client that sends immediately can have
/// its first chunk decoded as text first (KI-102 — 256 raw bytes came back as 512, the
/// UTF-8 re-encoding of 128 ASCII plus 128 U+FFFD).
///
/// On a **listener** it sets the mode every socket that listener accepts *starts* in, which
/// has no such window — the listener's mode is fixed before any connection exists. This is
/// the reliable way to run a binary server, and mirrors `gen_tcp:listen(Port, [binary])`.
/// A stream may still switch mode afterwards; inheritance only decides where it starts.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let reg = reg();
    match reg.get(&id) {
        // Every kind: a stream switches its own mode, a listener sets what its accepted
        // sockets start in. `TlsListener` inherits the same way `Listener` does.
        Some(ctl) => {
            ctl.binary.store(on, Ordering::Release);
            Ok(())
        }
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
    reactor_up()?;
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
/// Bound on the TLS client's TCP connect. Matches `dist`'s dial timeout: long enough for
/// any healthy path, short enough that a silently-dropping host does not hold the caller's
/// registry entry and request buffer for the kernel's multi-minute SYN timeout.
const TLS_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn tls_request(
    host: &str,
    port: u16,
    request: Vec<u8>,
    ca_pem: Option<String>,
    subscriber: u64,
) -> std::io::Result<u64> {
    reactor_up()?;
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
            subscriber: cell.clone(),
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
            // Bounded connect. `TcpStream::connect` waits out the kernel's SYN timeout —
            // minutes against a host that silently drops — and although this runs on its
            // own thread rather than a scheduler worker, that thread holds the caller's
            // registry entry and its `request` buffer for the whole time.
            let connected = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .and_then(|addrs| {
                    let mut last: Option<std::io::Error> = None;
                    for sa in addrs {
                        match std::net::TcpStream::connect_timeout(&sa, TLS_CONNECT_TIMEOUT) {
                            Ok(s) => return Ok(s),
                            Err(e) => last = Some(e),
                        }
                    }
                    Err(last.unwrap_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "tls: no addresses resolved",
                        )
                    }))
                });
            match connected {
                Ok(std_stream) => {
                    if std_stream.set_nonblocking(true).is_err() {
                        sink.emit(tcp_error_msg(id, "tls: could not configure socket"));
                        reg().remove(&id);
                        return;
                    }
                    // Disable Nagle: this is a request/response client that writes the
                    // whole request then reads. With Nagle on, the tail of a multi-record
                    // upload stalls waiting on the peer's delayed ACK, so a large body's
                    // records go out one delayed-ACK apart — O(size) round-trips, seconds
                    // for a few hundred KB.
                    let _ = std_stream.set_nodelay(true);
                    // The owner may have closed this handle while we were connecting: the
                    // registry entry was inserted BEFORE the connect, and `close` removes
                    // it. Handing the socket to the reactor now would install a live
                    // connection under an id nothing owns and nothing can close — an fd and
                    // a TLS session leaked for the life of the runtime. Dropping the stream
                    // here closes it. KI-97 item 4.
                    if !reg().contains_key(&id) {
                        return;
                    }
                    let stream = MioStream::from_std(std_stream);
                    reactor().cmd(Cmd::TlsClient {
                        id,
                        stream,
                        sink,
                        subscriber: cell,
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
        .map_err(|e| {
            // `Builder::spawn`, and its error handled rather than `.expect`ed: a refused
            // thread (EAGAIN) would otherwise panic in whichever green process called this
            // (KI-97 item 3's class). Undo the registry entry so the id does not linger.
            reg().remove(&id);
            std::io::Error::other(format!("tls: cannot start connect thread: {e}"))
        })?;
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
    reactor_up()?;
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
    // See `connect` — closes the race with `reactor_died`'s sweep.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
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
