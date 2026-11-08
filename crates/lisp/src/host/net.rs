//! TCP sockets (ADR-062) — and UDP sockets (ADR-449) — on one **reactor thread** (ADR-143),
//! delivering to process mailboxes (ADR-059).
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
//! assigns it an owner, so no inbound bytes are lost to the acceptor before a
//! per-connection handler takes over.
//!
//! **Writes are queued, and flow-controlled** (ADR-143): `tcp-send` lowers its iolist
//! to bytes, hands them to the reactor, and returns; the reactor flushes as the socket
//! accepts them. Past [`OUTBOUND_HIGH_WATER`] unwritten, the sender waits (parked, no
//! worker held) until the reactor has written it below [`OUTBOUND_LOW_WATER`] or the
//! connection is gone ([`OutboundFlow`]), so a fast producer runs at its peer's pace
//! and holds about a megabyte, not the whole of what it sends. Output that makes no
//! progress for [`SEND_STALL_TIMEOUT`] drops the connection, which ends such a wait on a
//! peer that stopped reading. `tcp-close` lets what is queued flush in the background (bounded
//! by [`LINGER`]), so `tcp-send` + `tcp-close` can never truncate a response; but
//! from the moment it returns the owner receives nothing more for that socket
//! ([`SocketOwner`]) — no data read while lingering, no `[:tcp-closed]`. [`OUT_CAP`] per
//! socket stays as the backstop past which the connection is dropped rather than
//! buffering without bound. Write failures surface as `[:tcp-closed …]`
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

use mio::net::{TcpListener as MioListener, TcpStream as MioStream, UdpSocket as MioUdp};
use mio::{Events, Interest, Poll, Token, Waker};

use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

use crate::core::value;
use crate::host::dial::dial_tcp;
use crate::host::owned::OwnedTable;
use crate::process::{
    CreditSource, DeliveryCredit, Message, chunk_flush, chunk_payload, deliver, deliver_credited,
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

/// Per-socket outbound queue cap: past it the connection is dropped and the owner sees
/// `[:tcp-closed …]`. A sender waits at [`OUTBOUND_HIGH_WATER`], so only a single
/// `tcp-send` larger than this reaches it; it is the backstop that keeps one such call
/// from buffering without bound.
const OUT_CAP: usize = 16 * 1024 * 1024;

/// Outbound backpressure: how much a stream may have handed to `tcp-send` and not yet
/// written to the kernel before `tcp-send` waits. `tcp-send` used to queue and return
/// whatever the peer's pace, so a server streaming a large response to a slow reader
/// produced it as fast as it could and held all of it, up to [`OUT_CAP`], and then the
/// connection was dropped mid-response. Past this mark the sender is parked (no worker
/// held) until the reactor has written the queue below [`OUTBOUND_LOW_WATER`], or the
/// connection is gone ([`OutboundFlow`]). Overshoot is at most one `tcp-send`.
const OUTBOUND_HIGH_WATER: usize = 1024 * 1024;

/// A parked sender is woken once the queue falls below this: well under the high-water
/// mark, so a sender writing small pieces does not park and wake per piece.
const OUTBOUND_LOW_WATER: usize = 256 * 1024;

/// How long output waiting to be sent may make no progress before the connection is
/// dropped (`tcp-set-stall-timeout` changes it per socket; 0 disarms). The bound on a
/// peer that stops reading: with `tcp-send` waiting on the queue, nothing else would
/// end that wait. Only a peer whose kernel buffers are full stalls (a peer not reading a
/// small reply never does), so a legitimately idle stream is never touched.
const SEND_STALL_TIMEOUT: Duration = Duration::from_secs(60);
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

/// How long a closing socket may keep flushing queued outbound bytes, and then wait for
/// the peer's EOF with its write half shut (the lingering close, KI-228), before the
/// reactor gives up and drops it. Bounds `tcp-close` after a large `tcp-send` to a slow
/// peer, and the descriptor a peer that never closes its side would otherwise pin.
const LINGER: Duration = Duration::from_secs(5);

/// The reactor's poll timeout — the cadence of reaper/linger housekeeping when
/// no IO is happening. Purely a housekeeping tick: IO readiness wakes the poll
/// immediately, commands wake it via the `Waker`.
const TICK: Duration = Duration::from_millis(1000);

/// How often a listener whose accept failed (fd exhaustion: `EMFILE`/`ENFILE`) is retried.
/// The registration is edge-triggered, so without a retry the connections already in its
/// backlog wait for some later arrival to wake it — forever, if none comes (KI-227).
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

const WAKER_TOKEN: Token = Token(0);

// ---- control plane: the id → socket registry the builtins talk to ----

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Stream,
    TlsStream,
    Listener,
    TlsListener,
    /// A bound UDP socket (`udp-open`): datagrams in either direction, no connection.
    Udp,
}

/// The control-plane entry for one socket id: what the builtins need for
/// validation and bookkeeping. The data plane (fd, rustls state, queues,
/// carries) lives with the reactor; commands cross via [`Cmd`].
struct Ctl {
    kind: Kind,
    /// Inbound decode mode (ADR-141) — shared with the reactor, read per chunk.
    binary: Arc<AtomicBool>,
    /// Where the socket's messages go — shared with the reactor, retargeted by
    /// `controlling_process`, silenced by `close`.
    owner: Arc<SocketOwner>,
    /// Whether reads have been started (an active connect, or a claimed accept).
    /// Gates TLS `tcp-send` (a TLS connection exists only once claimed).
    claimed: bool,
    /// The local port, cached at creation so `tcp-local-port` never blocks.
    port: Option<u16>,
    /// A listener's fd, shared with the reactor so `close` can shut it on the calling
    /// thread. `None` for a stream.
    listener: Option<ListenerSlot>,
    /// A TLS client stream's negotiated ALPN protocol (`tls-connect`), set by the reactor
    /// when the handshake completes; `None` for every other socket.
    alpn: Option<Arc<OnceLock<String>>>,
    /// Outbound backpressure, made by the first `tcp-send` and shared with the reactor
    /// through each [`Cmd::Send`] ([`OutboundFlow`]); `None` for a socket never sent on.
    outbound: Option<Arc<OutboundFlow>>,
}

/// A listener's fd, held jointly by its control-plane entry and the reactor. `close`
/// empties it on the calling thread, so the port refuses connections before
/// `tcp-close` returns — a queued close left the fd accepting until the reactor got to
/// it. The reactor touches the listener only under this lock and treats an empty slot
/// as closed, so no accept can run after the close.
type ListenerSlot = Arc<Mutex<Option<MioListener>>>;

fn listener_slot(listener: MioListener) -> ListenerSlot {
    Arc::new(Mutex::new(Some(listener)))
}

/// Close a listener's fd now. Deregistering one the reactor never registered fails
/// harmlessly.
fn shut_listener(slot: &ListenerSlot, registry: &mio::Registry) {
    if let Some(mut listener) = crate::core::sync::lock(slot).take() {
        let _ = registry.deregister(&mut listener);
    }
}

/// Where one socket's messages go: its owner's pid, or `None` once `tcp-close` has
/// retired it. Every message for the socket — data, `[:tcp-closed]`, `[:tcp-error]`,
/// `[:tcp-accept]`, `[:tcp-connected]` — is delivered through [`emit`](Self::emit)
/// **while this lock is held**, and `close` clears the pid under the same lock
/// ([`silence`](Self::silence)). So when `tcp-close` returns, every delivery that read
/// the pid has landed, and every later one is dropped: nothing new for that socket
/// reaches any mailbox. The reactor keeps flushing queued outbound bytes after that
/// (bounded by [`LINGER`]); what it reads meanwhile is discarded.
///
/// Lock order: this, then the target's mailbox (`deliver`). Nothing takes a mailbox lock
/// and then this one, and neither `close` nor `close_process_sockets` is called with a
/// mailbox lock held.
struct SocketOwner {
    pid: Mutex<Option<u64>>,
}

impl SocketOwner {
    fn new(pid: u64) -> Arc<SocketOwner> {
        Arc::new(SocketOwner {
            pid: Mutex::new(Some(pid)),
        })
    }

    /// The current owner, `None` once silenced.
    fn pid(&self) -> Option<u64> {
        *crate::core::sync::lock(&self.pid)
    }

    fn emit(&self, message: Message) {
        if let Some(pid) = *crate::core::sync::lock(&self.pid) {
            deliver(pid, message);
        }
    }

    /// Deliver a flow-controlled data chunk. A silenced socket drops it here, which
    /// returns its credit at once.
    fn emit_credited(&self, message: Message, credit: DeliveryCredit) {
        if let Some(pid) = *crate::core::sync::lock(&self.pid) {
            deliver_credited(pid, message, credit);
        }
    }

    /// Hand the socket to `pid` (`tcp-controlling-process`). A silenced socket stays
    /// silent.
    fn retarget(&self, pid: u64) {
        if let Some(current) = crate::core::sync::lock(&self.pid).as_mut() {
            *current = pid;
        }
    }

    /// Stop all delivery for good. Returns once no delivery is in flight.
    fn silence(&self) {
        *crate::core::sync::lock(&self.pid) = None;
    }
}

/// The socket table, indexed by owner — the green process whose death closes the socket
/// (`close_process_sockets`, O(sockets it owns)); `controlling_process` hands it on.
static REGISTRY: LazyLock<Mutex<OwnedTable<Ctl>>> =
    LazyLock::new(|| Mutex::new(OwnedTable::default()));
/// Socket ids double as reactor poll tokens, so 0 is reserved for the waker.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, OwnedTable<Ctl>> {
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
    start_reactor().map(|_| ())
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
    // Emitted with the registry lock held, so a `close` racing the sweep either removed
    // its entry first (and gets nothing) or waits until these have landed.
    let mut registry = reg();
    for (id, ctl) in registry.drain() {
        ctl.owner.emit(tcp_error_msg(id, "net reactor died"));
        ctl.owner.emit(tcp_closed_msg(id));
        // nothing will write its queue now: wake whoever waits on it (a socket already
        // closed but still flushing is woken as its connection unwinds with the thread)
        if let Some(flow) = &ctl.outbound {
            flow.close();
        }
    }
}

// ---- commands into the reactor ----

enum Cmd {
    /// A connected plaintext stream (from `tcp-connect`): start reading at once.
    Stream {
        id: u64,
        stream: MioStream,
        owner: Arc<SocketOwner>,
        binary: Arc<AtomicBool>,
    },
    /// A plaintext listener.
    Listen {
        id: u64,
        listener: ListenerSlot,
        owner: Arc<SocketOwner>,
    },
    /// A TLS listener (accepted connections become passive TLS streams).
    TlsListen {
        id: u64,
        listener: ListenerSlot,
        owner: Arc<SocketOwner>,
        config: Arc<ServerConfig>,
    },
    /// A one-shot TLS client exchange (from `tls-request`): handshake, send
    /// `request`, stream the response, `[:tcp-closed]` at EOF.
    TlsClient {
        id: u64,
        stream: MioStream,
        owner: Arc<SocketOwner>,
        binary: Arc<AtomicBool>,
        server_name: ServerName<'static>,
        request: Vec<u8>,
        config: Arc<ClientConfig>,
        /// `Some` for a persistent `tls-connect` stream: no request, announce when ready.
        announce: Option<Arc<OnceLock<String>>>,
    },
    /// Start reading a passive (accepted) stream — the claim half of
    /// `tcp-controlling-process` (the owner is retargeted control-side).
    Claim { id: u64 },
    /// A claimed stream changed owner (`tcp-controlling-process` again): give it a fresh
    /// [`InboundFlow`], resuming it if the old one held it paused.
    Retarget { id: u64 },
    /// The owner consumed enough of a paused stream's chunks — read it again
    /// ([`InboundFlow::release`]).
    Resume { id: u64 },
    /// A bound UDP socket (from `udp-open`): start reading at once.
    Udp {
        id: u64,
        socket: MioUdp,
        owner: Arc<SocketOwner>,
        binary: Arc<AtomicBool>,
    },
    /// One datagram for a UDP socket to send to `to`.
    UdpSend {
        id: u64,
        to: std::net::SocketAddr,
        bytes: Vec<u8>,
    },
    /// Queue outbound bytes, already charged against `flow`.
    Send {
        id: u64,
        bytes: Vec<u8>,
        flow: Arc<OutboundFlow>,
    },
    /// Arm/disarm an established stream's idle timeout (`ms` = 0 disarms).
    SetIdle { id: u64, ms: u64 },
    /// Set how long a stream's queued output may make no progress (`ms` = 0 disarms).
    SetStall { id: u64, ms: u64 },
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
    /// For deregistering a listener `close` shuts on the calling thread.
    registry: mio::Registry,
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

static REACTOR: OnceLock<Reactor> = OnceLock::new();

/// The reactor singleton. Every socket-creating entry point starts it first through
/// [`reactor_up`], which reports a reactor that cannot start as an error.
fn reactor() -> &'static Reactor {
    start_reactor().unwrap_or_else(|error| panic!("net reactor: {error}"))
}

/// Start the reactor on first socket use. Fallible: the OS can refuse its thread
/// (`EAGAIN` under a thread or task limit) or its poll descriptors (`EMFILE`), and either
/// is the caller's error to report. Nothing is kept from a failed start, so the next
/// socket tries again.
fn start_reactor() -> std::io::Result<&'static Reactor> {
    static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(reactor) = REACTOR.get() {
        return Ok(reactor);
    }
    let _starting = crate::core::sync::lock(&STARTING);
    if let Some(reactor) = REACTOR.get() {
        return Ok(reactor);
    }
    let poll = Poll::new()?;
    let waker = Waker::new(poll.registry(), WAKER_TOKEN)?;
    let registry = poll.registry().try_clone()?;
    let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
    std::thread::Builder::new()
        .name("brood-net-reactor".into())
        .spawn(move || {
            // The reactor multiplexes EVERY socket in the runtime on this one
            // thread, so its death must be observed, not swallowed: catch a
            // panic (a `poll` error returns normally) and run the death hook —
            // mark the reactor down, fail every socket at its owner, and make
            // subsequent socket ops error instead of silently succeeding.
            let r =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reactor_loop(poll, rx)));
            reactor_died(r.err());
        })
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot start the network thread: {error}"),
            )
        })?;
    Ok(REACTOR.get_or_init(|| Reactor {
        tx,
        waker,
        registry,
    }))
}

// ---- message builders (off-heap; symbols are a global interner) ----

fn tcp_data_msg(id: u64, payload: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp")),
        Message::Socket(id),
        payload,
    ])
}

/// `[:udp sock host port data]` — one datagram, with the address it came from.
fn udp_msg(id: u64, from: std::net::SocketAddr, payload: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("udp")),
        Message::Socket(id),
        Message::Str(from.ip().to_string()),
        Message::Int(i64::from(from.port())),
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

fn tcp_drained_msg(id: u64) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-drained")),
        Message::Socket(id),
    ])
}

// ---- outbound flow control ----

/// One stream's outbound budget: the bytes `tcp-send` has handed over and the reactor
/// has not yet written to the kernel (for TLS, plaintext not yet fully flushed as
/// records). `tcp-send` charges it before queuing; the reactor releases it as it writes.
/// A send that leaves it at or past [`OUTBOUND_HIGH_WATER`] parks its caller: the pid
/// goes on `waiters`, and the caller waits for `[:tcp-drained sock]`.
///
/// Every parked sender is woken exactly once, by whichever comes first: the release that
/// takes the queue below [`OUTBOUND_LOW_WATER`], or the connection going by any route
/// ([`close`](OutboundFlow::close): the reactor drops its [`SendGuard`] with the
/// connection, finds no connection for a send, or dies). After a close no one parks
/// again, so a sender's next `tcp-send` fails or returns at once. The parking decision
/// and the wake-ups are taken under the one `waiters` lock, so a release can never slip
/// between a sender's check and its parking.
///
/// It is created by the first `tcp-send` on a socket and travels with each
/// [`Cmd::Send`]; the reactor keeps it in the connection's [`SendGuard`]. Waiters are
/// whichever processes sent, not the socket's owner: a helper writing to a socket
/// another process owns parks itself.
struct OutboundFlow {
    id: u64,
    queued: AtomicUsize,
    waiters: Mutex<Waiters>,
}

#[derive(Default)]
struct Waiters {
    pids: Vec<u64>,
    closed: bool,
}

impl OutboundFlow {
    fn new(id: u64) -> Arc<OutboundFlow> {
        Arc::new(OutboundFlow {
            id,
            queued: AtomicUsize::new(0),
            waiters: Mutex::new(Waiters::default()),
        })
    }

    /// Control side: charge `bytes` about to be queued.
    fn charge(&self, bytes: usize) {
        self.queued.fetch_add(bytes, Ordering::SeqCst);
    }

    /// Control side, after queuing: whether `pid` must wait for `[:tcp-drained]`. Parks
    /// it when the queue is at or past the high-water mark and the connection is not
    /// gone.
    fn park_if_full(&self, pid: u64) -> bool {
        let mut waiters = crate::core::sync::lock(&self.waiters);
        if waiters.closed || self.queued.load(Ordering::SeqCst) < OUTBOUND_HIGH_WATER {
            return false;
        }
        waiters.pids.push(pid);
        true
    }

    /// Reactor side: `bytes` were written. Wakes the parked senders once the queue is
    /// below the low-water mark.
    fn release(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let after = self.queued.fetch_sub(bytes, Ordering::SeqCst) - bytes;
        if after < OUTBOUND_LOW_WATER {
            let mut waiters = crate::core::sync::lock(&self.waiters);
            for pid in waiters.pids.drain(..) {
                deliver(pid, tcp_drained_msg(self.id));
            }
        }
    }

    /// The connection is gone: wake every parked sender, and park no one again.
    fn close(&self) {
        let mut waiters = crate::core::sync::lock(&self.waiters);
        waiters.closed = true;
        for pid in waiters.pids.drain(..) {
            deliver(pid, tcp_drained_msg(self.id));
        }
    }
}

/// The reactor's hold on a connection's [`OutboundFlow`]: dropped with the connection,
/// whatever ends it (a write failure, [`OUT_CAP`], a stall, an idle or linger deadline,
/// owner death, the reactor's own death), and dropping it closes the flow, so no sender
/// stays parked on a connection that no longer exists.
struct SendGuard(Arc<OutboundFlow>);

impl SendGuard {
    fn release(&self, bytes: usize) {
        self.0.release(bytes);
    }
}

impl Drop for SendGuard {
    fn drop(&mut self) {
        self.0.close();
    }
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
    owner: &SocketOwner,
    flow: &Arc<InboundFlow>,
    payload: Message,
    bytes: usize,
) {
    owner.emit_credited(tcp_data_msg(id, payload), flow.charge(bytes));
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
    /// Where every message for this socket goes ([`SocketOwner`]); data chunks carry a
    /// flow-control credit ([`emit_data`]).
    owner: Arc<SocketOwner>,
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
    /// The write half was shut down after the close's last queued byte: the peer has its
    /// FIN, and the reactor reads and discards until the peer's EOF (or the deadline).
    /// Dropping the socket with input still unread would send a RESET instead, and a reset
    /// makes the peer's kernel discard the response it has not read yet (KI-228).
    write_shut: bool,
    registered: bool,
    /// Opt-in idle bound (`tcp-set-idle-timeout`, default off). When `Some`, the
    /// reactor drops the connection if no bytes move in either direction for this
    /// long — slow-loris protection a raw-TCP server can arm on a connection it
    /// accepts. Off by default so a legitimately long-idle stream (SSE, long-poll,
    /// the editor daemon) is never reaped.
    idle: Option<Duration>,
    /// Last time bytes moved (inbound read or outbound `Send`) — the idle stamp.
    last_activity: Instant,
    /// Outbound backpressure ([`OutboundFlow`]), held from the first `Send`; dropping it
    /// with the connection wakes every sender parked on it.
    sent: Option<SendGuard>,
    /// How long queued output may make no progress before the connection is dropped
    /// ([`SEND_STALL_TIMEOUT`] unless `tcp-set-stall-timeout` changed it; `None` disarmed).
    stall: Option<Duration>,
    /// When queued output last made progress, or became queued: the stall stamp.
    out_progress: Instant,
}

/// One TLS stream's reactor state — the same machine drives a server
/// connection (accepted via `tls-listen`) and a one-shot client exchange
/// (`tls-request`); rustls's `Connection` deref-target covers both.
struct TlsConn {
    stream: MioStream,
    conn: rustls::Connection,
    /// See [`PlainConn::owner`].
    owner: Arc<SocketOwner>,
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
    /// See [`PlainConn::sent`].
    sent: Option<SendGuard>,
    /// See [`PlainConn::stall`].
    stall: Option<Duration>,
    /// See [`PlainConn::out_progress`].
    out_progress: Instant,
    /// A persistent client (`tls-connect`) still to announce itself: when the handshake
    /// completes the reactor records the negotiated ALPN protocol here and emits
    /// `[:tcp-connected sock]`, ahead of any data. `None` once announced, and always for a
    /// server connection or a one-shot `tls-request`.
    announce: Option<Arc<OnceLock<String>>>,
}

/// A passive accepted TLS connection: raw materials until claimed.
struct TlsPending {
    stream: MioStream,
    config: Arc<ServerConfig>,
    owner: Arc<SocketOwner>,
    binary: Arc<AtomicBool>,
    accepted_at: Instant,
}

/// One UDP socket's reactor state (`udp-open`). No connection, so nothing to linger or
/// hand over: inbound datagrams go to the owner under the same flow control as a stream's
/// chunks, and outbound ones are sent at once, queued only while the kernel buffer is full.
struct UdpConn {
    socket: MioUdp,
    owner: Arc<SocketOwner>,
    flow: Arc<InboundFlow>,
    binary: Arc<AtomicBool>,
    /// Datagrams the kernel refused for now (`WouldBlock`), in order, and their bytes.
    out: std::collections::VecDeque<(std::net::SocketAddr, Vec<u8>)>,
    out_total: usize,
    registered: bool,
}

// The `Tls` variant (rustls state) is much larger than the others, but the
// reactor holds exactly one `Rx` per live socket and a TLS connection genuinely
// needs that state inline — boxing would just add an indirection on the hot
// read/write path for no real saving.
#[allow(clippy::large_enum_variant)]
enum Rx {
    Plain(PlainConn),
    Tls(TlsConn),
    Udp(UdpConn),
    TlsPending(TlsPending),
    Listener {
        listener: ListenerSlot,
        owner: Arc<SocketOwner>,
    },
    TlsListener {
        listener: ListenerSlot,
        owner: Arc<SocketOwner>,
        config: Arc<ServerConfig>,
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
    let mut accepted: Vec<Accepted> = Vec::new();
    // Listeners whose accept drain stopped on an error (fd exhaustion), each with the time
    // of its next retry; `starved` collects this pass's stops (KI-227).
    let mut accept_retries: HashMap<u64, Instant> = HashMap::new();
    let mut starved: Vec<(u64, std::io::Error)> = Vec::new();

    loop {
        let timeout = accept_retries.values().min().map_or(TICK, |next| {
            next.saturating_duration_since(Instant::now()).min(TICK)
        });
        if let Err(e) = poll.poll(&mut events, Some(timeout)) {
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
                Some(rx) => drive(
                    id,
                    rx,
                    readable,
                    writable,
                    &registry,
                    &mut accepted,
                    &mut starved,
                ),
                None => false,
            };
            if remove {
                teardown(id, &mut conns, &registry);
            }
            announce_accepted(&mut conns, &mut accepted);
        }

        // Retry every starved listener that is due, as if it were readable again: the
        // backlog it could not drain has no new edge coming.
        let now = Instant::now();
        let due: Vec<u64> = accept_retries
            .iter()
            .filter(|(_, next)| **next <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(rx) = conns.get_mut(&id) {
                drive(id, rx, true, false, &registry, &mut accepted, &mut starved);
                announce_accepted(&mut conns, &mut accepted);
            }
            if !starved.iter().any(|(starved_id, _)| *starved_id == id) {
                accept_retries.remove(&id);
                if conns.contains_key(&id) {
                    eprintln!("net: listener {id} is accepting again");
                }
            }
        }
        // Out of descriptors: the lingering closes hold some, and accepting the backlog is
        // worth more than sparing a peer a reset on a connection already answered.
        if !starved.is_empty() {
            let lingering: Vec<u64> = conns
                .iter()
                .filter(|(_, rx)| matches!(rx, Rx::Plain(connection) if connection.write_shut))
                .map(|(id, _)| *id)
                .collect();
            for id in lingering {
                teardown(id, &mut conns, &registry);
            }
        }
        for (id, error) in starved.drain(..) {
            // Said once per episode, not once per retry.
            if accept_retries
                .insert(id, Instant::now() + ACCEPT_RETRY)
                .is_none()
            {
                eprintln!(
                    "net: listener {id} cannot accept ({error}); retrying every {} ms",
                    ACCEPT_RETRY.as_millis()
                );
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

/// Park the staged accepts, then announce them — the entry must be in place before the
/// owner can react to `[:tcp-accept …]`.
fn announce_accepted(conns: &mut HashMap<u64, Rx>, accepted: &mut Vec<Accepted>) {
    for staged in accepted.drain(..) {
        conns.insert(staged.id, staged.entry);
        staged
            .listener_owner
            .emit(tcp_accept_msg(staged.listener_id, staged.id));
    }
}

/// Drive one socket's readiness. Returns true when the entry must be removed. A listener
/// whose drain stopped on an error is pushed onto `starved` for the reactor to retry.
fn drive(
    id: u64,
    rx: &mut Rx,
    readable: bool,
    writable: bool,
    registry: &mio::Registry,
    accepted: &mut Vec<Accepted>,
    starved: &mut Vec<(u64, std::io::Error)>,
) -> bool {
    match rx {
        Rx::Listener { listener, owner } => {
            let mut slot = crate::core::sync::lock(listener);
            let Some(listener) = slot.as_mut() else {
                return true; // `close` shut it; its `Cmd::Close` removes the entry too
            };
            if readable && let Some(error) = accept_ready(id, listener, owner, None, accepted) {
                starved.push((id, error));
            }
            false
        }
        Rx::TlsListener {
            listener,
            owner,
            config,
        } => {
            let mut slot = crate::core::sync::lock(listener);
            let Some(listener) = slot.as_mut() else {
                return true;
            };
            if readable
                && let Some(error) =
                    accept_ready(id, listener, owner, Some(config.clone()), accepted)
            {
                starved.push((id, error));
            }
            false
        }
        Rx::Plain(c) => drive_plain(id, c, readable, writable, registry),
        Rx::Tls(c) => drive_tls(id, c, readable, writable, registry),
        Rx::Udp(c) => {
            drive_udp(id, c, readable, writable, registry);
            false
        }
        Rx::TlsPending(_) => false,
    }
}

/// A connection accepted during event handling, staged until the reactor can park it
/// and announce it to the listener's owner.
struct Accepted {
    id: u64,
    entry: Rx,
    listener_id: u64,
    listener_owner: Arc<SocketOwner>,
}

/// Accept every waiting connection on a ready listener; new sockets are
/// registered control-side and announced, then parked passive in the reactor.
/// Returns the error that stopped the drain before the backlog was empty, if one did.
fn accept_ready(
    lid: u64,
    listener: &mut MioListener,
    listener_owner: &Arc<SocketOwner>,
    tls: Option<Arc<ServerConfig>>,
    out: &mut Vec<Accepted>,
) -> Option<std::io::Error> {
    // The mode every socket accepted here starts in, read once per readiness rather than
    // per connection. An accepted socket is already reading by the time its owner gets
    // `[:tcp-accept …]`, so `tcp-set-binary` on the *stream* cannot close the window: a
    // client that sends immediately can have its first chunk decoded as text before the
    // call lands, and raw bytes then come back as U+FFFD-riddled UTF-8 (KI-102). Setting
    // the mode on the LISTENER has no window, because it is fixed before any connection
    // exists.
    let inherited = reg()
        .get(&lid)
        .map(|c| c.binary.load(Ordering::Acquire))
        .unwrap_or(false);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // A listener silenced by `close` (whose fd `close` is about to shut)
                // announces nothing more: drop the connection.
                let Some(owner_pid) = listener_owner.pid() else {
                    continue;
                };
                let cid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let binary = Arc::new(AtomicBool::new(inherited));
                let owner = SocketOwner::new(owner_pid);
                let port = stream.local_addr().ok().map(|a| a.port());
                reg().insert(
                    cid,
                    owner_pid,
                    Ctl {
                        kind: if tls.is_some() {
                            Kind::TlsStream
                        } else {
                            Kind::Stream
                        },
                        binary: binary.clone(),
                        owner: owner.clone(),
                        claimed: false,
                        port,
                        listener: None,
                        alpn: None,
                        outbound: None,
                    },
                );
                let entry = match &tls {
                    Some(config) => Rx::TlsPending(TlsPending {
                        stream,
                        config: config.clone(),
                        owner,
                        binary,
                        accepted_at: Instant::now(),
                    }),
                    None => Rx::Plain(PlainConn {
                        stream,
                        owner,
                        flow: InboundFlow::new(cid),
                        binary,
                        carry: Vec::new(),
                        out: OutQ::new(),
                        reading: false,
                        read_done: false,
                        accepted_at: Some(Instant::now()),
                        closing: None,
                        write_shut: false,
                        registered: false,
                        idle: None,
                        last_activity: Instant::now(),
                        sent: None,
                        stall: Some(SEND_STALL_TIMEOUT),
                        out_progress: Instant::now(),
                    }),
                };
                out.push(Accepted {
                    id: cid,
                    entry,
                    listener_id: lid,
                    listener_owner: listener_owner.clone(),
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return None,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // A connection that died between the readiness event and `accept` — the peer
            // reset it, or the kernel refused it. That is a fact about ONE connection, not
            // about the listener, so skip it and keep draining: the registration is
            // edge-triggered, so breaking here would strand every connection still queued
            // in the backlog until some *later* arrival happened to re-arm us. Silently.
            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionAborted => continue,
            // Anything else ends this drain — fd exhaustion, where continuing would spin, or
            // a broken listener. The caller retries it on `ACCEPT_RETRY`: the registration
            // is edge-triggered, so the connections still queued in the backlog would
            // otherwise wait for some later arrival to re-arm us (KI-227).
            Err(e) => return Some(e),
        }
    }
}

/// Desired poll interests for a plaintext connection; `None` = deregister.
fn plain_interests(c: &PlainConn) -> Option<Interest> {
    // A paused socket (inbound backpressure) drops read interest; `Cmd::Resume` re-reads
    // it explicitly, since the edge that would have announced buffered data is spent.
    // A lingering close (write half shut) reads to discard, paused or not.
    let want_read = !c.read_done && (c.write_shut || (c.reading && !c.flow.is_paused()));
    interests_for(want_read, !c.out.is_empty())
}

/// The poll interests for wanting to read and/or write; `None` = want neither.
fn interests_for(want_read: bool, want_write: bool) -> Option<Interest> {
    match (want_read, want_write) {
        (true, true) => Some(Interest::READABLE.add(Interest::WRITABLE)),
        (true, false) => Some(Interest::READABLE),
        (false, true) => Some(Interest::WRITABLE),
        (false, false) => None,
    }
}

/// Bring `stream`'s registration in line with `interests`: (re)register it under `id`'s
/// token, or deregister it on `None`. `registered` tracks which call mio expects next.
fn sync_registration(
    id: u64,
    stream: &mut MioStream,
    registered: &mut bool,
    interests: Option<Interest>,
    registry: &mio::Registry,
) {
    match interests {
        Some(interests) => {
            let res = if *registered {
                registry.reregister(stream, Token(id as usize), interests)
            } else {
                registry.register(stream, Token(id as usize), interests)
            };
            if res.is_ok() {
                *registered = true;
            }
        }
        None => {
            if *registered {
                let _ = registry.deregister(stream);
                *registered = false;
            }
        }
    }
}

fn sync_plain_registration(id: u64, c: &mut PlainConn, registry: &mio::Registry) {
    let interests = plain_interests(c);
    sync_registration(id, &mut c.stream, &mut c.registered, interests, registry);
}

/// Returns true when the connection should be torn down.
fn drive_plain(
    id: u64,
    c: &mut PlainConn,
    readable: bool,
    writable: bool,
    registry: &mio::Registry,
) -> bool {
    if c.write_shut {
        // Lingering close: the peer has our FIN; read and drop what it still sends until
        // its EOF, so the socket closes without a reset.
        if discard_until_blocked(&mut c.stream) {
            return true;
        }
        sync_plain_registration(id, c, registry);
        return false;
    }
    if writable || !c.out.is_empty() {
        let before = c.out.total;
        match c.out.flush_into(&mut c.stream) {
            Ok(_) => {}
            Err(_) => {
                if c.reading && !c.read_done {
                    c.owner.emit(tcp_closed_msg(id));
                    c.read_done = true;
                }
                return true;
            }
        }
        // Outbound bytes actually left the queue — count it as activity so a large
        // response draining to a slow reader isn't idle-reaped mid-send.
        if c.out.total < before {
            c.last_activity = Instant::now();
            c.out_progress = c.last_activity;
            if let Some(sent) = &c.sent {
                sent.release(before - c.out.total);
            }
        }
        if c.out.is_empty() && c.closing.is_some() {
            return begin_linger(id, c, registry);
        }
    }
    if readable && c.reading && !c.read_done && !c.flow.is_paused() {
        let mut buf = [0u8; 65536];
        loop {
            match c.stream.read(&mut buf) {
                Ok(0) => {
                    if let Some(p) = chunk_flush(&mut c.carry) {
                        c.owner.emit(tcp_data_msg(id, p));
                    }
                    c.owner.emit(tcp_closed_msg(id));
                    c.read_done = true;
                    break;
                }
                Ok(n) => {
                    c.last_activity = Instant::now();
                    let bin = c.binary.load(Ordering::Acquire);
                    if let Some(p) = chunk_payload(&mut c.carry, &buf[..n], bin) {
                        emit_data(id, &c.owner, &c.flow, p, n);
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
                    c.owner.emit(tcp_closed_msg(id));
                    c.read_done = true;
                    break;
                }
            }
        }
    }
    // NOTE deliberately no auto-teardown on read EOF: a peer half-close leaves
    // the write side usable (the request-then-FIN client still gets its
    // response). The entry lives until an explicit `Close`, a
    // write failure, the OUT_CAP breach, the linger deadline, or owner death.
    if c.read_done && c.out.is_empty() && c.closing.is_some() {
        return true;
    }
    sync_plain_registration(id, c, registry);
    false
}

/// The close's last queued byte is out: shut the write half (the peer gets every byte,
/// then EOF) and linger, discarding input, until the peer's EOF or the close deadline.
/// Returns true when the socket can go at once — the peer already sent EOF, or the
/// shutdown or a read failed.
fn begin_linger(id: u64, c: &mut PlainConn, registry: &mio::Registry) -> bool {
    if c.read_done {
        return true; // the peer's EOF was read: nothing unread can provoke a reset
    }
    if c.reading {
        c.owner.emit(tcp_closed_msg(id));
    }
    if c.stream.shutdown(std::net::Shutdown::Write).is_err() {
        return true;
    }
    c.write_shut = true;
    if discard_until_blocked(&mut c.stream) {
        return true;
    }
    sync_plain_registration(id, c, registry);
    false
}

/// Read and drop everything the peer has sent. True at its EOF or on a read error.
fn discard_until_blocked(stream: &mut MioStream) -> bool {
    let mut buffer = [0u8; 65536];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
}

/// What a UDP socket waits for: datagrams (unless its owner is full) and, while some are
/// queued, room to send.
fn sync_udp_registration(id: u64, c: &mut UdpConn, registry: &mio::Registry) {
    let interest = interests_for(!c.flow.is_paused(), !c.out.is_empty());
    let token = Token(id as usize);
    let result = match (interest, c.registered) {
        (Some(interest), true) => registry.reregister(&mut c.socket, token, interest),
        (Some(interest), false) => registry.register(&mut c.socket, token, interest),
        (None, true) => registry.deregister(&mut c.socket),
        (None, false) => Ok(()),
    };
    if result.is_ok() {
        c.registered = interest.is_some();
    }
}

/// Read every datagram waiting (until the kernel is empty or the owner's mailbox is full)
/// and send what is queued. A receive error is a previous send's ICMP refusal surfacing
/// (`ECONNREFUSED`) — not this socket failing — so it is skipped, not fatal; a send that
/// cannot go now waits for writability.
fn drive_udp(id: u64, c: &mut UdpConn, readable: bool, writable: bool, registry: &mio::Registry) {
    if readable {
        let mut buf = vec![0u8; 65_536];
        while !c.flow.is_paused() {
            match c.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let payload = if c.binary.load(Ordering::Acquire) {
                        Message::Bytes(crate::core::blob::SharedBlob::new(&buf[..n]))
                    } else {
                        Message::Str(String::from_utf8_lossy(&buf[..n]).into_owned())
                    };
                    c.owner
                        .emit_credited(udp_msg(id, from, payload), c.flow.charge(n));
                    if c.flow.pause_if_full() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {}
            }
        }
    }
    if writable || !c.out.is_empty() {
        while let Some((to, bytes)) = c.out.front() {
            match c.socket.send_to(bytes, *to) {
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // unroutable, too large, refused: a datagram is sent or it is not
                Err(_) => {}
            }
            if let Some((_, sent)) = c.out.pop_front() {
                c.out_total -= sent.len();
            }
        }
    }
    sync_udp_registration(id, c, registry);
}

fn tls_interests(c: &TlsConn) -> Option<Interest> {
    // See `plain_interests` — a paused stream drops read interest.
    interests_for(!c.read_done && !c.flow.is_paused(), c.conn.wants_write())
}

fn sync_tls_registration(id: u64, c: &mut TlsConn, registry: &mio::Registry) {
    let interests = tls_interests(c);
    sync_registration(id, &mut c.stream, &mut c.registered, interests, registry);
}

/// Finish a TLS connection: emit the right terminal message once. Returns true.
fn tls_finish(id: u64, c: &mut TlsConn, error: Option<String>) -> bool {
    if let Some(p) = chunk_flush(&mut c.carry) {
        c.owner.emit(tcp_data_msg(id, p));
    }
    if !c.read_done {
        match error {
            Some(msg) if c.one_shot => c.owner.emit(tcp_error_msg(id, &msg)),
            _ => c.owner.emit(tcp_closed_msg(id)),
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
                Ok(_) => {
                    c.last_activity = Instant::now();
                    c.out_progress = c.last_activity;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return tls_finish(id, c, Some(format!("tls: {e}"))),
            }
        }
        // Fully drained to the socket → the OUT_CAP accounting resets (rustls's
        // buffer is empty again), and the senders' budget gets it all back.
        if !c.conn.wants_write() {
            if let Some(sent) = &c.sent {
                sent.release(c.pending_out);
            }
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
                        announce_if_ready(id, c);
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
                                    emit_data(id, &c.owner, &c.flow, p, got);
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
    announce_if_ready(id, c);
    if c.handshake_deadline.is_some() && !c.conn.is_handshaking() {
        c.handshake_deadline = None;
        c.last_activity = Instant::now();
    }
    sync_tls_registration(id, c, registry);
    false
}

/// A persistent client whose handshake has just completed: record the protocol ALPN chose
/// and tell the owner `[:tcp-connected sock]` — once, and before any of its data, since
/// every caller runs this before delivering what the records decrypted to.
fn announce_if_ready(id: u64, c: &mut TlsConn) {
    if c.announce.is_none() || c.conn.is_handshaking() {
        return;
    }
    if let Some(slot) = c.announce.take() {
        if let Some(protocol) = c.conn.alpn_protocol() {
            let _ = slot.set(String::from_utf8_lossy(protocol).into_owned());
        }
        c.owner.emit(tcp_connected_msg(id));
    }
}

/// Apply one control-plane command on the reactor thread — one function per command.
fn handle_cmd(cmd: Cmd, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    match cmd {
        Cmd::Stream {
            id,
            stream,
            owner,
            binary,
        } => start_stream(id, stream, owner, binary, conns, registry),
        Cmd::Listen {
            id,
            listener,
            owner,
        } => {
            register_listener(id, &listener, registry);
            conns.insert(id, Rx::Listener { listener, owner });
        }
        Cmd::TlsListen {
            id,
            listener,
            owner,
            config,
        } => {
            register_listener(id, &listener, registry);
            conns.insert(
                id,
                Rx::TlsListener {
                    listener,
                    owner,
                    config,
                },
            );
        }
        Cmd::TlsClient {
            id,
            stream,
            owner,
            binary,
            server_name,
            request,
            config,
            announce,
        } => start_tls_client(
            id,
            TlsClientStart {
                stream,
                owner,
                binary,
                server_name,
                request,
                config,
                announce,
            },
            conns,
            registry,
        ),
        Cmd::Udp {
            id,
            socket,
            owner,
            binary,
        } => {
            let mut connection = UdpConn {
                socket,
                owner,
                flow: InboundFlow::new(id),
                binary,
                out: std::collections::VecDeque::new(),
                out_total: 0,
                registered: false,
            };
            sync_udp_registration(id, &mut connection, registry);
            conns.insert(id, Rx::Udp(connection));
        }
        Cmd::UdpSend { id, to, bytes } => {
            if let Some(Rx::Udp(connection)) = conns.get_mut(&id) {
                // past the cap a datagram is dropped, which is what UDP promises anyway
                if connection.out_total + bytes.len() <= OUT_CAP {
                    connection.out_total += bytes.len();
                    connection.out.push_back((to, bytes));
                    drive_udp(id, connection, false, true, registry);
                }
            }
        }
        Cmd::Claim { id } => claim(id, conns, registry),
        Cmd::Retarget { id } => retarget(id, conns, registry),
        Cmd::Resume { id } => resume_reading(id, conns, registry),
        Cmd::Send { id, bytes, flow } => queue_send(id, bytes, flow, conns, registry),
        Cmd::SetIdle { id, ms } => set_idle(id, ms, conns),
        Cmd::SetStall { id, ms } => set_stall(id, ms, conns),
        Cmd::Close { id } => close_connection(id, conns, registry),
        #[cfg(debug_assertions)]
        Cmd::DieForTest => panic!("net reactor: test-induced death (Cmd::DieForTest)"),
    }
}

/// An active plaintext stream from `tcp-connect`: announce it, then start reading.
/// Emitting `[:tcp-connected]` here, on the reactor, is what makes the announcement
/// mean "established": every command the owner issues after seeing it (a send, a
/// close, an idle timeout) is queued behind this one, and it reaches the owner's
/// mailbox ahead of any `[:tcp …]` data because this thread delivers both.
fn start_stream(
    id: u64,
    stream: MioStream,
    owner: Arc<SocketOwner>,
    binary: Arc<AtomicBool>,
    conns: &mut HashMap<u64, Rx>,
    registry: &mio::Registry,
) {
    owner.emit(tcp_connected_msg(id));
    let mut connection = PlainConn {
        stream,
        owner,
        flow: InboundFlow::new(id),
        binary,
        carry: Vec::new(),
        out: OutQ::new(),
        reading: true,
        read_done: false,
        accepted_at: None,
        closing: None,
        write_shut: false,
        registered: false,
        idle: None,
        last_activity: Instant::now(),
        sent: None,
        stall: Some(SEND_STALL_TIMEOUT),
        out_progress: Instant::now(),
    };
    sync_plain_registration(id, &mut connection, registry);
    conns.insert(id, Rx::Plain(connection));
}

/// Register a listener for accept readiness — unless `close` already shut it.
fn register_listener(id: u64, listener: &ListenerSlot, registry: &mio::Registry) {
    if let Some(listener) = crate::core::sync::lock(listener).as_mut() {
        let _ = registry.register(listener, Token(id as usize), Interest::READABLE);
    }
}

/// The parts of a `Cmd::TlsClient` that become the connection.
struct TlsClientStart {
    stream: MioStream,
    owner: Arc<SocketOwner>,
    binary: Arc<AtomicBool>,
    server_name: ServerName<'static>,
    request: Vec<u8>,
    config: Arc<ClientConfig>,
    /// `Some` for a persistent `tls-connect` stream (see [`TlsConn::announce`]).
    announce: Option<Arc<OnceLock<String>>>,
}

/// A one-shot TLS client exchange (`tls-request`): buffer the request, start the handshake.
fn start_tls_client(
    id: u64,
    start: TlsClientStart,
    conns: &mut HashMap<u64, Rx>,
    registry: &mio::Registry,
) {
    match ClientConnection::new(start.config, start.server_name) {
        Ok(mut tls) => {
            // Buffer the whole request as plaintext now; rustls emits it once the handshake
            // completes. Lift rustls's 64 KiB writer cap, or `write_all` silently keeps only
            // the first 64 KiB and the peer waits forever for the rest. The request is
            // already fully in memory, so no cap just holds one copy of it.
            tls.set_buffer_limit(None);
            let _ = tls.writer().write_all(&start.request);
            let mut connection = TlsConn {
                stream: start.stream,
                conn: rustls::Connection::Client(tls),
                owner: start.owner,
                flow: InboundFlow::new(id),
                binary: start.binary,
                carry: Vec::new(),
                read_done: false,
                closing: None,
                registered: false,
                pending_out: 0,
                one_shot: start.announce.is_none(),
                handshake_deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
                idle: None,
                last_activity: Instant::now(),
                sent: None,
                stall: Some(SEND_STALL_TIMEOUT),
                out_progress: Instant::now(),
                announce: start.announce,
            };
            sync_tls_registration(id, &mut connection, registry);
            conns.insert(id, Rx::Tls(connection));
        }
        Err(error) => {
            start
                .owner
                .emit(tcp_error_msg(id, &format!("tls: {error}")));
            reg().remove(&id);
        }
    }
}

/// The first `tcp-controlling-process` on an accepted socket: start reading it (and, for
/// a TLS accept, start the server handshake).
fn claim(id: u64, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    match conns.remove(&id) {
        Some(Rx::Plain(mut connection)) => {
            connection.reading = true;
            connection.accepted_at = None;
            // Start the idle clock from establishment: time spent passive and unclaimed
            // must not count against an idle bound armed before the claim.
            connection.last_activity = Instant::now();
            sync_plain_registration(id, &mut connection, registry);
            conns.insert(id, Rx::Plain(connection));
        }
        Some(Rx::TlsPending(pending)) => match ServerConnection::new(pending.config) {
            Ok(mut tls) => {
                // Lift rustls's 64 KiB outbound cap: `Cmd::Send` ignores a short
                // `write_all`, so a backed-up slow reader would silently lose bytes past it.
                // Our bound is `pending_out` against `OUT_CAP`, which drops loudly.
                tls.set_buffer_limit(None);
                let mut connection = TlsConn {
                    stream: pending.stream,
                    conn: rustls::Connection::Server(tls),
                    owner: pending.owner,
                    flow: InboundFlow::new(id),
                    binary: pending.binary,
                    carry: Vec::new(),
                    read_done: false,
                    closing: None,
                    registered: false,
                    pending_out: 0,
                    one_shot: false,
                    handshake_deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
                    idle: None,
                    last_activity: Instant::now(),
                    sent: None,
                    stall: Some(SEND_STALL_TIMEOUT),
                    out_progress: Instant::now(),
                    announce: None,
                };
                sync_tls_registration(id, &mut connection, registry);
                conns.insert(id, Rx::Tls(connection));
            }
            Err(error) => {
                pending
                    .owner
                    .emit(tcp_error_msg(id, &format!("tls: {error}")));
                reg().remove(&id);
            }
        },
        Some(other) => {
            conns.insert(id, other);
        }
        None => {}
    }
}

/// A claimed stream changed owner: give it a fresh [`InboundFlow`], resuming it if the
/// old one held it paused.
fn retarget(id: u64, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    let flow = match conns.get_mut(&id) {
        Some(Rx::Plain(connection)) => Some(&mut connection.flow),
        Some(Rx::Tls(connection)) => Some(&mut connection.flow),
        Some(Rx::Udp(connection)) => Some(&mut connection.flow),
        _ => None,
    };
    let was_paused = match flow {
        Some(flow) => {
            let was_paused = flow.retire();
            *flow = InboundFlow::new(id);
            was_paused
        }
        None => false,
    };
    if was_paused {
        resume_reading(id, conns, registry);
    }
}

/// Queue outbound bytes and try to flush at once (the common case is a writable socket,
/// and edge-triggered polls only fire on transitions). Past [`OUT_CAP`] the connection is
/// dropped rather than buffered without bound. The connection takes its [`SendGuard`]
/// with its first send; a send for a connection already gone closes the flow, so its
/// sender is never left parked.
fn queue_send(
    id: u64,
    bytes: Vec<u8>,
    flow: Arc<OutboundFlow>,
    conns: &mut HashMap<u64, Rx>,
    registry: &mio::Registry,
) {
    let remove = match conns.get_mut(&id) {
        Some(Rx::Plain(connection)) => {
            connection.sent.get_or_insert_with(|| SendGuard(flow));
            if connection.out.is_empty() {
                connection.out_progress = Instant::now();
            }
            if connection.out.total + bytes.len() > OUT_CAP {
                // A stuck reader. Notify the current subscriber whether or not reads were
                // started — an unclaimed accepted socket write-bombed here would otherwise
                // drop silently.
                if !connection.read_done {
                    connection.owner.emit(tcp_closed_msg(id));
                    connection.read_done = true;
                }
                true
            } else {
                connection.out.push(bytes);
                connection.last_activity = Instant::now();
                drive_plain(id, connection, false, true, registry)
            }
        }
        Some(Rx::Tls(connection)) => {
            connection.sent.get_or_insert_with(|| SendGuard(flow));
            if connection.pending_out == 0 {
                connection.out_progress = Instant::now();
            }
            // Bound the plaintext handed to rustls (whose writer buffers without limit):
            // once backed up (`wants_write`) and past OUT_CAP, drop rather than grow.
            if connection.conn.wants_write() && connection.pending_out + bytes.len() > OUT_CAP {
                if !connection.read_done {
                    if connection.one_shot {
                        connection
                            .owner
                            .emit(tcp_error_msg(id, "tls: outbound buffer overflow"));
                    } else {
                        connection.owner.emit(tcp_closed_msg(id));
                    }
                    connection.read_done = true;
                }
                true
            } else {
                connection.pending_out += bytes.len();
                connection.last_activity = Instant::now();
                let _ = connection.conn.writer().write_all(&bytes);
                drive_tls(id, connection, false, true, registry)
            }
        }
        _ => {
            flow.close();
            false
        }
    };
    if remove {
        teardown(id, conns, registry);
    }
}

/// Arm (`ms` > 0) or disarm an established stream's idle timeout. A listener or a
/// not-yet-claimed accept has nothing to arm (an unclaimed accept is already bounded by
/// `ACCEPT_REAP_AFTER`).
fn set_idle(id: u64, ms: u64, conns: &mut HashMap<u64, Rx>) {
    let idle = (ms != 0).then(|| Duration::from_millis(ms));
    let (idle_slot, last_activity) = match conns.get_mut(&id) {
        Some(Rx::Plain(connection)) => (&mut connection.idle, &mut connection.last_activity),
        Some(Rx::Tls(connection)) => (&mut connection.idle, &mut connection.last_activity),
        _ => return,
    };
    *idle_slot = idle;
    *last_activity = Instant::now();
}

/// Set (`ms` > 0) or disarm how long a stream's queued output may make no progress before
/// the connection is dropped ([`SEND_STALL_TIMEOUT`] by default). The clock starts again
/// now, so shortening it never drops a connection that was within the old bound.
fn set_stall(id: u64, ms: u64, conns: &mut HashMap<u64, Rx>) {
    let stall = (ms != 0).then(|| Duration::from_millis(ms));
    let (stall_slot, out_progress) = match conns.get_mut(&id) {
        Some(Rx::Plain(connection)) => (&mut connection.stall, &mut connection.out_progress),
        Some(Rx::Tls(connection)) => (&mut connection.stall, &mut connection.out_progress),
        _ => return,
    };
    *stall_slot = stall;
    *out_progress = Instant::now();
}

/// Flush queued outbound (bounded by [`LINGER`]) and close. The socket's owner was
/// silenced before this command was queued (`retire`), so the terminal messages the
/// paths below emit, and whatever is read while lingering, reach nobody.
fn close_connection(id: u64, conns: &mut HashMap<u64, Rx>, registry: &mio::Registry) {
    let remove = match conns.get_mut(&id) {
        Some(Rx::Plain(connection)) => {
            connection.closing = Some(Instant::now() + LINGER);
            if connection.out.is_empty() {
                begin_linger(id, connection, registry)
            } else {
                drive_plain(id, connection, false, true, registry)
            }
        }
        Some(Rx::Tls(connection)) => {
            connection.closing = Some(Instant::now() + LINGER);
            if !connection.conn.wants_write() {
                tls_finish(id, connection, None)
            } else {
                drive_tls(id, connection, false, true, registry)
            }
        }
        // no linger for datagrams: one last try at what is queued, then gone
        Some(Rx::Udp(connection)) => {
            drive_udp(id, connection, false, true, registry);
            true
        }
        Some(Rx::TlsPending(_)) | Some(Rx::Listener { .. }) | Some(Rx::TlsListener { .. }) => true,
        None => false,
    };
    if remove {
        teardown(id, conns, registry);
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
        Some(Rx::Udp(c)) if !c.flow.is_paused() => {
            drive_udp(id, c, true, false, registry);
            false
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
            Rx::Udp(mut c) => {
                if c.registered {
                    let _ = registry.deregister(&mut c.socket);
                }
            }
            Rx::TlsPending(_) => {}
            Rx::Listener { listener, .. } | Rx::TlsListener { listener, .. } => {
                shut_listener(&listener, registry);
            }
        }
    }
    reg().remove(&id);
}

/// Whether output waiting since `out_progress` has gone past its `stall` bound.
fn stalled(stall: Option<Duration>, out_progress: Instant, now: Instant) -> bool {
    stall.is_some_and(|bound| now.duration_since(out_progress) >= bound)
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
    // Stalled output is reaped the same way (`SEND_STALL_TIMEOUT`).
    let mut idle_reaps: Vec<u64> = Vec::new();
    for (&id, rx) in conns.iter() {
        match rx {
            Rx::Plain(c) => {
                if let Some(t) = c.accepted_at
                    && now.duration_since(t) >= ACCEPT_REAP_AFTER
                {
                    doomed.push(id);
                }
                if let Some(deadline) = c.closing {
                    if now >= deadline {
                        doomed.push(id);
                    }
                } else if !c.out.is_empty() && stalled(c.stall, c.out_progress, now) {
                    // Output waiting and the peer has taken none of it for the bound: it
                    // has stopped reading, and its sender would wait for it forever.
                    idle_reaps.push(id);
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
                } else if c.pending_out > 0 && stalled(c.stall, c.out_progress, now) {
                    // See the plaintext arm.
                    idle_reaps.push(id);
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
                    c.owner.emit(tcp_closed_msg(id));
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

/// Bound on one address's TCP handshake in `tcp-connect`. The dial runs on a helper
/// thread, never a scheduler worker, so this bounds how long the caller waits for its
/// `[:tcp-connected …]` / `[:tcp-error …]`, not a worker stall.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Make a freshly dialled stream reactor-ready: non-blocking, Nagle off (a request
/// body would otherwise go out one delayed-ACK per record — O(size) round trips).
fn prepare_dialled(stream: &std::net::TcpStream) -> std::io::Result<()> {
    stream.set_nonblocking(true)?;
    let _ = stream.set_nodelay(true);
    Ok(())
}

/// Hand a connected stream to the reactor only if its id is still registered, with the
/// registry lock held across the hand-off: `close` and `close_process_sockets` remove
/// the entry under that lock and only then queue `Cmd::Close`, so a stream handed off
/// here is always queued ahead of any close for its id. Dropping `make_command`'s
/// stream (the entry is gone) closes it. Returns whether the hand-off happened.
fn hand_off_if_registered(id: u64, make_command: impl FnOnce() -> Cmd) -> bool {
    let registry = reg();
    if !registry.contains_key(&id) {
        return false;
    }
    reactor().cmd(make_command());
    drop(registry);
    true
}

/// Spawn a short-lived helper thread for a blocking dial. A refused thread is an error
/// for the caller (never a panic in its green process); the registry entry is undone.
fn spawn_dialler(id: u64, name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map(|_| ())
        .map_err(|error| {
            reg().remove(&id);
            std::io::Error::other(format!("cannot start connect thread: {error}"))
        })
}

fn tcp_connected_msg(id: u64) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-connected")),
        Message::Socket(id),
    ])
}

/// `(%tcp-connect host port)` — start a connect and return its socket id at once. Name
/// resolution and the (time-bounded) handshake run on a helper thread, so a slow or
/// blackholed host never holds a scheduler worker. The outcome reaches `subscriber`
/// as `[:tcp-connected sock]` — after which reads have started — or
/// `[:tcp-error sock msg]`. `tcp/connect` waits for it, so the Brood API stays
/// synchronous.
pub fn connect(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    reactor_up()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(false));
    let owner = SocketOwner::new(subscriber);
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::Stream,
            binary: binary.clone(),
            owner: owner.clone(),
            claimed: true,
            port: None,
            listener: None,
            alpn: None,
            outbound: None,
        },
    );
    // Closes the race with `reactor_died`'s sweep: the flag is set before the sweep
    // drains the registry, so an entry inserted after the sweep observes it here and
    // withdraws instead of hanging its owner on a socket nothing will ever drive.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
    let host = host.to_string();
    spawn_dialler(id, "brood-tcp-connect", move || {
        let connected = dial_tcp(&host, port, CONNECT_TIMEOUT).and_then(|stream| {
            prepare_dialled(&stream)?;
            Ok(stream)
        });
        match connected {
            Ok(std_stream) => {
                let local_port = std_stream.local_addr().ok().map(|address| address.port());
                if let Some(ctl) = reg().get_mut(&id) {
                    ctl.port = local_port;
                }
                let stream = MioStream::from_std(std_stream);
                // `[:tcp-connected]` is NOT emitted here: the reactor emits it once it
                // holds the stream (`start_stream`). Announced from this thread, the owner
                // could `tcp/send` before `Cmd::Stream` reached the reactor, and the send —
                // for an id the reactor did not know yet — was silently dropped.
                hand_off_if_registered(id, || Cmd::Stream {
                    id,
                    stream,
                    owner,
                    binary,
                });
            }
            Err(error) => fail_dial(id, &owner, &error.to_string()),
        }
    })?;
    Ok(id)
}

/// A dial that never produced a stream: withdraw the socket and tell its owner — unless
/// `close` withdrew it first. Delivered with the registry lock held, so a `close`
/// racing this either finds the entry gone and returns after the error has landed, or
/// removed it first and the error is never sent.
fn fail_dial(id: u64, owner: &SocketOwner, message: &str) {
    let mut registry = reg();
    if registry.remove(&id).is_some() {
        owner.emit(tcp_error_msg(id, message));
    }
}

/// Raise a just-bound listener's accept backlog to the system's maximum.
///
/// `std::net::TcpListener::bind` listens with a backlog of 128, so a Brood server dropped
/// connections once a burst outran its accept loop by that many, and each dropped client
/// waited out the kernel's one-second SYN retransmit: brood-benchmarks' `http` fixture
/// answered 500 concurrent GETs in 160 ms and 1 000 in 1 152 ms (2026-10-20; the Python server
/// it replaced asked for 1 024). `listen` on a listening socket only resets the backlog, and
/// the kernel clamps `SOMAXCONN` to its own ceiling (`net.core.somaxconn` on Linux).
fn widen_backlog(listener: &std::net::TcpListener) {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: `listener` owns a valid, bound, listening fd; re-listening only changes
        // the backlog. A failure leaves the old one, which is no worse than before.
        unsafe {
            libc::listen(listener.as_raw_fd(), libc::SOMAXCONN);
        }
    }
    #[cfg(not(unix))]
    let _ = listener;
}

/// `(tcp-listen host port)` — bind; connections are announced as
/// `[:tcp-accept lid client]` to `subscriber`. Port 0 = OS-assigned.
pub fn listen(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    reactor_up()?;
    let std_listener = std::net::TcpListener::bind((host, port))?;
    widen_backlog(&std_listener);
    let local = std_listener.local_addr()?.port();
    std_listener.set_nonblocking(true)?;
    let listener = listener_slot(MioListener::from_std(std_listener));
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let owner = SocketOwner::new(subscriber);
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::Listener,
            binary: Arc::new(AtomicBool::new(false)),
            owner: owner.clone(),
            claimed: true,
            port: Some(local),
            listener: Some(listener.clone()),
            alpn: None,
            outbound: None,
        },
    );
    reactor().cmd(Cmd::Listen {
        id,
        listener,
        owner,
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
        let mut registry = reg();
        match registry.get_mut(&id) {
            Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream | Kind::Udp) => {
                ctl.owner.retarget(pid);
                let was_claimed = ctl.claimed;
                ctl.claimed = true;
                registry.set_owner(id, pid);
                !was_claimed
            }
            Some(_) => {
                return Err(invalid(
                    "tcp-controlling-process: socket is a listener, not a stream",
                ));
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
/// the reliable way to run a binary server.
/// A stream may still switch mode afterwards; inheritance only decides where it starts.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let registry = reg();
    match registry.get(&id) {
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
        let registry = reg();
        match registry.get(&id) {
            Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream) => {}
            Some(_) => {
                return Err(invalid(
                    "tcp-set-idle-timeout: socket is a listener, not a stream",
                ));
            }
            None => return Err(bad_socket()),
        }
    }
    reactor().cmd(Cmd::SetIdle { id, ms });
    Ok(())
}

/// `(tcp-set-stall-timeout sock ms)` — how long a stream's queued output may make no
/// progress before the reactor drops the connection, delivering `[:tcp-closed]` (or
/// `[:tcp-error]` for a one-shot TLS client) and waking any sender waiting on it. **On by
/// default**, at [`SEND_STALL_TIMEOUT`]: it is what ends a `tcp-send` waiting on a peer
/// that stopped reading. `ms` 0 disarms it, for a peer trusted to read eventually. No-op if
/// the socket is already gone by the time the reactor applies it; errors now if it's a
/// listener.
pub fn set_stall_timeout(id: u64, ms: u64) -> std::io::Result<()> {
    {
        let registry = reg();
        match registry.get(&id) {
            Some(ctl) if matches!(ctl.kind, Kind::Stream | Kind::TlsStream) => {}
            Some(_) => {
                return Err(invalid(
                    "tcp-set-stall-timeout: socket is a listener, not a stream",
                ));
            }
            None => return Err(bad_socket()),
        }
    }
    reactor().cmd(Cmd::SetStall { id, ms });
    Ok(())
}

/// `(tcp-send sock data)` — queue `data` for the reactor to write (ADR-143), charged
/// against the socket's [`OutboundFlow`]. Returns whether `sender` (the calling process)
/// must now wait for `[:tcp-drained sock]`: the queue reached [`OUTBOUND_HIGH_WATER`],
/// and the reactor wakes it once the queue is low again or the connection is gone. A
/// write failure surfaces later as `[:tcp-closed …]`. Erroring cases the caller can know
/// now — unknown socket, a listener, an unclaimed TLS stream — still error synchronously.
pub fn send(id: u64, data: &[u8], sender: u64) -> std::io::Result<bool> {
    reactor_up()?;
    let flow = {
        let mut registry = reg();
        let Some(ctl) = registry.get_mut(&id) else {
            return Err(bad_socket());
        };
        match ctl.kind {
            Kind::Stream => {}
            Kind::TlsStream if !ctl.claimed => {
                return Err(invalid(
                    "tcp-send: TLS socket not yet claimed (tcp-controlling-process)",
                ));
            }
            Kind::TlsStream => {}
            Kind::Udp => return Err(invalid("tcp-send: a UDP socket sends with udp-send")),
            Kind::Listener | Kind::TlsListener => {
                return Err(invalid("tcp-send: socket is a listener, not a stream"));
            }
        }
        ctl.outbound
            .get_or_insert_with(|| OutboundFlow::new(id))
            .clone()
    };
    flow.charge(data.len());
    reactor().cmd(Cmd::Send {
        id,
        bytes: data.to_vec(),
        flow: flow.clone(),
    });
    Ok(flow.park_if_full(sender))
}

/// `(tcp-close sock)` — close a stream or listener. Idempotent; sends nothing.
///
/// Guaranteed when it returns:
/// - **no new message for `sock` reaches any mailbox** — no `[:tcp …]` data, no
///   `[:tcp-closed]`, `[:tcp-error]`, `[:tcp-accept]` or `[:tcp-connected]`
///   ([`SocketOwner::silence`]). Messages already delivered stay in the mailbox;
///   there is no terminal `[:tcp-closed]` for a socket you closed yourself.
/// - a listener's fd is shut, so a connect issued after it is refused.
///
/// A stream's already-queued outbound bytes keep flushing in the background (bounded by
/// [`LINGER`]), so `tcp-send` then `tcp-close` never truncates; inbound bytes read
/// meanwhile are discarded.
pub fn close(id: u64) {
    let removed = reg().remove(&id);
    if let Some(ctl) = removed {
        retire(id, ctl);
    }
}

/// Close every socket owned by green-process `pid` (scheduler `deregister`):
/// a dead owner never leaks fds or registry slots. O(sockets `pid` owns).
pub fn close_process_sockets(pid: u64) {
    let doomed = reg().remove_owned_by(pid);
    for (id, ctl) in doomed {
        retire(id, ctl);
    }
}

/// Close a socket whose control-plane entry the caller has just removed (outside the
/// registry lock: the reactor takes a listener's slot before the registry). A stream's
/// flush-then-close stays on the reactor; its owner is silenced and a listener's fd is
/// shut here and now.
fn retire(id: u64, ctl: Ctl) {
    ctl.owner.silence();
    let reactor = reactor();
    if let Some(listener) = &ctl.listener {
        shut_listener(listener, &reactor.registry);
    }
    reactor.cmd(Cmd::Close { id });
}

/// The local port `sock` is bound to.
pub fn local_port(id: u64) -> Option<u16> {
    reg().get(&id).and_then(|ctl| ctl.port)
}

/// An IP address from what a UDP call names: a literal, or `localhost`. Datagrams have
/// no dialler thread to hide a name lookup in, so a name is resolved before the call
/// (`net/dns`), never here on a worker.
fn udp_address(what: &str, host: &str, port: u16) -> std::io::Result<std::net::SocketAddr> {
    let ip = if host == "localhost" {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    } else {
        host.parse::<std::net::IpAddr>().map_err(|_| {
            invalid(&format!(
                "{what}: {host} is not an IP address (resolve a name with net/dns first)"
            ))
        })?
    };
    Ok(std::net::SocketAddr::new(ip, port))
}

/// `(udp-open host port)` — a UDP socket bound to `host`:`port` (`"0.0.0.0"` or `"::"` for
/// every interface, port 0 for one the OS picks), owned by `subscriber`: each datagram
/// arrives as `[:udp sock host port data]`, `data` bytes until `tcp-set-binary` says text.
/// Closed by `tcp-close`; handed to another process by `tcp-controlling-process`.
pub fn udp_open(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    reactor_up()?;
    let std_socket = std::net::UdpSocket::bind(udp_address("udp-open", host, port)?)?;
    std_socket.set_nonblocking(true)?;
    let local = std_socket.local_addr()?.port();
    let socket = MioUdp::from_std(std_socket);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // datagrams are binary by nature: a DNS answer or a game packet is not text
    let binary = Arc::new(AtomicBool::new(true));
    let owner = SocketOwner::new(subscriber);
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::Udp,
            binary: binary.clone(),
            owner: owner.clone(),
            claimed: true,
            port: Some(local),
            listener: None,
            alpn: None,
            outbound: None,
        },
    );
    reactor().cmd(Cmd::Udp {
        id,
        socket,
        owner,
        binary,
    });
    // See `connect` — closes the race with `reactor_died`'s sweep.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
    Ok(id)
}

/// `(udp-send sock host port data)` — send one datagram from `sock` to `host`:`port`.
/// Asynchronous like `tcp-send`; a datagram the network loses or refuses is simply gone.
pub fn udp_send(id: u64, host: &str, port: u16, data: &[u8]) -> std::io::Result<()> {
    reactor_up()?;
    let to = udp_address("udp-send", host, port)?;
    match reg().get(&id) {
        Some(ctl) if ctl.kind == Kind::Udp => {}
        Some(_) => return Err(invalid("udp-send: not a UDP socket")),
        None => return Err(bad_socket()),
    }
    reactor().cmd(Cmd::UdpSend {
        id,
        to,
        bytes: data.to_vec(),
    });
    Ok(())
}

// ---- TLS configuration + entry points ----

/// The shared client TLS config, built once: the PLATFORM's trust store — the system CA
/// bundle (`SSL_CERT_FILE` / `SSL_CERT_DIR` when set), the macOS keychain, Windows'
/// certificate store — so a CA the machine trusts (a company's, a distribution's update)
/// is trusted here too; the bundled Mozilla roots (webpki-roots) when that store yields no
/// certificate. Reading the store can take a while (the keychain), so a caller builds this
/// off the scheduler, on its dialler thread.
fn tls_config() -> Arc<ClientConfig> {
    static CFG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CFG.get_or_init(|| {
        Arc::new(
            ClientConfig::builder()
                .with_root_certificates(trust_roots(rustls_native_certs::load_native_certs().certs))
                .with_no_client_auth(),
        )
    })
    .clone()
}

/// The roots a client trusts given the platform's certificates: every one rustls can parse,
/// else — none found, or none usable — the bundled Mozilla roots.
fn trust_roots(platform: Vec<CertificateDer<'static>>) -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(platform);
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    roots
}

/// A client config trusting exactly the given PEM CA/certificate — for private
/// CAs and for talking to a `tls-self-signed` dev server (also what makes the
/// TLS loop testable end-to-end in-tree).
fn tls_config_with_ca(ca_pem: &str) -> std::io::Result<Arc<ClientConfig>> {
    let certs = CertificateDer::pem_slice_iter(ca_pem.as_bytes())
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

/// Bound on the TLS client's TCP connect. Matches `dist`'s dial timeout: long enough for
/// any healthy path, short enough that a silently-dropping host does not hold the caller's
/// registry entry and request buffer for the kernel's multi-minute SYN timeout.
const TLS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// `(tls-request host port request [ca-pem [binary]])` — one HTTPS exchange: handshake,
/// send `request` (already-flattened iolist bytes), stream the response as
/// `[:tcp id data]` … `[:tcp-closed id]` (or `[:tcp-error id msg]`). Returns
/// the id immediately; the blocking name-resolution + connect happens on a
/// short-lived helper thread, then the exchange rides the reactor. `binary` is the
/// response's starting mode: the request goes out without the caller's help, so a
/// response can arrive before a later `tcp-set-binary` lands and its first chunk
/// would decode as text. `ca_pem` (private CAs, dev certs) replaces the Mozilla roots
/// as the trust anchor for this request.
pub fn tls_request(
    host: &str,
    port: u16,
    request: Vec<u8>,
    ca_pem: Option<String>,
    binary: bool,
    subscriber: u64,
) -> std::io::Result<u64> {
    reactor_up()?;
    // a private CA is read (and refused) here; the default trust store is read on the
    // dialler thread (`tls_config`)
    let private = ca_pem.map(|pem| tls_config_with_ca(&pem)).transpose()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(binary));
    let owner = SocketOwner::new(subscriber);
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::TlsStream,
            binary: binary.clone(),
            owner: owner.clone(),
            claimed: true,
            port: None,
            listener: None,
            alpn: None,
            outbound: None,
        },
    );
    // See `connect` — closes the race with `reactor_died`'s sweep.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
    let host = host.to_string();
    spawn_dialler(id, "brood-tls-connect", move || {
        let server_name = match ServerName::try_from(host.clone()) {
            Ok(name) => name,
            Err(_) => return fail_dial(id, &owner, "tls: invalid server name"),
        };
        let config = private.unwrap_or_else(tls_config);
        let connected = dial_tcp(&host, port, TLS_CONNECT_TIMEOUT).and_then(|stream| {
            prepare_dialled(&stream)?;
            Ok(stream)
        });
        match connected {
            Ok(std_stream) => {
                // The owner may have closed this handle while we were connecting;
                // installing it then would leak an fd and a TLS session under an id
                // nothing can close (KI-97 item 4) — the hand-off checks under the
                // registry lock.
                let stream = MioStream::from_std(std_stream);
                hand_off_if_registered(id, || Cmd::TlsClient {
                    id,
                    stream,
                    owner,
                    binary,
                    server_name,
                    request,
                    config,
                    announce: None,
                });
            }
            Err(error) => fail_dial(id, &owner, &error.to_string()),
        }
    })?;
    Ok(id)
}

/// `(tls-connect host port ca alpn binary)` — a persistent TLS client stream (ADR-451): the
/// counterpart of `connect` over TLS. Once the handshake completes the owner gets
/// `[:tcp-connected sock]`; then `tcp-send`, `[:tcp sock data]` and `tcp-close` work as on
/// a plaintext stream, so a connection can carry many requests (HTTP keep-alive, a
/// WebSocket, a database protocol). `alpn` lists the protocols to offer (`["h2"
/// "http/1.1"]`); the one the server chose is `tls_protocol` once connected. A failure
/// before the handshake completes arrives as `[:tcp-error sock msg]` or `[:tcp-closed sock]`.
pub fn tls_connect(
    host: &str,
    port: u16,
    ca_pem: Option<String>,
    alpn: Vec<String>,
    binary: bool,
    subscriber: u64,
) -> std::io::Result<u64> {
    reactor_up()?;
    // as in `tls_request`: a private CA here, the default trust store on the dialler thread
    let private = ca_pem.map(|pem| tls_config_with_ca(&pem)).transpose()?;
    let with_alpn = move |base: Arc<ClientConfig>| {
        if alpn.is_empty() {
            base
        } else {
            let mut config = (*base).clone();
            config.alpn_protocols = alpn.into_iter().map(String::into_bytes).collect();
            Arc::new(config)
        }
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(binary));
    let owner = SocketOwner::new(subscriber);
    let slot = Arc::new(OnceLock::new());
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::TlsStream,
            binary: binary.clone(),
            owner: owner.clone(),
            claimed: true,
            port: None,
            listener: None,
            alpn: Some(slot.clone()),
            outbound: None,
        },
    );
    // See `connect` — closes the race with `reactor_died`'s sweep.
    if REACTOR_DOWN.load(Ordering::SeqCst) {
        reg().remove(&id);
        return Err(reactor_dead_err());
    }
    let host = host.to_string();
    spawn_dialler(id, "brood-tls-connect", move || {
        let server_name = match ServerName::try_from(host.clone()) {
            Ok(name) => name,
            Err(_) => return fail_dial(id, &owner, "tls: invalid server name"),
        };
        let config = with_alpn(private.unwrap_or_else(tls_config));
        let connected = dial_tcp(&host, port, TLS_CONNECT_TIMEOUT).and_then(|stream| {
            prepare_dialled(&stream)?;
            Ok(stream)
        });
        match connected {
            Ok(std_stream) => {
                let stream = MioStream::from_std(std_stream);
                hand_off_if_registered(id, || Cmd::TlsClient {
                    id,
                    stream,
                    owner,
                    binary,
                    server_name,
                    request: Vec::new(),
                    config,
                    announce: Some(slot),
                });
            }
            Err(error) => fail_dial(id, &owner, &error.to_string()),
        }
    })?;
    Ok(id)
}

/// The ALPN protocol a `tls-connect` stream negotiated (`"h2"`, `"http/1.1"`), or `None`:
/// not yet connected, the server chose none, or not such a stream.
pub fn tls_protocol(id: u64) -> Option<String> {
    reg()
        .get(&id)
        .and_then(|ctl| ctl.alpn.as_ref())
        .and_then(|slot| slot.get().cloned())
}

/// Build a rustls `ServerConfig` from a PEM certificate chain + private key (the app
/// supplies them; reading files/secrets is Brood-side policy).
fn build_server_config(cert_pem: &str, key_pem: &str) -> std::io::Result<Arc<ServerConfig>> {
    let certs = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| invalid(&format!("tls: bad certificate PEM: {e}")))?;
    if certs.is_empty() {
        return Err(invalid("tls: no certificates in cert PEM"));
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|e| match e {
        pem::Error::NoItemsFound => invalid("tls: no private key in key PEM"),
        e => invalid(&format!("tls: bad key PEM: {e}")),
    })?;
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
    alpn: Vec<String>,
    subscriber: u64,
) -> std::io::Result<u64> {
    reactor_up()?;
    let base = build_server_config(cert_pem, key_pem)?;
    let config = if alpn.is_empty() {
        base
    } else {
        let mut config = (*base).clone();
        config.alpn_protocols = alpn.into_iter().map(String::into_bytes).collect();
        Arc::new(config)
    };
    let std_listener = std::net::TcpListener::bind((host, port))?;
    widen_backlog(&std_listener);
    let local = std_listener.local_addr()?.port();
    std_listener.set_nonblocking(true)?;
    let listener = listener_slot(MioListener::from_std(std_listener));
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let owner = SocketOwner::new(subscriber);
    reg().insert(
        id,
        subscriber,
        Ctl {
            kind: Kind::TlsListener,
            binary: Arc::new(AtomicBool::new(false)),
            owner: owner.clone(),
            claimed: true,
            port: Some(local),
            listener: Some(listener.clone()),
            alpn: None,
            outbound: None,
        },
    );
    reactor().cmd(Cmd::TlsListen {
        id,
        listener,
        owner,
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

#[cfg(all(test, unix))]
mod backlog_tests {
    /// A Brood listener holds a burst of connections the accept loop has not reached yet.
    /// With std's backlog of 128 every connect past ~129 waited out a one-second SYN
    /// retransmit; here none is accepted, so the backlog alone must hold all 600.
    #[test]
    fn a_listener_holds_a_burst_bigger_than_std_s_backlog() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        super::widen_backlog(&listener);
        let addr = listener.local_addr().unwrap();
        let somaxconn = std::fs::read_to_string("/proc/sys/net/core/somaxconn")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(4096);
        // The kernel's ceiling bounds what any process can ask for; below 600 the
        // question this asks cannot be answered on this machine.
        if somaxconn < 600 {
            return;
        }
        let held: Vec<_> = (0..600)
            .map(|_| {
                std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(250))
            })
            .collect();
        let failed = held.iter().filter(|c| c.is_err()).count();
        assert_eq!(
            failed, 0,
            "{failed} of 600 connects did not complete: the backlog is too small"
        );
    }
}

#[cfg(test)]
mod trust_store_tests {
    use super::*;

    #[test]
    fn the_platforms_certificates_are_trusted_and_the_bundle_is_the_fallback() {
        let bundled = webpki_roots::TLS_SERVER_ROOTS.len();
        // none from the platform: the bundled Mozilla roots
        assert_eq!(trust_roots(Vec::new()).len(), bundled);
        // the platform's alone when it has any: a CA the machine added is trusted, one the
        // machine removed is not brought back by the bundle
        let (pem, _key) = tls_self_signed(vec!["localhost".to_string()]).expect("a cert");
        let platform: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("PEM");
        assert_eq!(trust_roots(platform).len(), 1);
        // unparsable certificates count as none
        let garbage = vec![CertificateDer::from(vec![0u8, 1, 2, 3])];
        assert_eq!(trust_roots(garbage).len(), bundled);
    }
}
