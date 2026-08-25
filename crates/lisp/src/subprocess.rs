//! Persistent child processes (ADR-104), built on the blocking-IO → mailbox seam
//! (ADR-059) — the same mechanism `crate::net` uses for sockets.
//!
//! `system/cmd` (`%os-cmd`) runs a child to completion and hands back its captured
//! `{:stdout :stderr :exit}`. That is the wrong shape for a long-lived co-process
//! you talk to *continuously* — an LSP server, a REPL, a formatter daemon — where
//! you write a request and read the reply, over and over, for the life of the
//! child. This module is that missing primitive: spawn a child with piped stdio,
//! write to its stdin, and receive its output as mailbox messages.
//!
//! A child never blocks a scheduler worker. Its stdout and stderr are each read on
//! a dedicated non-worker thread (`spawn_io_source`) that **delivers to the owning
//! process's mailbox**; the Brood side just `receive`s. Shapes (the handle is a
//! `Value::Subprocess`):
//!
//! - stdout: a `[:proc handle data]` message per chunk;
//! - stderr: a `[:proc-err handle data]` message per chunk (kept **separate** —
//!   merging it into stdout would corrupt a framed protocol like JSON-RPC);
//! - exit:   one `[:proc-closed handle code]` when the child exits (`code` is the
//!   integer exit status, or `nil` if it was terminated by a signal).
//!
//! Writing is a blocking `proc-send` (write the bytes to stdin + flush). Closing
//! is `proc-close`: kill the child if it is still running, drop its stdin, and let
//! the readers see EOF — the stdout reader then reaps the child and emits the
//! final `[:proc-closed …]`, so the owner learns the exit status either way.
//!
//! A subprocess is a `u64` id into a global registry, surfaced as the scalar
//! handle `Value::Subprocess(id)` (the GC never traces or moves it). Valid across
//! this runtime's processes; not node-portable (the id names an OS process on this
//! host — the dist wire codec rejects it).
//!
//! **Text mode (default) vs binary mode.** By default inbound bytes are delivered
//! as a Brood string: valid UTF-8 is preserved exactly (a multi-byte character
//! split across a read boundary is reassembled — the reader carries an incomplete
//! trailing sequence to the next read via [`chunk_payload`]), and only a genuinely
//! non-UTF-8 byte run is replaced with U+FFFD. Fine for text protocols (JSON-RPC
//! over stdio, line protocols); for a child speaking a binary protocol,
//! `proc-set-binary` switches it to **binary mode** (mirroring the socket's
//! `tcp-set-binary`): inbound `[:proc …]`/`[:proc-err …]` data is then a
//! byte-faithful first-class `bytes` value and `proc-send` accepts `bytes` too
//! (see `crate::net` and the `bytes` type).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use crate::core::value;
use crate::process::{chunk_flush, chunk_payload, spawn_io_source, Message};

/// A live child process: the write half (its stdin) plus a shared handle to the
/// `Child` itself, used to reap (`wait`) and to `kill`. The stdout/stderr read
/// halves are owned by their reader threads, not held here.
struct Proc {
    /// The child's stdin, behind its own lock so a blocking `proc-send` write
    /// serializes per-child **without** holding the global registry lock — a
    /// child that never drains its stdin must not stall every other `proc-*` op.
    /// Dropped (sending EOF to the child) when the entry is removed.
    stdin: Arc<Mutex<ChildStdin>>,
    /// Shared with the stdout reader, which reaps the child on EOF. `proc-close`
    /// locks it briefly to `kill`; the reader locks it briefly to `try_wait`.
    /// Never held across a blocking call, so the two never deadlock.
    child: Arc<ChildHandle>,
    /// Inbound decode mode (default text; mirrors `net`'s socket flag, ADR-141:
    /// outbound `proc-send` is unaffected — string leaves are always UTF-8).
    /// Binary mode delivers `[:proc …]` data as byte-faithful `bytes` values.
    /// Shared with the reader threads, which load it per chunk, so
    /// `proc-set-binary` flips an already-running child mid-stream.
    binary: Arc<AtomicBool>,
}

/// The `Child` plus a condvar the reaper waits on.
///
/// `std::process::Child::wait` needs `&mut Child`, so a reaper blocking in it would hold
/// this mutex for the child's whole life and `proc-close` could never take it to `kill` —
/// which is why the reaper polls `try_wait` instead. The condvar makes that poll a real
/// *wait*: [`close`] signals it after killing, so a kill is observed at once, and the
/// residual poll (for a child that exits on its own after closing stdout) backs off
/// geometrically to [`REAP_POLL_MAX`] instead of running a 5 ms spin for the whole
/// lifetime of a daemon-style child.
struct ChildHandle {
    child: Mutex<Child>,
    /// Notified by [`close`] once it has killed the child, so the reaper stops waiting.
    killed: std::sync::Condvar,
}

/// First reap-poll interval after stdout EOF, and the cap it backs off to. The child has
/// almost always already exited by then, so the first `try_wait` answers and neither is
/// reached; these only bound the *daemon-closed-stdout-early* case.
const REAP_POLL_MIN: Duration = Duration::from_millis(5);
const REAP_POLL_MAX: Duration = Duration::from_millis(500);

static REGISTRY: LazyLock<Mutex<HashMap<u64, Proc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Proc>> {
    REGISTRY.lock().expect("subprocess registry mutex")
}

// ---- message builders (off-heap; symbols are a global interner) ----

/// Wrap a decoded [`chunk_payload`] result in a `[:proc handle data]` (stdout) or
/// `[:proc-err handle data]` (stderr) message. The text/binary decode and the
/// UTF-8 carry-across-reads live in `chunk_payload`; this just tags the payload.
fn data_msg(tag: &str, id: u64, payload: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Subprocess(id),
        payload,
    ])
}

/// Build the `[:proc-closed handle code]` message. `code` is the integer exit
/// status, or `nil` when the child was terminated by a signal (no exit code).
fn closed_msg(id: u64, code: Option<i32>) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("proc-closed")),
        Message::Subprocess(id),
        code.map(|c| Message::Int(c as i64)).unwrap_or(Message::Nil),
    ])
}

// ---- reader threads ----

/// Read `src` to EOF on a non-worker thread, emitting one `[<tag> id data]`
/// message per chunk to `subscriber`. Used for both stdout (`:proc`) and stderr
/// (`:proc-err`).
fn start_pipe_reader<R: Read + Send + 'static>(
    id: u64,
    tag: &'static str,
    src: R,
    subscriber: u64,
    binary: Arc<AtomicBool>,
) {
    spawn_io_source(subscriber, "brood-proc-reader", move |sink| {
        let mut rd = src;
        let mut buf = [0u8; 65536];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match rd.read(&mut buf) {
                Ok(0) => {
                    if let Some(p) = chunk_flush(&mut carry) {
                        sink.emit(data_msg(tag, id, p));
                    }
                    break;
                }
                Ok(n) => {
                    let bin = binary.load(Ordering::Acquire);
                    if let Some(p) = chunk_payload(&mut carry, &buf[..n], bin) {
                        sink.emit(data_msg(tag, id, p));
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// The stdout reader: stream stdout as `[:proc id data]`, then **reap** the child
/// and emit the final `[:proc-closed id code]`. It owns the reap so there is
/// exactly one waiter (the stderr reader never waits — it would race for the
/// zombie). After stdout EOF the child has usually already exited, so one `try_wait`
/// answers; a child that merely *closed* stdout (a daemon) is waited for on the
/// [`ChildHandle::killed`] condvar — which releases the mutex, so a concurrent
/// `proc-close`/`kill` can always take it, and wakes the reaper immediately when it
/// does. Deferring `[:proc-closed]` to the child's actual exit is the contract; what
/// changed is that waiting for it no longer costs a 200 Hz poll for the child's whole
/// lifetime. On exit, drop the registry entry.
fn start_stdout_reader(
    id: u64,
    out: ChildStdout,
    child: Arc<ChildHandle>,
    subscriber: u64,
    binary: Arc<AtomicBool>,
) {
    spawn_io_source(subscriber, "brood-proc-stdout", move |sink| {
        let mut rd = out;
        let mut buf = [0u8; 65536];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            match rd.read(&mut buf) {
                Ok(0) => {
                    if let Some(p) = chunk_flush(&mut carry) {
                        sink.emit(data_msg("proc", id, p));
                    }
                    break;
                }
                Ok(n) => {
                    let bin = binary.load(Ordering::Acquire);
                    if let Some(p) = chunk_payload(&mut carry, &buf[..n], bin) {
                        sink.emit(data_msg("proc", id, p));
                    }
                }
                Err(_) => break,
            }
        }
        // stdout is at EOF: reap the child for its exit status.
        let mut guard = child.child.lock().expect("subprocess child mutex");
        let mut backoff = REAP_POLL_MIN;
        let code = loop {
            match guard.try_wait() {
                Ok(Some(st)) => break st.code(),
                // Not exited yet — the child closed stdout but is still running. Wait on
                // the condvar (which RELEASES the mutex, so `proc-close` can take it to
                // `kill`, and wakes us the moment it has). The timeout is only the
                // backstop for a child that exits by itself with nobody to signal us; it
                // grows to `REAP_POLL_MAX` so a long-lived daemon child costs a couple of
                // wakeups a second rather than 200.
                Ok(None) => {
                    let (g, _) = child
                        .killed
                        .wait_timeout(guard, backoff)
                        .expect("subprocess child mutex");
                    guard = g;
                    backoff = (backoff * 2).min(REAP_POLL_MAX);
                }
                // wait() failed (already reaped elsewhere, etc.): give up cleanly.
                Err(_) => break None,
            }
        };
        drop(guard);
        reg().remove(&id);
        sink.emit(closed_msg(id, code));
    });
}

// ---- the primitive operations ----

/// `(proc/spawn prog args opts)` — spawn `prog` with `args`, piping its stdin/
/// stdout/stderr. `cwd` (if set) is the child's working directory; otherwise it
/// inherits ours. `env` entries are added on top of the inherited environment.
/// The stdout/stderr readers deliver to `subscriber`. Returns the handle id.
/// Errors if the program can't be spawned (not found, not executable, …).
pub fn spawn(
    prog: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &[(String, String)],
    subscriber: u64,
) -> std::io::Result<u64> {
    let mut command = Command::new(prog);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command.spawn()?;
    // Take the three pipe ends; piped() guarantees they are Some.
    let stdin: ChildStdin = child.stdin.take().expect("piped stdin");
    let stdout: ChildStdout = child.stdout.take().expect("piped stdout");
    let stderr: ChildStderr = child.stderr.take().expect("piped stderr");

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Arc::new(ChildHandle {
        child: Mutex::new(child),
        killed: std::sync::Condvar::new(),
    });
    let binary = Arc::new(AtomicBool::new(false));
    reg().insert(
        id,
        Proc {
            stdin: Arc::new(Mutex::new(stdin)),
            child: child.clone(),
            binary: binary.clone(),
        },
    );
    start_stdout_reader(id, stdout, child, subscriber, binary.clone());
    start_pipe_reader(id, "proc-err", stderr, subscriber, binary);
    Ok(id)
}

/// `(proc-set-binary handle on)` — switch `handle` between text mode (default)
/// and binary mode. Binary mode is byte-faithful both directions: inbound
/// `[:proc …]`/`[:proc-err …]` data is a Latin-1 byte-string (one codepoint
/// 0–255 per byte) and `proc-send` writes codepoints as raw bytes. Errors if the
/// handle is unknown (already closed). Mirrors `net::set_binary`.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let reg = reg();
    match reg.get(&id) {
        Some(p) => {
            p.binary.store(on, Ordering::Release);
            Ok(())
        }
        None => Err(bad_proc()),
    }
}

/// `(proc-send handle data)` — write all of `data` to the child's stdin (blocking)
/// and flush. Errors if the handle is unknown or its stdin is closed.
pub fn send(id: u64, data: &[u8]) -> std::io::Result<()> {
    // Clone the stdin handle out under a brief registry lock, then write outside
    // it: a pipe write is bounded by the OS buffer, so a child that never drains
    // its stdin would block here (the blocking contract `tcp-send` also has) —
    // but only this child's stdin lock is held, never the global registry lock.
    let stdin = {
        let reg = reg();
        match reg.get(&id) {
            Some(p) => p.stdin.clone(),
            None => return Err(bad_proc()),
        }
    };
    let mut stdin = stdin.lock().expect("subprocess stdin mutex");
    stdin.write_all(data)?;
    stdin.flush()
}

/// `(proc-close handle)` — terminate the child: kill it if still running, drop its
/// stdin (EOF). Idempotent. The stdout reader sees EOF, reaps, and emits the final
/// `[:proc-closed …]`; this call does not wait for that.
pub fn close(id: u64) {
    let removed = {
        let mut reg = reg();
        reg.remove(&id)
    };
    if let Some(Proc { child, .. }) = removed {
        // Brief lock (kill doesn't block) — the stdout reaper waits on the condvar rather
        // than holding this mutex across a blocking `wait()`, so we never contend.
        {
            let mut c = child.child.lock().expect("subprocess child mutex");
            let _ = c.kill();
        }
        // Rouse the reaper so it reaps *now* instead of at its next backoff tick, and the
        // `[:proc-closed …]` follows the kill promptly.
        child.killed.notify_all();
        // `stdin` (in `removed`) drops here, sending EOF to the child too.
    }
}

fn bad_proc() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no such subprocess (already closed?)",
    )
}
