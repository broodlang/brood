//! TCP sockets (ADR-062), built on the blocking-IO → mailbox seam (ADR-059).
//!
//! A socket never blocks a worker. Inbound data is read on a dedicated non-worker
//! thread (`spawn_io_source`) and **delivered to the owning process's mailbox** as
//! messages; the Brood side just `receive`s them. The owner is the process that
//! `tcp-connect`ed (or that owns the listener, for accepted connections):
//!
//! - a connected/accepted stream delivers `[:tcp sock data]` per chunk, then
//!   `[:tcp-closed sock]` at EOF / error;
//! - a listener delivers `[:tcp-accept lsock client]` for each connection (the
//!   client stream is already wired to the same owner).
//!
//! Outbound writes (`send`) and `close` are direct calls. A socket is a `u64` id
//! into a global registry, surfaced as the scalar handle `Value::Socket(id)`
//! (the GC never traces or moves it). Valid across this runtime's processes; not
//! node-portable.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use crate::core::value;
use crate::process::{spawn_io_source, MailboxSink, Message};

enum Sock {
    /// A connected stream — the write/close handle. The reader thread holds an
    /// independent clone of the same fd for blocking reads.
    Stream(TcpStream),
    /// A listening socket — the accept thread owns the `TcpListener`; `alive`
    /// stops it on close. `port` is cached so `local-port` works without it.
    Listener { alive: Arc<AtomicBool>, port: u16 },
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Sock>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Sock>> {
    REGISTRY.lock().expect("socket registry mutex")
}

// ---- message builders (off-heap; symbols are a global interner) ----

fn tcp_data_msg(id: u64, bytes: &[u8]) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("tcp")),
        Message::Socket(id),
        Message::Str(String::from_utf8_lossy(bytes).into_owned()),
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

/// Register `stream` as a connected socket and start its reader thread, which
/// delivers `[:tcp id data]` / `[:tcp-closed id]` to `subscriber`. Returns the id.
fn register_stream(stream: TcpStream, subscriber: u64) -> std::io::Result<u64> {
    let reader = stream.try_clone()?; // independent handle, same fd
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    reg().insert(id, Sock::Stream(stream));
    spawn_io_source(subscriber, "brood-tcp-reader", move |sink| {
        let mut rd = reader;
        let mut buf = [0u8; 65536];
        loop {
            match rd.read(&mut buf) {
                Ok(0) => {
                    sink.emit(tcp_closed_msg(id));
                    break;
                }
                Ok(n) => sink.emit(tcp_data_msg(id, &buf[..n])),
                Err(_) => {
                    sink.emit(tcp_closed_msg(id));
                    break;
                }
            }
        }
    });
    Ok(id)
}

fn accept_loop(lid: u64, listener: TcpListener, alive: Arc<AtomicBool>, sink: &MailboxSink) {
    while alive.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Wire the accepted stream to the same owner, then announce it.
                if let Ok(cid) = register_stream(stream, sink.subscriber()) {
                    sink.emit(tcp_accept_msg(lid, cid));
                }
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

/// `(tcp-connect host port)` — blocking connect, then a reader thread delivers
/// inbound data to `subscriber`. Returns the socket id.
pub fn connect(host: &str, port: u16, subscriber: u64) -> std::io::Result<u64> {
    let stream = TcpStream::connect((host, port))?;
    register_stream(stream, subscriber)
}

/// `(tcp-listen host port)` — bind and start an accept thread delivering
/// `[:tcp-accept lid client]` (and wiring each client) to `subscriber`. Port 0
/// asks the OS for an ephemeral port.
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
        },
    );
    spawn_io_source(subscriber, "brood-tcp-accept", move |sink| {
        accept_loop(id, listener, alive, sink)
    });
    Ok(id)
}

/// `(tcp-send sock data)` — write all of `data` (blocking; clones the handle so
/// the registry lock isn't held during the write).
pub fn send(id: u64, data: &[u8]) -> std::io::Result<()> {
    let stream = {
        let reg = reg();
        match reg.get(&id) {
            Some(Sock::Stream(s)) => s.try_clone()?,
            Some(Sock::Listener { .. }) => {
                return Err(invalid("tcp-send: socket is a listener, not a stream"))
            }
            None => return Err(bad_socket()),
        }
    };
    (&stream).write_all(data)?;
    (&stream).flush()
}

/// `(tcp-close sock)` — shut a stream down (its reader sees EOF and exits) or stop
/// a listener's accept loop. Idempotent.
pub fn close(id: u64) {
    let removed = reg().remove(&id);
    match removed {
        Some(Sock::Stream(s)) => {
            let _ = s.shutdown(Shutdown::Both);
        }
        Some(Sock::Listener { alive, .. }) => alive.store(false, Ordering::Relaxed),
        None => {}
    }
}

/// The local port `sock` is bound to.
pub fn local_port(id: u64) -> Option<u16> {
    let reg = reg();
    match reg.get(&id)? {
        Sock::Stream(s) => s.local_addr().ok().map(|a| a.port()),
        Sock::Listener { port, .. } => Some(*port),
    }
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
