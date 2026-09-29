//! Persistent child processes (ADR-104), built on the blocking-IO → mailbox seam
//! (ADR-059) — the same mechanism `crate::host::net` uses for sockets.
//!
//! `os/cmd` (`%os-cmd`) runs a child to completion and hands back its captured
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
//! - exit:   one `[:proc-exit handle code]` the moment the child itself exits (`code` is
//!   the integer exit status, or `nil` if it was terminated by a signal), after every
//!   byte of output the child wrote before it exited;
//! - closed: one `[:proc-closed handle code]` last of all, once the child has exited AND
//!   its output pipes are at end of file (or `proc-close` ended them). Same `code`.
//!
//! Exit and end of output are two events because they are two events: a child that
//! starts a background job (`sh -c "server & echo started"`) exits at once while the job
//! it left behind still holds its stdout. Waiting for end of file to report the exit
//! meant the owner heard nothing until that job closed the pipe — possibly never. The
//! child's exit is watched by its own **waiter thread**, which peeks at it without
//! reaping (`waitid` + `WNOWAIT`), asks the readers to deliver what the child left in
//! its pipes, reports `:proc-exit`, and reaps only when the handle is finished — so
//! the pid, which is also the process group `proc-close` and `proc-signal` address,
//! cannot be reused while anything can still name it.
//!
//! Writing is a non-blocking `proc-send` (queued for a writer thread). Closing is
//! `proc-close`: kill the child's process group if it is still running, drop its
//! stdin, and stop the readers — the waiter then reports `:proc-exit` (a killed child
//! has no code) and `:proc-closed`, so the owner learns how it ended either way.
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
//! (see `crate::host::net` and the `bytes` type).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
#[cfg(not(unix))]
use std::time::Duration;

use crate::core::value;
use crate::process::{chunk_flush, chunk_payload, spawn_io_source, Message};

/// A live child process: the write half (its stdin) plus a shared handle to the
/// `Child` itself, used to reap (`wait`) and to `kill`. The stdout/stderr read
/// halves are owned by their reader threads, not held here.
struct Proc {
    /// Queue into this child's **writer thread**, which owns its stdin.
    ///
    /// `proc-send` used to `write_all` on the calling thread. A pipe write is bounded by
    /// the OS buffer, so a child that stops draining its stdin blocked that thread
    /// forever — and for a green process that thread is a scheduler worker, which cannot
    /// be preempted mid-syscall (ADR-059), so a handful of such sends drained the
    /// ~nproc pool with no timeout or `try` able to recover. The old comment justified
    /// this as "the blocking contract `tcp-send` also has", but `tcp-send` went async in
    /// ADR-143, so nothing was left holding that contract up (KI-97 item 2).
    ///
    /// The shape is `dist`'s, which had the same problem and solved it the same way: one
    /// writer thread per link, fed by a **bounded** channel, with a full queue treated as
    /// "the peer is not draining" rather than something to buffer without limit. A single
    /// writer also keeps writes serialized and whole, which a per-call timeout could not —
    /// timing out mid-`write_all` would leave a **partial** message in the child's input
    /// stream, silently corrupting its protocol, which is worse than the hang it fixes.
    ///
    /// Dropping this sender is what closes the child's stdin: the writer thread sees the
    /// channel disconnect and drops its `ChildStdin`, sending EOF exactly as dropping the
    /// old handle did.
    writer: mpsc::SyncSender<Vec<u8>>,
    /// Shared with the waiter thread, which reaps the child once the handle is finished.
    /// `proc-close` and `proc-signal` lock it briefly to signal the group; the waiter
    /// locks it briefly to reap. Never held across a blocking call.
    child: Arc<ChildHandle>,
    /// Where the child is in its life, shared with its readers and its waiter;
    /// `proc-close` uses it to stop the readers. That stop is a unix wake signal, so on
    /// wasm32 nothing reads this; allowed rather than cfg'd out, as `pty` is below.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    lifecycle: Arc<Lifecycle>,
    /// Inbound decode mode (default text; mirrors `net`'s socket flag, ADR-141:
    /// outbound `proc-send` is unaffected — string leaves are always UTF-8).
    /// Binary mode delivers `[:proc …]` data as byte-faithful `bytes` values.
    /// Shared with the reader threads, which load it per chunk, so
    /// `proc-set-binary` flips an already-running child mid-stream.
    binary: Arc<AtomicBool>,
    /// The green process this child belongs to — the `subscriber` its readers
    /// deliver to. When that process dies, [`close_process_procs`] kills and reaps
    /// the child (Erlang port semantics: a port dies with its owner). Without
    /// this, an owner that crashed without `proc-close` orphaned the OS child
    /// forever: the registry entry leaked, and both reader threads kept draining
    /// output into a dead pid's mailbox (a no-op delivery) for the child's life.
    owner: u64,
    /// The pty master fd, for a child spawned by [`spawn_pty`]; `None` for a plain
    /// piped child. Only `pty_resize` reads it — the reader and writer already hold
    /// their own `File` clones of it.
    /// A raw fd as a plain `i32` rather than `std::os::fd::RawFd`: that module does not
    /// exist on `wasm32-unknown-unknown`, and this struct is not itself unix-gated, so
    /// naming the alias here broke the playground's build. `RawFd` IS `i32` on every
    /// unix target; the unix-only code below converts at its own boundary.
    ///
    /// Its only reader (`pty_resize`) is unix-gated while this struct is not, so on wasm32
    /// nothing reads it and that target warned. Allowed rather than cfg'd out: gating the
    /// field would fork the constructor for one dead word.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pty: Option<i32>,
}

/// The `Child`, and whether the waiter has reaped it.
///
/// The waiter does not block in `Child::wait` (that needs `&mut Child`, so it would hold
/// this mutex for the child's whole life and `proc-close` could never take it to kill).
/// It blocks in `waitid` with `WNOWAIT`, which needs no lock and leaves the child a
/// zombie, and takes this mutex only for the instant of the final reap.
struct ChildHandle {
    slot: Mutex<ChildSlot>,
}

struct ChildSlot {
    child: Child,
    /// Set by the waiter under this mutex when it reaps. Until then the child's pid —
    /// which is also its process group id — is held by the zombie and cannot be reused,
    /// so `close` and `signal` may `killpg` it; after, they must not, and they check this
    /// under the same lock.
    reaped: bool,
}

/// Where a child is in its life: shared by its reader threads, its waiter thread and
/// `close`. One mutex and one condvar, never held across a blocking call.
struct Lifecycle {
    state: Mutex<LifecycleState>,
    changed: Condvar,
    /// Raised by the waiter when the child has exited: each reader then delivers what
    /// the pipe held at that moment and reports it drained.
    #[cfg(unix)]
    exited: WakeSignal,
    /// Raised by `close`: the readers stop where they are, so a handle that was closed
    /// finishes even while a descendant that escaped the kill still holds a pipe.
    #[cfg(unix)]
    stop: WakeSignal,
}

struct LifecycleState {
    /// Readers that have neither reached end of file nor delivered what their pipe held
    /// when the child exited. `:proc-exit` waits for this to reach zero.
    undrained: usize,
    /// Readers still running. `:proc-closed` waits for this to reach zero.
    open: usize,
}

impl Lifecycle {
    fn new(readers: usize) -> std::io::Result<Self> {
        Ok(Lifecycle {
            state: Mutex::new(LifecycleState {
                undrained: readers,
                open: readers,
            }),
            changed: Condvar::new(),
            #[cfg(unix)]
            exited: WakeSignal::new()?,
            #[cfg(unix)]
            stop: WakeSignal::new()?,
        })
    }

    /// A reader has delivered everything its pipe held when the child exited.
    #[cfg(unix)]
    fn reader_drained(&self) {
        let mut state = crate::core::sync::lock(&self.state);
        state.undrained -= 1;
        self.changed.notify_all();
    }

    /// A reader has stopped (end of file, a read error, or `close`). `drained` says
    /// whether it already reported [`Lifecycle::reader_drained`]; one that ended first
    /// has nothing left to deliver, which counts as drained.
    fn reader_finished(&self, drained: bool) {
        let mut state = crate::core::sync::lock(&self.state);
        if !drained {
            state.undrained -= 1;
        }
        state.open -= 1;
        self.changed.notify_all();
    }

    /// Block until `done` holds of the state. Poison-tolerant like every lock in the
    /// runtime (`core/sync.rs`): a panic elsewhere must not strand the waiter, which is
    /// what reports the child's end.
    fn wait_until(&self, done: impl Fn(&LifecycleState) -> bool) {
        let mut state = crate::core::sync::lock(&self.state);
        while !done(&state) {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// A one-shot, level-triggered broadcast a reader can `poll` beside its pipe: raising it
/// makes one byte readable on `watched`, and since nobody reads that byte it stays
/// readable, so every reader sees it however late it looks. A socket pair because std
/// creates one close-on-exec atomically, so no child spawned meanwhile inherits it.
#[cfg(unix)]
struct WakeSignal {
    raiser: std::os::unix::net::UnixStream,
    watched: std::os::unix::net::UnixStream,
    raised: AtomicBool,
}

#[cfg(unix)]
impl WakeSignal {
    fn new() -> std::io::Result<Self> {
        let (raiser, watched) = std::os::unix::net::UnixStream::pair()?;
        Ok(WakeSignal {
            raiser,
            watched,
            raised: AtomicBool::new(false),
        })
    }

    fn raise(&self) {
        if !self.raised.swap(true, Ordering::AcqRel) {
            // One byte into an empty socket buffer cannot block. A failure would mean the
            // pair is gone, which only happens once nobody is watching it.
            let _ = (&self.raiser).write_all(&[1]);
        }
    }

    fn watched_fd(&self) -> i32 {
        std::os::fd::AsRawFd::as_raw_fd(&self.watched)
    }
}

/// What a reader reads: a pipe end, or a pty master. On unix it must also be `poll`able.
#[cfg(unix)]
trait OutputSource: Read + std::os::fd::AsRawFd + Send + 'static {}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd + Send + 'static> OutputSource for T {}
#[cfg(not(unix))]
trait OutputSource: Read + Send + 'static {}
#[cfg(not(unix))]
impl<T: Read + Send + 'static> OutputSource for T {}

/// Off unix there is no `waitid`, so the waiter polls `try_wait`: first interval, and the
/// cap it backs off to.
#[cfg(not(unix))]
const REAP_POLL_MIN: Duration = Duration::from_millis(5);
#[cfg(not(unix))]
const REAP_POLL_MAX: Duration = Duration::from_millis(500);

/// Depth of a child's pending-write queue. Deep enough that an ordinary burst never
/// notices, shallow enough that a child which has stopped reading is reported rather
/// than buffered without bound. `dist` uses the same shape for the same reason.
const WRITE_QUEUE_CAP: usize = 1024;

static REGISTRY: LazyLock<Mutex<HashMap<u64, Proc>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, HashMap<u64, Proc>> {
    crate::core::sync::lock(&REGISTRY)
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

/// Build a `[:proc-exit handle code]` or `[:proc-closed handle code]` message. `code` is
/// the integer exit status, or `nil` when the child was terminated by a signal.
fn end_msg(tag: &str, id: u64, code: Option<i32>) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern(tag)),
        Message::Subprocess(id),
        code.map(|c| Message::Int(c as i64)).unwrap_or(Message::Nil),
    ])
}

// ---- reader threads ----

/// Read `source` on a non-worker thread, emitting one `[<tag> id data]` message per
/// chunk to `subscriber` — `:proc` for stdout (and a pty), `:proc-err` for stderr —
/// until end of file or `close`.
///
/// On unix the reader `poll`s its pipe beside the lifecycle's two wake signals. When the
/// child exits it delivers what the pipe held at that moment ([`drain_buffered`]) and
/// reports it drained, which is what lets the waiter put `:proc-exit` after the child's
/// own output; then it carries on reading whatever descendants still write.
fn start_output_reader<R: OutputSource>(
    id: u64,
    tag: &'static str,
    source: R,
    lifecycle: Arc<Lifecycle>,
    subscriber: u64,
    binary: Arc<AtomicBool>,
) {
    spawn_io_source(subscriber, "brood-proc-reader", move |sink| {
        let mut source = source;
        let mut buffer = [0u8; 65536];
        let mut carry: Vec<u8> = Vec::new();
        let mut deliver = |bytes: &[u8]| {
            let binary_mode = binary.load(Ordering::Acquire);
            if let Some(payload) = chunk_payload(&mut carry, bytes, binary_mode) {
                sink.emit(data_msg(tag, id, payload));
            }
        };
        let drained = read_until_end(&mut source, &mut buffer, &lifecycle, &mut deliver);
        if let Some(payload) = chunk_flush(&mut carry) {
            sink.emit(data_msg(tag, id, payload));
        }
        lifecycle.reader_finished(drained);
    });
}

/// The unix read loop. Returns whether it reported [`Lifecycle::reader_drained`].
#[cfg(unix)]
fn read_until_end<R: OutputSource>(
    source: &mut R,
    buffer: &mut [u8],
    lifecycle: &Lifecycle,
    deliver: &mut impl FnMut(&[u8]),
) -> bool {
    let data_fd = source.as_raw_fd();
    let mut drained = false;
    loop {
        let watch = |fd: i32| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let mut watched = [
            watch(data_fd),
            watch(lifecycle.stop.watched_fd()),
            watch(lifecycle.exited.watched_fd()),
        ];
        // Once drained, stop watching the exit signal: it stays raised, and would spin.
        let count = if drained { 2 } else { 3 };
        // SAFETY: `watched` is a live array of `count` initialised pollfds.
        let ready = unsafe { libc::poll(watched.as_mut_ptr(), count as libc::nfds_t, -1) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return drained;
        }
        if watched[1].revents != 0 {
            return drained;
        }
        if !drained && watched[2].revents != 0 {
            if drain_buffered(source, buffer, deliver) {
                return drained; // end of file on the way: nothing more will come
            }
            lifecycle.reader_drained();
            drained = true;
            continue;
        }
        if watched[0].revents != 0 {
            match source.read(buffer) {
                Ok(0) => return drained,
                Ok(read) => deliver(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                // A pty master reports its slave's last close as EIO: end of file.
                Err(_) => return drained,
            }
        }
    }
}

/// Deliver what the pipe holds right now — measured once, with `FIONREAD`, so a
/// descendant writing without pause cannot keep this going. Every byte the child wrote
/// before it exited is either delivered already or counted in that measure: a `write` to
/// a pipe returns only once its bytes are in the pipe's buffer. (A pty is weaker: the
/// line discipline hands bytes to the master asynchronously, so a child's last output
/// can still be in flight when it exits and arrive after `:proc-exit` — never after
/// `:proc-closed`.) Reads only while `poll` says the pipe is readable, so it never
/// blocks; setting `O_NONBLOCK` instead would reach the pty's writer through the shared
/// file description. Returns whether it reached end of file.
#[cfg(unix)]
fn drain_buffered<R: OutputSource>(
    source: &mut R,
    buffer: &mut [u8],
    deliver: &mut impl FnMut(&[u8]),
) -> bool {
    let data_fd = source.as_raw_fd();
    let mut pending: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int through the pointer, which is live.
    if unsafe { libc::ioctl(data_fd, libc::FIONREAD, &mut pending) } < 0 {
        pending = 0;
    }
    let mut remaining = pending.max(0) as usize;
    while remaining > 0 {
        let mut probe = libc::pollfd {
            fd: data_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one live pollfd, zero timeout.
        if unsafe { libc::poll(&mut probe, 1, 0) } <= 0 {
            return false;
        }
        match source.read(buffer) {
            Ok(0) => return true,
            Ok(read) => {
                deliver(&buffer[..read]);
                remaining = remaining.saturating_sub(read);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return true,
        }
    }
    false
}

/// Off unix: read to end of file. Nothing to drain at exit, so it never reports drained
/// before it finishes, and `:proc-exit` waits for end of file as it always did there.
#[cfg(not(unix))]
fn read_until_end<R: OutputSource>(
    source: &mut R,
    buffer: &mut [u8],
    _lifecycle: &Lifecycle,
    deliver: &mut impl FnMut(&[u8]),
) -> bool {
    loop {
        match source.read(buffer) {
            Ok(0) => return false,
            Ok(read) => deliver(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
}

// ---- the waiter thread ----

/// The child's one waiter, and the only thread that reports its end or reaps it.
///
/// 1. Wait for the child to exit, without reaping it ([`wait_for_exit`]).
/// 2. Raise [`Lifecycle::exited`] and wait until every reader has delivered what its pipe
///    held, then emit `[:proc-exit id code]` — after the child's own output.
/// 3. Wait until every reader has finished (end of file, or `close` stopped them), drop
///    the registry entry, reap, and emit `[:proc-closed id code]`, the handle's last
///    message.
///
/// Reaping last keeps the zombie — and with it the pid and process group id — reserved
/// for as long as `close`/`signal` can reach the child through the registry, so a
/// descendant still holding the pipes can be killed through the group without any risk
/// of the id having been recycled.
fn start_waiter(id: u64, child: Arc<ChildHandle>, lifecycle: Arc<Lifecycle>, subscriber: u64) {
    spawn_io_source(subscriber, "brood-proc-wait", move |sink| {
        let code = wait_for_exit(&child);
        #[cfg(unix)]
        lifecycle.exited.raise();
        lifecycle.wait_until(|state| state.undrained == 0);
        sink.emit(end_msg("proc-exit", id, code));
        lifecycle.wait_until(|state| state.open == 0);
        // The entry leaves the registry here for a child that finished on its own, so this
        // is the removal site that has to give its fds back — see `release`.
        let removed = reg().remove(&id);
        release(removed);
        {
            let mut slot = crate::core::sync::lock(&child.slot);
            // A zombie by now, so this returns at once.
            let _ = slot.child.wait();
            slot.reaped = true;
        }
        sink.emit(end_msg("proc-closed", id, code));
    });
}

/// Block until the child exits and return its exit code (`None` if a signal ended it),
/// leaving it unreaped. `waitid` with `WNOWAIT` needs no lock on the `Child`, so `close`
/// can kill it meanwhile, and the kill ends this wait at once.
#[cfg(unix)]
fn wait_for_exit(child: &ChildHandle) -> Option<i32> {
    let pid = crate::core::sync::lock(&child.slot).child.id();
    loop {
        // SAFETY: zeroed is a valid siginfo_t; waitid fills it in.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `pid` is our own unreaped child (only this thread reaps it, later), and
        // `info` is live for the call.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            if info.si_code != libc::CLD_EXITED {
                return None; // killed or dumped by a signal: no exit status
            }
            #[cfg(any(target_os = "linux", target_os = "android"))]
            // SAFETY: for a CLD_* code the union holds the child fields.
            let status = unsafe { info.si_status() };
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let status = info.si_status;
            return Some(status);
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
    }
}

/// Off unix there is no `WNOWAIT`: poll `try_wait`, which reaps.
#[cfg(not(unix))]
fn wait_for_exit(child: &ChildHandle) -> Option<i32> {
    let mut backoff = REAP_POLL_MIN;
    loop {
        {
            let mut slot = crate::core::sync::lock(&child.slot);
            match slot.child.try_wait() {
                Ok(Some(status)) => {
                    slot.reaped = true;
                    return status.code();
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(REAP_POLL_MAX);
    }
}

// ---- the primitive operations ----

/// `(os/spawn prog args opts)` — spawn `prog` with `args`, piping its stdin/
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
    // Its own process group, so `close` can end the program AND everything it started:
    // `sh -c "make"` killed alone left `make` and its compilers running, holding the output
    // pipes. (A pty child already leads its own session — `spawn_pty` — and the same
    // `killpg` covers it.)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    // Before the spawn: once there is a child, every failure would have to clean it up.
    let lifecycle = Arc::new(Lifecycle::new(2)?);
    let mut child = command.spawn()?;
    // Take the three pipe ends; piped() guarantees they are Some.
    let stdin: ChildStdin = child.stdin.take().expect("piped stdin");
    let stdout: ChildStdout = child.stdout.take().expect("piped stdout");
    let stderr: ChildStderr = child.stderr.take().expect("piped stderr");

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Arc::new(ChildHandle {
        slot: Mutex::new(ChildSlot {
            child,
            reaped: false,
        }),
    });
    let binary = Arc::new(AtomicBool::new(false));
    let writer = start_stdin_writer(stdin);
    reg().insert(
        id,
        Proc {
            writer,
            child: child.clone(),
            lifecycle: lifecycle.clone(),
            binary: binary.clone(),
            owner: subscriber,
            pty: None,
        },
    );
    start_output_reader(
        id,
        "proc",
        stdout,
        lifecycle.clone(),
        subscriber,
        binary.clone(),
    );
    start_output_reader(
        id,
        "proc-err",
        stderr,
        lifecycle.clone(),
        subscriber,
        binary,
    );
    start_waiter(id, child, lifecycle, subscriber);
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

/// Own a child's stdin on a dedicated thread and write whatever arrives on the channel.
///
/// This is the thread that is *allowed* to block: it is not a scheduler worker, so a child
/// that stops draining its stdin costs one parked thread instead of a slice of the pool
/// (ADR-059). Each queued buffer is written whole and in order, so a message is never
/// split — the property a per-call write timeout could not have preserved.
///
/// The loop ends when every sender is dropped (the registry entry removed by `proc-close`
/// or owner death), and dropping `stdin` on the way out is what gives the child EOF.
/// A write error also ends it: the child is gone, and its death is already reported to the
/// owner as `[:proc-exit …]` by the waiter, which is the one report worth having.
fn start_stdin_writer<W: Write + Send + 'static>(mut stdin: W) -> mpsc::SyncSender<Vec<u8>> {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE_CAP);
    let spawned = std::thread::Builder::new()
        .name("proc-stdin".into())
        .spawn(move || {
            while let Ok(buf) = rx.recv() {
                if stdin.write_all(&buf).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
            // Explicit for the reader: this is the EOF the child waits for.
            drop(stdin);
        });
    if spawned.is_err() {
        // Out of threads. The receiver drops here, so every later `send` reports a
        // disconnected writer rather than silently succeeding into a channel nobody
        // drains — and stdin closes, which the child sees as EOF.
        eprintln!("subprocess: cannot spawn stdin writer thread; this child accepts no input");
    }
    tx
}

/// `(proc-send handle data)` — queue `data` for the child's stdin.
///
/// **Non-blocking**, deliberately: the bytes are handed to this child's writer thread
/// rather than written on the caller's, because the caller is usually a scheduler worker
/// and a child that stops reading would otherwise pin it forever (KI-97 item 2; see
/// [`Proc::writer`]). Writes still land whole and in order.
///
/// Errors if the handle is unknown, or if the queue is full — which means this child has
/// stopped draining its stdin, a condition worth reporting rather than burying under an
/// unbounded buffer. A *write* failure is not reported here (it happens later, on the
/// writer thread); the child's death arrives as `[:proc-exit …]`, which is the signal
/// that actually tells the owner what happened.
pub fn send(id: u64, data: &[u8]) -> std::io::Result<()> {
    // Clone the sender out under a brief registry lock, then queue outside it, so a
    // full queue never stalls every other `proc-*` op.
    let writer = {
        let reg = reg();
        match reg.get(&id) {
            Some(p) => p.writer.clone(),
            None => return Err(bad_proc()),
        }
    };
    writer.try_send(data.to_vec()).map_err(|e| match e {
        mpsc::TrySendError::Full(_) => std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "proc-send: child {id} is not draining its stdin ({WRITE_QUEUE_CAP} writes queued)"
            ),
        ),
        mpsc::TrySendError::Disconnected(_) => bad_proc(),
    })
}

/// `(proc-close handle)` — terminate the child: kill its process group if it is still
/// running, drop its stdin (EOF), and stop its readers. Idempotent. The waiter then
/// emits `[:proc-exit …]` (if it had not yet) and the final `[:proc-closed …]`; this
/// call does not wait for either.
pub fn close(id: u64) {
    let removed = {
        let mut reg = reg();
        reg.remove(&id)
    };
    if let Some(entry) = &removed {
        {
            // Brief: signalling does not block, and the waiter holds this lock only to reap.
            let mut slot = crate::core::sync::lock(&entry.child.slot);
            // The waiter may already have found the entry gone and reaped; after that the
            // pid is free for reuse and must not be signalled. Before it, the zombie (or
            // the live child) holds the pid, so it names this child's group alone.
            if !slot.reaped {
                // The whole group — the child leads it (`spawn` / `spawn_pty`), and a
                // grandchild left behind keeps running and keeps the pipes open.
                // SAFETY: an unreaped child's pid cannot have been reused.
                #[cfg(unix)]
                unsafe {
                    libc::killpg(slot.child.id() as libc::pid_t, libc::SIGKILL);
                }
                let _ = slot.child.kill();
            }
        }
        // Stop the readers where they are: a descendant that left the group escaped the
        // kill and may hold a pipe open forever, and the handle must still finish.
        #[cfg(unix)]
        entry.lifecycle.stop.raise();
    }
    // `stdin` (in `removed`) drops here, sending EOF to the child too.
    release(removed);
}

/// `(proc-signal handle sig)` — deliver signal `sig` (`"int"`, `"term"`, `"hup"`,
/// `"quit"`, `"kill"`) to the child's whole process group, leaving it registered: the
/// child decides what the signal means, and if it exits the waiter emits
/// `[:proc-exit …]` as for any exit. The group, not the pid, for the reason `close`
/// uses it: `sh -c "make"` interrupted at `sh` alone leaves `make` running. Errors if
/// the handle is unknown (closed, or already reaped) or the name is not one of these.
pub fn signal(id: u64, sig: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let number = match sig {
            "int" => libc::SIGINT,
            "term" => libc::SIGTERM,
            "hup" => libc::SIGHUP,
            "quit" => libc::SIGQUIT,
            "kill" => libc::SIGKILL,
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown signal :{other} (one of :int :term :hup :quit :kill)"),
                ))
            }
        };
        let reg = reg();
        let p = reg.get(&id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "unknown or closed subprocess")
        })?;
        let slot = crate::core::sync::lock(&p.child.slot);
        // SAFETY: the entry is still registered (we hold the registry lock), so the waiter
        // has not reaped the child — it removes the entry first — and the pid, the group id
        // since `spawn`/`spawn_pty` make the child a group leader, cannot have been reused.
        // That holds after the child has exited too: until the handle is finished it is a
        // zombie, so a signal still reaches the descendants it left in its group.
        if unsafe { libc::killpg(slot.child.id() as libc::pid_t, number) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (id, sig);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "signals need a Unix host",
        ))
    }
}

/// Release what a removed registry entry owned beyond its Rust values — today, a pty
/// child's master fd, which is a bare `RawFd` with no `Drop`.
///
/// Called from BOTH places an entry can leave the registry: `close`, and the waiter when
/// a child exits by itself. That second one is the common case and the one that leaked —
/// `close` cleaning up was not enough, because for a child that has already exited the
/// waiter had removed the entry first and `close` then found nothing.
fn release(entry: Option<Proc>) {
    #[cfg(unix)]
    if let Some(Proc { pty: Some(fd), .. }) = entry {
        // SAFETY: dup'd for this entry, which has just been removed, so there is no
        // other owner and it cannot be closed twice.
        unsafe { libc::close(fd) };
    }
    #[cfg(not(unix))]
    let _ = entry;
}

/// Close every subprocess owned by `pid` — the process-death hook, called from the
/// scheduler's retirement path beside `net::close_process_sockets` (the OS-process
/// model: a dead process's resources are reclaimed on exit). A process that
/// `proc-close`d its children has none left here, so this is a no-op then.
pub fn close_process_procs(pid: u64) {
    // Collect under the lock, close outside it: `close` re-takes the registry lock.
    let ids: Vec<u64> = reg()
        .iter()
        .filter(|(_, p)| p.owner == pid)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        close(id);
    }
}

fn bad_proc() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "no such subprocess (already closed?)",
    )
}

// ---- pseudo-terminals ------------------------------------------------------
//
// A pipe is the wrong shape for a program that expects a *terminal*. `iex -S mix`,
// `python`, `psql`, a shell — each asks `isatty(0)` and, told no, drops its prompt,
// its line editing and usually its echo, then line-buffers its output so nothing
// arrives until it exits. That is why an editor cannot host a REPL over `proc-spawn`:
// the child is not misbehaving, it is correctly declining to be interactive.
//
// A pty is the missing primitive, and it is the same seam as everything else here —
// so a pty child reuses the whole `Proc` registry, `proc-send`, `proc-close`, the
// owner-death cleanup and the message protocol. Two things differ, both inherent:
//
//   * ONE fd carries both directions, so there is no separate stderr; the child's
//     stderr is its terminal, and everything arrives as `[:proc handle data]`. A
//     caller that needs the streams apart wants pipes, not a terminal.
//   * a terminal has a SIZE, and programs lay out against it (and are told when it
//     changes), so `pty_resize` exists and is expected to be called.
//
// The child gets its own session and a controlling terminal (`setsid` + `TIOCSCTTY`
// in `pre_exec`), which is what makes job control and ^C work rather than delivering
// signals to the editor.

#[cfg(unix)]
mod pty {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    /// Set a pty's window size. Programs read this at start and on `SIGWINCH`.
    pub(super) fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> std::io::Result<()> {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `fd` is a pty master we opened; `ws` is the struct TIOCSWINSZ expects.
        let rc = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Open a pty pair, returning `(master, slave)` as OWNED descriptors.
    ///
    /// Owned rather than raw so that every `?` on the way through [`super::spawn_pty`]
    /// closes them. There are several such exits before the registry takes the master,
    /// and a raw fd leaked on each one.
    ///
    /// `posix_openpt` + `grantpt` + `unlockpt` + `ptsname` rather than `openpty`,
    /// because that quartet is plain POSIX in `libc` while `openpty` lives in
    /// `libutil` and needs an extra link on some platforms.
    pub(super) fn open_pair() -> std::io::Result<(OwnedFd, OwnedFd)> {
        // SAFETY: each call is the documented POSIX sequence, and every return is
        // checked before the next step uses it.
        unsafe {
            // `O_CLOEXEC` on both ends is load-bearing: a child that inherits the MASTER holds
            // its own terminal open, so closing ours never hangs it up — an `sh -i` spawned
            // under the pty outlived the editor that closed it, still reading a terminal
            // nobody else could reach. The child gets its stdio through `stdio_dup`.
            let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
            if master < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
                let e = std::io::Error::last_os_error();
                libc::close(master);
                return Err(e);
            }
            // `ptsname_r` is the thread-safe form and is Linux-only; macOS has only
            // `ptsname`, whose static buffer is safe here because the name is copied
            // out before this function returns and no other thread calls it.
            #[cfg(target_os = "linux")]
            let slave = {
                let mut name = [0 as libc::c_char; 256];
                if libc::ptsname_r(master, name.as_mut_ptr(), name.len()) != 0 {
                    let e = std::io::Error::last_os_error();
                    libc::close(master);
                    return Err(e);
                }
                libc::open(
                    name.as_ptr(),
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
                )
            };
            #[cfg(not(target_os = "linux"))]
            let slave = {
                let name = libc::ptsname(master);
                if name.is_null() {
                    let e = std::io::Error::last_os_error();
                    libc::close(master);
                    return Err(e);
                }
                libc::open(name, libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC)
            };
            if slave < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(master);
                return Err(e);
            }
            // SAFETY: both are freshly opened descriptors with no other owner.
            Ok((OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)))
        }
    }

    /// A `Stdio` on a fresh dup of `fd`, so each of the child's three descriptors owns
    /// its own file and closing one does not close the others.
    pub(super) fn stdio_dup(fd: &OwnedFd) -> std::io::Result<std::process::Stdio> {
        let fd = fd.as_raw_fd();
        // SAFETY: `fd` is open; `dup` returns a new owned descriptor, which `File`
        // takes ownership of and `Stdio` then consumes.
        let dup = unsafe { libc::dup(fd) };
        if dup < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(std::process::Stdio::from(unsafe {
            std::fs::File::from_raw_fd(dup)
        }))
    }
}

/// `(pty-spawn prog args opts)` — spawn `prog` under a **pseudo-terminal**, so it
/// behaves as it would in a terminal: prompts, line editing, unbuffered output.
///
/// Same handle type and same messages as [`spawn`], except that there is no
/// `[:proc-err …]` stream — a terminal has one channel, and the child's stderr goes
/// to it. `cols`/`rows` set the initial window size; [`pty_resize`] changes it.
#[cfg(unix)]
pub fn spawn_pty(
    prog: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &[(String, String)],
    subscriber: u64,
    cols: u16,
    rows: u16,
) -> std::io::Result<u64> {
    use std::os::unix::process::CommandExt;

    // Before the spawn: once there is a child, every failure would have to clean it up.
    let lifecycle = Arc::new(Lifecycle::new(1)?);
    let (master, slave) = pty::open_pair()?;
    pty::set_winsize(std::os::fd::AsRawFd::as_raw_fd(&master), cols, rows)?;

    let mut command = Command::new(prog);
    command
        .args(args)
        .stdin(pty::stdio_dup(&slave)?)
        .stdout(pty::stdio_dup(&slave)?)
        .stderr(pty::stdio_dup(&slave)?);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    // A terminal-oriented program wants to know it has one.
    if !env.iter().any(|(k, _)| k == "TERM") && std::env::var_os("TERM").is_none() {
        command.env("TERM", "xterm-256color");
    }
    // SAFETY: runs in the forked child before exec. `setsid` detaches it from our
    // session and `TIOCSCTTY` makes the pty its controlling terminal — which is what
    // sends ^C and SIGWINCH to the CHILD rather than to the editor hosting it. Both
    // are async-signal-safe syscalls, the only thing permitted between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            // `.into()` is load-bearing, not decoration: `ioctl`'s request parameter and
            // `TIOCSCTTY` are typed independently per platform and DISAGREE on Apple —
            // libc derives `TIOCSWINSZ` from `_IOW` (c_ulong, matching) but `TIOCSCTTY`
            // from `_IO` through `ulong_cast_uint` (c_uint), so a bare constant is
            // `expected u64, found u32` and only the macOS release job ever sees it.
            // Linux types both as its `Ioctl` alias — c_ulong on gnu, c_int on musl —
            // so `as u64` would trade the macOS break for a musl one; the identity
            // conversion is correct on all of them and cannot truncate.
            // The allow is the other half of the same platform story, not a silencing:
            // clippy only ever runs here on linux-gnu, where `Ioctl` is `c_ulong` and the
            // conversion genuinely is the identity, so `useless_conversion` fires and CI's
            // `-D warnings` turns it into a hard error — which also skips every step behind
            // the clippy job. It is right about this target and wrong about the other four.
            #[allow(clippy::useless_conversion)]
            let request = libc::TIOCSCTTY.into();
            if libc::ioctl(0, request, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn();
    // The parent keeps only the master: the child has its own dups of the slave, and
    // dropping ours is what lets the master see EOF when the child exits.
    drop(slave);
    let child = child?;

    // One fd, two directions: the reader and the writer each get their own `File` over
    // a dup, so EOF on one does not close the other out from under it, while the
    // registry keeps the master itself for resizing.
    let read_file = std::fs::File::from(master.try_clone()?);
    let write_file = std::fs::File::from(master.try_clone()?);
    // Hand the master to the registry as a raw fd — deliberately, and `close` releases
    // it. Everything above this line was owned, so no earlier exit could leak it.
    let master = std::os::fd::IntoRawFd::into_raw_fd(master);

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Arc::new(ChildHandle {
        slot: Mutex::new(ChildSlot {
            child,
            reaped: false,
        }),
    });
    let binary = Arc::new(AtomicBool::new(false));
    let writer = start_stdin_writer(write_file);
    reg().insert(
        id,
        Proc {
            writer,
            child: child.clone(),
            lifecycle: lifecycle.clone(),
            binary: binary.clone(),
            owner: subscriber,
            pty: Some(master),
        },
    );
    start_output_reader(id, "proc", read_file, lifecycle.clone(), subscriber, binary);
    start_waiter(id, child, lifecycle, subscriber);
    Ok(id)
}

/// `(pty-resize handle cols rows)` — tell a pty child its terminal changed size.
/// A no-op error for a handle that is not a pty, or is already closed.
#[cfg(unix)]
pub fn pty_resize(id: u64, cols: u16, rows: u16) -> std::io::Result<()> {
    let reg = reg();
    let Some(p) = reg.get(&id) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pty-resize: unknown or closed handle",
        ));
    };
    let Some(fd) = p.pty else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pty-resize: this child was spawned with pipes, not a pty",
        ));
    };
    pty::set_winsize(fd, cols, rows)
}

#[cfg(not(unix))]
pub fn spawn_pty(
    _prog: &str,
    _args: &[String],
    _cwd: Option<&str>,
    _env: &[(String, String)],
    _subscriber: u64,
    _cols: u16,
    _rows: u16,
) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pty-spawn: pseudo-terminals are a Unix facility",
    ))
}

#[cfg(not(unix))]
pub fn pty_resize(_id: u64, _cols: u16, _rows: u16) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pty-resize: pseudo-terminals are a Unix facility",
    ))
}
