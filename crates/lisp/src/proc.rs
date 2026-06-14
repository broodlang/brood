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
//! **TEXT-ONLY MECHANISM — BINARY-UNSAFE.** Like the socket mechanism, inbound
//! bytes are delivered as a Brood string via `from_utf8_lossy`: any byte run that
//! isn't valid UTF-8 is **silently replaced** with U+FFFD. This is fine for text
//! protocols (JSON-RPC over stdio, line protocols) and unsafe for binary ones.
//! Brood has no arbitrary-bytes value kind to carry raw bytes; adding one is a
//! language-surface decision, not a fix to make here (see `crate::net`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use crate::core::value;
use crate::process::{spawn_io_source, Message};

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
    child: Arc<Mutex<Child>>,
}

static REGISTRY: LazyLock<Mutex<HashMap<u64, Proc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Proc>> {
    REGISTRY.lock().expect("subprocess registry mutex")
}

// ---- message builders (off-heap; symbols are a global interner) ----

/// Build a `[:proc handle data]` (stdout) or `[:proc-err handle data]` (stderr)
/// message for an inbound chunk.
///
/// BINARY-UNSAFE: `bytes` is forced through `from_utf8_lossy` (see the module
/// doc) — lossless for UTF-8 text, lossy otherwise.
fn data_msg(tag: &str, id: u64, bytes: &[u8]) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Subprocess(id),
        Message::Str(String::from_utf8_lossy(bytes).into_owned()),
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
fn start_pipe_reader<R: Read + Send + 'static>(id: u64, tag: &'static str, src: R, subscriber: u64) {
    spawn_io_source(subscriber, "brood-proc-reader", move |sink| {
        let mut rd = src;
        let mut buf = [0u8; 65536];
        loop {
            match rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => sink.emit(data_msg(tag, id, &buf[..n])),
                Err(_) => break,
            }
        }
    });
}

/// The stdout reader: stream stdout as `[:proc id data]`, then **reap** the child
/// and emit the final `[:proc-closed id code]`. It owns the reap so there is
/// exactly one waiter (the stderr reader never waits — it would race for the
/// zombie). After stdout EOF the child has exited or is exiting; poll `try_wait`
/// with a brief lock + short nap (never holding the lock while blocked) so a
/// concurrent `proc-close`/`kill` can always take the lock. On exit, drop the
/// registry entry.
fn start_stdout_reader(id: u64, out: ChildStdout, child: Arc<Mutex<Child>>, subscriber: u64) {
    spawn_io_source(subscriber, "brood-proc-stdout", move |sink| {
        let mut rd = out;
        let mut buf = [0u8; 65536];
        loop {
            match rd.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => sink.emit(data_msg("proc", id, &buf[..n])),
                Err(_) => break,
            }
        }
        // stdout is at EOF: reap the child for its exit status.
        let code = loop {
            let status = {
                let mut c = child.lock().expect("subprocess child mutex");
                c.try_wait()
            };
            match status {
                Ok(Some(st)) => break st.code(),
                // Not exited yet (e.g. stdout closed early): nap and re-poll. The
                // lock is released between polls so `proc-close` can `kill`.
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                // wait() failed (already reaped elsewhere, etc.): give up cleanly.
                Err(_) => break None,
            }
        };
        reg().remove(&id);
        sink.emit(closed_msg(id, code));
    });
}

// ---- the primitive operations ----

/// `(proc-spawn prog args)` — spawn `prog` with `args`, piping its stdin/stdout/
/// stderr. The stdout/stderr readers deliver to `subscriber`. Returns the handle
/// id. Errors if the program can't be spawned (not found, not executable, …).
pub fn spawn(prog: &str, args: &[String], subscriber: u64) -> std::io::Result<u64> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Take the three pipe ends; piped() guarantees they are Some.
    let stdin: ChildStdin = child.stdin.take().expect("piped stdin");
    let stdout: ChildStdout = child.stdout.take().expect("piped stdout");
    let stderr: ChildStderr = child.stderr.take().expect("piped stderr");

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Arc::new(Mutex::new(child));
    reg().insert(
        id,
        Proc {
            stdin: Arc::new(Mutex::new(stdin)),
            child: child.clone(),
        },
    );
    start_stdout_reader(id, stdout, child, subscriber);
    start_pipe_reader(id, "proc-err", stderr, subscriber);
    Ok(id)
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
        // Brief lock (kill doesn't block) — the stdout reaper polls try_wait, so
        // we never contend with a blocked wait().
        let mut c = child.lock().expect("subprocess child mutex");
        let _ = c.kill();
        // `stdin` (in `removed`) drops here, sending EOF to the child too.
    }
}

fn bad_proc() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no such subprocess (already closed?)",
    )
}
