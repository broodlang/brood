//! TCP sockets (ADR-062), built on the blocking-IO → mailbox seam (ADR-059).
//!
//! A socket never blocks a worker. A connected stream is read on a dedicated
//! non-worker thread (`spawn_io_source`) that **delivers to the owning process's
//! mailbox**; the Brood side just `receive`s. Shapes:
//!
//! - a stream delivers `[:tcp sock data]` per chunk, then `[:tcp-closed sock]`;
//! - a listener delivers `[:tcp-accept lsock client]` per connection.
//!
//! Ownership: `tcp-connect` makes an **active** stream — its reader starts at once,
//! delivering to the connecting process. An **accepted** stream is **passive** —
//! announced via `[:tcp-accept …]` but not read until `tcp-controlling-process`
//! assigns it an owner (then its reader starts, delivering there). This is the
//! Erlang `gen_tcp` handoff: no inbound bytes are lost to the acceptor before a
//! per-connection handler takes over.
//!
//! A socket is a `u64` id into a global registry, surfaced as the scalar handle
//! `Value::Socket(id)` (the GC never traces or moves it). Valid across this
//! runtime's processes; not node-portable.
//!
//! **TEXT MODE (default) vs BINARY MODE.** A stream is created in *text mode*:
//! inbound bytes are delivered as a Brood string (`Message::Str`) via
//! `from_utf8_lossy`, so any byte sequence that isn't valid UTF-8 is **silently
//! corrupted** (each bad run becomes U+FFFD) and outbound `tcp-send` writes the
//! string's UTF-8. That's right for text protocols (HTTP headers, line protocols;
//! the distributed-node handshake is on its *own* codec) and unsafe for binary
//! ones (raw images, compressed/encrypted streams, length-prefixed binary framing).
//!
//! `tcp-set-binary` switches a socket to *binary mode*, which is byte-faithful in
//! both directions without a new value kind: Brood strings are sequences of Unicode
//! codepoints, so we use the **Latin-1 subset** (codepoints 0–255) as a one-byte-
//! per-codepoint byte carrier. Inbound, each received byte becomes codepoint b
//! (no UTF-8 interpretation); outbound, `tcp-send` writes each codepoint 0–255 as
//! one raw byte (and errors on a codepoint > 255). That's enough for WebSocket
//! framing (control bytes ≥ 0x80, length-prefixed binary frames): the caller
//! UTF-8-encodes any text payload into this byte-string form itself. A general
//! bytes/blob value kind is still a separate, larger language-surface decision
//! (see CLAUDE.md / `docs/types.md`); this is the pragmatic seam until then.
//! See `tcp_data_msg` and `set_binary`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::core::value;
use crate::process::{spawn_io_source, MailboxSink, Message, SubscriberHandle};

enum Sock {
    /// A connected stream — the write/close handle, plus the reader's retarget
    /// handle once started (`None` for a freshly accepted, still-passive socket).
    /// `accepted_at` is `Some(when)` only while the socket is a passive,
    /// **unclaimed** accepted stream: the reaper drops it if no
    /// `tcp-controlling-process` claims it within [`ACCEPT_REAP_AFTER`]. It is
    /// `None` for an actively-connected stream and is cleared to `None` the
    /// moment a passive socket is claimed — so a claimed socket is never reaped.
    Stream {
        stream: TcpStream,
        reader: Option<SubscriberHandle>,
        accepted_at: Option<Instant>,
        /// Text mode (false, default): inbound is a UTF-8-lossy string, outbound
        /// is UTF-8. Binary mode (true): inbound is a Latin-1 string (one
        /// codepoint 0–255 per byte received) and `tcp-send` writes each codepoint
        /// 0–255 as one raw byte — byte-faithful, for length-prefixed / control-
        /// byte protocols (WebSocket framing). The reader thread reads this per
        /// chunk, so `tcp-set-binary` flips an already-running socket mid-stream.
        binary: Arc<AtomicBool>,
        /// The green-process pid that owns this socket. Set when the socket is
        /// created and updated by `controlling_process`; when that process dies
        /// `close_process_sockets` shuts the socket down, so a dead owner never
        /// leaks its fd — a worker that crashes mid-connection, a listener whose
        /// process is killed, etc.
        owner: u64,
    },
    /// A listening socket — the accept thread owns the `TcpListener`; `alive`
    /// stops it on close. `port` is cached so `local-port` works without it.
    Listener {
        alive: Arc<AtomicBool>,
        port: u16,
        owner: u64,
    },
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Sock>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Sock>> {
    REGISTRY.lock().expect("socket registry mutex")
}

/// How long a passively-accepted socket may sit in the registry **unclaimed**
/// (announced via `[:tcp-accept …]` but never handed an owner with
/// `tcp-controlling-process`) before the reaper drops it. Without this, a peer
/// that opens connections an application never accepts would leak an fd + a
/// registry entry per connection forever — a DoS surface for any server built on
/// this mechanism. 30 s is generous for a handler to claim a fresh connection,
/// while still bounding the leak. Claimed/active sockets (`accepted_at == None`)
/// are never reaped.
const ACCEPT_REAP_AFTER: Duration = Duration::from_secs(30);

/// Drop every passive, unclaimed accepted socket older than [`ACCEPT_REAP_AFTER`].
/// Called on each accept tick (cheap: it only inspects entries, and there's an
/// accept tick exactly when new entries appear). Shutting the stream down here
/// releases the fd; a later `tcp-controlling-process`/`tcp-send` on the reaped id
/// just gets `bad_socket()`. Only `accepted_at: Some(_)` entries are candidates,
/// so an actively-connected or already-claimed socket is untouched.
fn reap_unclaimed(reg: &mut HashMap<u64, Sock>) {
    let now = Instant::now();
    let mut doomed = Vec::new();
    for (&id, sock) in reg.iter() {
        if let Sock::Stream {
            accepted_at: Some(t),
            ..
        } = sock
        {
            if now.duration_since(*t) >= ACCEPT_REAP_AFTER {
                doomed.push(id);
            }
        }
    }
    for id in doomed {
        if let Some(Sock::Stream { stream, .. }) = reg.remove(&id) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

// ---- message builders (off-heap; symbols are a global interner) ----

/// Build the `[:tcp sock data]` message for an inbound chunk.
///
/// BINARY-UNSAFE: `data` is forced through `from_utf8_lossy`, so any non-UTF-8
/// bytes are **silently replaced** with U+FFFD — the delivered string is *not*
/// byte-faithful for binary payloads. This is a known limitation of the text-only
/// socket mechanism (see the module doc): Brood has no arbitrary-bytes value kind
/// to carry raw bytes, and adding one is a language-surface decision, not a fix
/// to make here. Lossless for valid UTF-8 (text protocols); lossy otherwise.
fn tcp_data_msg(id: u64, bytes: &[u8], binary: bool) -> Message {
    let data = if binary {
        // Byte-faithful (binary mode): map each byte to its Latin-1 codepoint
        // (0–255), so the delivered string holds the exact bytes received — one
        // codepoint per byte, no UTF-8 interpretation. The inverse of the Latin-1
        // encode `tcp-send` does for a binary socket.
        bytes.iter().map(|&b| b as char).collect()
    } else {
        // Text mode (default): UTF-8, lossy for non-UTF-8 (see module doc).
        String::from_utf8_lossy(bytes).into_owned()
    };
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp")),
        Message::Socket(id),
        Message::Str(data),
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

// ---- reader / accept threads ----

/// Start a reader thread for the already-cloned `reader` handle of socket `id`,
/// delivering `[:tcp id data]` / `[:tcp-closed id]` to `subscriber`. Returns the
/// retarget handle.
fn start_reader(
    id: u64,
    reader: TcpStream,
    subscriber: u64,
    binary: Arc<AtomicBool>,
) -> SubscriberHandle {
    spawn_io_source(subscriber, "brood-tcp-reader", move |sink| {
        let mut rd = reader;
        let mut buf = [0u8; 65536];
        loop {
            match rd.read(&mut buf) {
                Ok(0) => {
                    sink.emit(tcp_closed_msg(id));
                    break;
                }
                Ok(n) => sink.emit(tcp_data_msg(id, &buf[..n], binary.load(Ordering::Relaxed))),
                Err(_) => {
                    sink.emit(tcp_closed_msg(id));
                    break;
                }
            }
        }
    })
}

fn accept_loop(
    lid: u64,
    listener: TcpListener,
    alive: Arc<AtomicBool>,
    owner: u64,
    sink: &MailboxSink,
) {
    while alive.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Register the accepted stream **passive** (no reader yet) and
                // announce it; the owner calls `tcp-controlling-process` to start
                // reading. Avoids losing early bytes before a handler takes over.
                let cid = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                {
                    let mut reg = reg();
                    // Stamp the accept time so the reaper can drop this entry if
                    // no owner claims it (an unclaimed passive socket otherwise
                    // leaks its fd + registry slot forever — a DoS surface). The
                    // sweep is cheap, so piggyback it on this same accept tick.
                    reg.insert(
                        cid,
                        Sock::Stream {
                            stream,
                            reader: None,
                            accepted_at: Some(Instant::now()),
                            binary: Arc::new(AtomicBool::new(false)),
                            // Owned by the listener's process until a handler claims it
                            // with `tcp-controlling-process` (which retargets owner).
                            // So if the listener dies before a claim, the unclaimed
                            // socket is cleaned up with it (not just by the reaper).
                            owner,
                        },
                    );
                    reap_unclaimed(&mut reg);
                }
                sink.emit(tcp_accept_msg(lid, cid));
            }
            // Non-blocking listener: nothing waiting — nap on this dedicated
            // (non-worker) thread and re-check `alive`, so `close` can stop us.
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(_) => break,
        }
    }
}

// ---- the primitive operations ----

/// `(tcp-connect host port)` — blocking connect; an **active** reader delivers
/// inbound data to `subscriber`. Returns the socket id.
pub fn connect(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    let stream = TcpStream::connect((host, port))?;
    let reader = stream.try_clone()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let binary = Arc::new(AtomicBool::new(false));
    let handle = start_reader(id, reader, subscriber, binary.clone());
    reg().insert(
        id,
        Sock::Stream {
            stream,
            reader: Some(handle),
            accepted_at: None, // actively connected → never reaped
            binary,
            owner: subscriber,
        },
    );
    Ok(id)
}

/// `(tcp-listen host port)` — bind and start an accept thread delivering
/// `[:tcp-accept lid client]` to `subscriber`. Port 0 = OS-assigned.
pub fn listen(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    let listener = TcpListener::bind((host, port))?;
    let local = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let alive = Arc::new(AtomicBool::new(true));
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    reg().insert(
        id,
        Sock::Listener {
            alive: alive.clone(),
            port: local,
            owner: subscriber,
        },
    );
    spawn_io_source(subscriber, "brood-tcp-accept", move |sink| {
        accept_loop(id, listener, alive, subscriber, sink)
    });
    Ok(id)
}

/// `(tcp-controlling-process sock pid)` — make `pid` the owner of `sock`'s inbound
/// data. For a passive (just-accepted) socket this **starts** its reader; for an
/// already-active socket it retargets delivery. Errors if `sock` is a listener or
/// is gone.
pub fn controlling_process(id: u64, pid: u64) -> std::io::Result<()> {
    let mut reg = reg();
    match reg.get_mut(&id) {
        Some(Sock::Stream {
            stream,
            reader,
            accepted_at,
            binary,
            owner,
        }) => {
            match reader {
                Some(h) => h.retarget(pid),
                None => {
                    let clone = stream.try_clone()?;
                    *reader = Some(start_reader(id, clone, pid, binary.clone()));
                }
            }
            // Claimed now: clear the accept stamp so the reaper never drops it, and
            // hand ownership to the claiming process so the socket dies with it.
            *accepted_at = None;
            *owner = pid;
            Ok(())
        }
        Some(Sock::Listener { .. }) => Err(invalid(
            "tcp-controlling-process: socket is a listener, not a stream",
        )),
        None => Err(bad_socket()),
    }
}

/// `(tcp-set-binary sock on)` — switch `sock` between text mode (default) and
/// binary mode. Binary mode is byte-faithful in both directions: inbound `[:tcp …]`
/// data is a Latin-1 string (one codepoint 0–255 per byte received) and `tcp-send`
/// writes each codepoint 0–255 as one raw byte. For length-prefixed / control-byte
/// protocols (WebSocket framing). The reader reads the flag per chunk, so this
/// takes effect for the next inbound chunk. Errors if `sock` is gone or a listener.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let reg = reg();
    match reg.get(&id) {
        Some(Sock::Stream { binary, .. }) => {
            binary.store(on, Ordering::Relaxed);
            Ok(())
        }
        Some(Sock::Listener { .. }) => Err(invalid(
            "tcp-set-binary: socket is a listener, not a stream",
        )),
        None => Err(bad_socket()),
    }
}

/// Whether `sock` is in binary mode. A missing or listener socket reports `false`
/// (text mode) — `tcp-send` then falls back to UTF-8 and surfaces the real error.
pub fn is_binary(id: u64) -> bool {
    match reg().get(&id) {
        Some(Sock::Stream { binary, .. }) => binary.load(Ordering::Relaxed),
        _ => false,
    }
}

/// `(tcp-send sock data)` — write all of `data` (blocking; clones the handle so
/// the registry lock isn't held during the write).
pub fn send(id: u64, data: &[u8]) -> std::io::Result<()> {
    let stream = {
        let reg = reg();
        match reg.get(&id) {
            Some(Sock::Stream { stream, .. }) => stream.try_clone()?,
            Some(Sock::Listener { .. }) => {
                return Err(invalid("tcp-send: socket is a listener, not a stream"))
            }
            None => return Err(bad_socket()),
        }
    };
    (&stream).write_all(data)?;
    (&stream).flush()
}

/// `(tcp-close sock)` — shut a stream down (its reader, if any, sees EOF and
/// exits) or stop a listener's accept loop. Idempotent.
pub fn close(id: u64) {
    let removed = reg().remove(&id);
    match removed {
        Some(Sock::Stream { stream, .. }) => {
            let _ = stream.shutdown(Shutdown::Both);
        }
        Some(Sock::Listener { alive, .. }) => alive.store(false, Ordering::Relaxed),
        None => {}
    }
}

/// Close every socket owned by green-process `pid`. Called from the scheduler's
/// once-per-death `deregister`, so a process that dies (crash, kill, normal exit)
/// without `tcp-close`ing its sockets doesn't leak them: a stream is shut down, a
/// listener's accept loop is stopped (freeing the bound port). The mirror of letting
/// a process's fds be reclaimed when it exits in an OS process model.
pub fn close_process_sockets(pid: u64) {
    let mut reg = reg();
    let doomed: Vec<u64> = reg
        .iter()
        .filter_map(|(&id, sock)| {
            let owner = match sock {
                Sock::Stream { owner, .. } => *owner,
                Sock::Listener { owner, .. } => *owner,
            };
            if owner == pid {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    for id in doomed {
        match reg.remove(&id) {
            Some(Sock::Stream { stream, .. }) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Some(Sock::Listener { alive, .. }) => alive.store(false, Ordering::Relaxed),
            None => {}
        }
    }
}

/// The local port `sock` is bound to.
pub fn local_port(id: u64) -> Option<u16> {
    let reg = reg();
    match reg.get(&id)? {
        Sock::Stream { stream, .. } => stream.local_addr().ok().map(|a| a.port()),
        Sock::Listener { port, .. } => Some(*port),
    }
}

// ---- TLS client (https), one-shot request/response (ADR-062) ----
//
// rustls connections can't be split read/write across threads like a raw fd, so
// the streaming socket model doesn't map cleanly to TLS. But an HTTPS client call
// is request→response, which IS sequential: connect, handshake, write the request,
// read the response to EOF. `tls-request` runs exactly that on one non-worker
// thread and delivers the response as the same `[:tcp id data]` / `[:tcp-closed
// id]` messages a plaintext socket does — so `tcp-drain` and the HTTP parser work
// unchanged. Errors arrive as `[:tcp-error id msg]`.

fn tcp_error_msg(id: u64, msg: &str) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp-error")),
        Message::Socket(id),
        Message::Str(msg.to_string()),
    ])
}

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

/// Connect + TLS handshake + write `request` + stream the response to `sink` as
/// `[:tcp id data]` chunks. Returns Ok at clean EOF (caller emits `[:tcp-closed]`).
fn tls_exchange(
    host: &str,
    port: u16,
    request: &str,
    id: u64,
    sink: &MailboxSink,
) -> std::io::Result<()> {
    let stream = TcpStream::connect((host, port))?;
    let server_name =
        ServerName::try_from(host.to_string()).map_err(|_| invalid("tls: invalid server name"))?;
    let conn = ClientConnection::new(tls_config(), server_name)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut tls = StreamOwned::new(conn, stream);
    tls.write_all(request.as_bytes())?;
    tls.flush()?;
    let mut buf = [0u8; 65536];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            // TLS is a one-shot request socket with no binary toggle: keep the
            // text-mode (UTF-8-lossy) delivery it has always used.
            Ok(n) => sink.emit(tcp_data_msg(id, &buf[..n], false)),
            // Many servers drop the connection without a TLS close_notify; treat
            // that as the end of the response, not an error.
            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `(tls-request host port request)` — perform one HTTPS request on a non-worker
/// thread; the response arrives at the calling process as `[:tcp id data]` …
/// `[:tcp-closed id]` (or `[:tcp-error id msg]`). Returns the id immediately.
pub fn tls_request(host: &str, port: u16, request: String, subscriber: u64) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let host = host.to_string();
    spawn_io_source(
        subscriber,
        "brood-tls-request",
        move |sink| match tls_exchange(&host, port, &request, id, sink) {
            Ok(()) => sink.emit(tcp_closed_msg(id)),
            Err(e) => sink.emit(tcp_error_msg(id, &e.to_string())),
        },
    );
    id
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
