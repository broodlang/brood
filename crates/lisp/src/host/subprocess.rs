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
//! a dedicated non-worker thread (an `IoThread`) that **delivers to the owning
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

use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
#[cfg(not(unix))]
use std::time::Duration;

use crate::core::value;
use crate::host::owned::OwnedTable;
use crate::process::{
    CreditSource, DeliveryCredit, IoThread, Message, chunk_flush, chunk_payload, deliver_credited,
    reserve_io_thread,
};

/// A live child process: the write half (its stdin) plus a shared handle to the
/// `Child` itself, used to reap (`wait`) and to `kill`. The stdout/stderr read
/// halves are owned by their reader threads, not held here.
struct Proc {
    /// Queue into this child's **writer thread**, which owns its stdin.
    ///
    /// `proc-send` must not `write_all` on the calling thread: a pipe write is bounded by
    /// the OS buffer, so a child that stops draining its stdin would block that thread
    /// forever — and for a green process that thread is a scheduler worker, which cannot
    /// be preempted mid-syscall (ADR-059), so a handful of such sends would drain the
    /// pool with no timeout or `try` able to recover (KI-97).
    ///
    /// The shape is `dist`'s, which has the same problem and solves it the same way: one
    /// writer thread per link, fed by a **bounded** channel, with a full queue treated as
    /// "the peer is not draining" rather than something to buffer without limit. A single
    /// writer also keeps writes serialized and whole, which a per-call timeout could not —
    /// timing out mid-`write_all` would leave a **partial** message in the child's input
    /// stream, silently corrupting its protocol, which is worse than the hang it fixes.
    ///
    /// Dropping this sender is what closes the child's stdin: the writer thread sees the
    /// channel disconnect and drops its `ChildStdin`, sending EOF.
    writer: mpsc::SyncSender<Vec<u8>>,
    /// Shared with the waiter thread, which reaps the child once the handle is finished.
    /// `proc-close` and `proc-signal` lock it briefly to signal the group; the waiter
    /// locks it briefly to reap. Never held across a blocking call.
    child: Arc<ChildHandle>,
    /// Where the child is in its life, shared with its readers and its waiter;
    /// `proc-close` uses it to stop the readers (including one waiting on its inbound
    /// budget).
    lifecycle: Arc<Lifecycle>,
    /// Inbound decode mode (default text; mirrors `net`'s socket flag, ADR-141:
    /// outbound `proc-send` is unaffected — string leaves are always UTF-8).
    /// Binary mode delivers `[:proc …]` data as byte-faithful `bytes` values.
    /// Shared with the reader threads, which load it per chunk, so
    /// `proc-set-binary` flips an already-running child mid-stream.
    binary: Arc<AtomicBool>,
    /// The pty master fd, for a child spawned by [`spawn_pty`]; `None` for a plain
    /// piped child. Only `pty_resize` reads it — the reader and writer already hold
    /// their own `File` clones of it.
    /// A raw fd as a plain `i32` rather than `std::os::fd::RawFd`: that module does not
    /// exist on `wasm32-unknown-unknown`, and this struct is not itself unix-gated, so
    /// naming the alias here broke the playground's build. `RawFd` IS `i32` on every
    /// unix target; the unix-only code below converts at its own boundary.
    ///
    /// Its only reader (`pty_resize`) is unix-gated while this struct is not, so on wasm32
    /// and Windows nothing reads it and those targets warned. Allowed rather than cfg'd out:
    /// gating the field would fork the constructor for one dead word.
    #[cfg_attr(not(unix), allow(dead_code))]
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
    /// The child's Job Object on Windows, so `close` ends what it started too; `None`
    /// elsewhere, where its process group does that.
    tree: Option<ProcessTree>,
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
    /// Set by `close` on every platform (where `stop` is unix-only): a reader waiting on
    /// its [`OutputBudget`] gives up rather than wait for an owner that may never read.
    abandoned: AtomicBool,
    /// Each reader's inbound budget, so the exit and `close` wakes reach a reader that is
    /// blocked waiting for its owner to consume (see [`Lifecycle::wake_budgets`]).
    budgets: Mutex<Vec<Arc<OutputBudget>>>,
}

// ---- inbound backpressure ----

/// How much one reader may have delivered to its owner's mailbox and not yet had taken
/// before it stops reading, so a chatty child (`yes`) whose owner is not receiving cannot
/// grow that mailbox without limit. Past
/// this mark the reader thread waits; the child then blocks on its full pipe, which is
/// the backpressure a pipe already has. Measured in payload bytes plus
/// [`MESSAGE_COST_OVERHEAD`] per chunk — the socket reactor's marks and cost
/// (`host::net`), for the same reasons.
const OUTPUT_HIGH_WATER: usize = 1024 * 1024;

/// The reader resumes once its outstanding cost falls below this.
const OUTPUT_LOW_WATER: usize = 256 * 1024;

/// What one delivered chunk costs beyond its payload bytes (see `host::net`).
const MESSAGE_COST_OVERHEAD: usize = 256;

/// One reader's inbound budget: the cost of the chunks it delivered that the owner has
/// not yet taken. Each chunk carries a [`DeliveryCredit`] against it, returned when the
/// message leaves the mailbox by any route — a `receive` takes it, or the owner dies and
/// its queue is dropped — and a return that brings the total under
/// [`OUTPUT_LOW_WATER`] wakes the reader.
///
/// Lock order: this mutex is never held while delivering (delivery takes the owner's
/// mailbox lock, and a refund runs under that lock and takes this one).
struct OutputBudget {
    outstanding: Mutex<usize>,
    changed: Condvar,
}

impl OutputBudget {
    fn new() -> Arc<OutputBudget> {
        Arc::new(OutputBudget {
            outstanding: Mutex::new(0),
            changed: Condvar::new(),
        })
    }

    /// Charge one chunk of `bytes` payload and build the credit its message carries.
    fn charge(self: &Arc<Self>, bytes: usize) -> DeliveryCredit {
        let cost = bytes + MESSAGE_COST_OVERHEAD;
        *crate::core::sync::lock(&self.outstanding) += cost;
        DeliveryCredit::new(self.clone(), cost)
    }

    /// If the reader is at its high-water mark, block until refunds bring it under the
    /// low-water mark or `interrupted` holds (evaluated under this mutex, which every
    /// interrupter takes before notifying — see [`Lifecycle::wake_budgets`]).
    fn wait_for_room(&self, interrupted: impl Fn() -> bool) {
        let mut outstanding = crate::core::sync::lock(&self.outstanding);
        if *outstanding < OUTPUT_HIGH_WATER {
            return;
        }
        while *outstanding >= OUTPUT_LOW_WATER && !interrupted() {
            outstanding = self
                .changed
                .wait(outstanding)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn wake(&self) {
        let _held = crate::core::sync::lock(&self.outstanding);
        self.changed.notify_all();
    }
}

impl CreditSource for OutputBudget {
    fn release(&self, cost: usize) {
        let mut outstanding = crate::core::sync::lock(&self.outstanding);
        *outstanding -= cost;
        if *outstanding < OUTPUT_LOW_WATER {
            self.changed.notify_all();
        }
    }
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
            abandoned: AtomicBool::new(false),
            budgets: Mutex::new(Vec::new()),
        })
    }

    /// A new reader's budget, registered so the exit and `close` wakes reach it.
    fn new_budget(&self) -> Arc<OutputBudget> {
        let budget = OutputBudget::new();
        crate::core::sync::lock(&self.budgets).push(budget.clone());
        budget
    }

    /// Wake every reader waiting on its budget, after the caller has published the state
    /// change it is waiting for (each `wake` takes the budget's mutex, so a reader that
    /// checked before the change is already in its wait and receives the notify).
    fn wake_budgets(&self) {
        let budgets = crate::core::sync::lock(&self.budgets).clone();
        for budget in budgets {
            budget.wake();
        }
    }

    /// The child has exited: each reader drains what its pipe holds (see
    /// [`drain_buffered`]), so a reader waiting on its budget must wake to do it —
    /// `:proc-exit` waits for that drain, and must not wait on the owner reading.
    #[cfg(unix)]
    fn raise_exited(&self) {
        self.exited.raise();
        self.wake_budgets();
    }

    /// `close`: stop the readers, including one waiting on its budget.
    fn stop_readers(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        self.stop.raise();
        self.wake_budgets();
    }

    /// Whether a reader waiting on its budget should stop waiting: the handle was
    /// closed, or (until it has drained) the child exited.
    fn interrupts_budget_wait(&self, drained: bool) -> bool {
        if self.abandoned.load(Ordering::SeqCst) {
            return true;
        }
        #[cfg(unix)]
        if !drained && self.exited.raised.load(Ordering::Acquire) {
            return true;
        }
        let _ = drained;
        false
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

/// The child table, indexed by owner: the green process the child belongs to (the
/// `subscriber` its readers deliver to). When that process dies,
/// [`close_process_procs`] kills and reaps its children — a port dies with its owner —
/// in O(children it owns).
static REGISTRY: LazyLock<Mutex<OwnedTable<Proc>>> =
    LazyLock::new(|| Mutex::new(OwnedTable::default()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn reg() -> std::sync::MutexGuard<'static, OwnedTable<Proc>> {
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
///
/// Reads are flow-controlled: each chunk carries a credit against the reader's
/// [`OutputBudget`], and past [`OUTPUT_HIGH_WATER`] untaken the reader waits for its
/// owner to consume. The exit drain does not wait (it is bounded by one pipe's
/// contents), so `:proc-exit` never waits on the owner reading.
fn start_output_reader<R: OutputSource>(
    thread: IoThread,
    id: u64,
    tag: &'static str,
    source: R,
    lifecycle: Arc<Lifecycle>,
    subscriber: u64,
    binary: Arc<AtomicBool>,
) {
    let budget = lifecycle.new_budget();
    thread.run_source(subscriber, move |_sink| {
        let mut source = source;
        let mut buffer = [0u8; 65536];
        let mut carry: Vec<u8> = Vec::new();
        // Every data chunk is charged against this reader's budget (inbound
        // backpressure); `read_until_end` waits on it between reads.
        let emit = |payload: Message, bytes: usize| {
            deliver_credited(subscriber, data_msg(tag, id, payload), budget.charge(bytes));
        };
        let mut deliver = |bytes: &[u8]| {
            let binary_mode = binary.load(Ordering::Acquire);
            if let Some(payload) = chunk_payload(&mut carry, bytes, binary_mode) {
                emit(payload, bytes.len());
            }
        };
        let drained = read_until_end(&mut source, &mut buffer, &lifecycle, &budget, &mut deliver);
        if let Some(payload) = chunk_flush(&mut carry) {
            emit(payload, 0);
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
    budget: &OutputBudget,
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
                Ok(read) => {
                    deliver(&buffer[..read]);
                    // Inbound backpressure: past the high-water mark, stop reading until
                    // the owner has taken enough. The child blocks on its full pipe
                    // meanwhile. The child's exit interrupts the wait (the drain below
                    // must not depend on the owner reading — `:proc-exit` waits for it;
                    // it is bounded by what one pipe holds), and so does `close`.
                    budget.wait_for_room(|| lifecycle.interrupts_budget_wait(drained));
                }
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
    lifecycle: &Lifecycle,
    budget: &OutputBudget,
    deliver: &mut impl FnMut(&[u8]),
) -> bool {
    loop {
        if lifecycle.abandoned.load(Ordering::SeqCst) {
            return false;
        }
        match source.read(buffer) {
            Ok(0) => return false,
            Ok(read) => {
                deliver(&buffer[..read]);
                // Inbound backpressure — see the unix loop.
                budget.wait_for_room(|| lifecycle.interrupts_budget_wait(false));
            }
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
fn start_waiter(
    thread: IoThread,
    id: u64,
    child: Arc<ChildHandle>,
    lifecycle: Arc<Lifecycle>,
    subscriber: u64,
) {
    thread.run_source(subscriber, move |sink| {
        let code = wait_for_exit(&child);
        #[cfg(unix)]
        lifecycle.raise_exited();
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
/// One `:env` entry of a child's options: a value to set, or `None` to remove the variable
/// from the environment the child inherits.
pub type EnvEntry = (String, Option<String>);

/// Apply `env` on top of the inherited environment — the one reading of `:env` that
/// `os/spawn`, `os/spawn-pty` and `os/cmd` share.
pub fn apply_env(command: &mut Command, env: &[EnvEntry]) {
    for (name, value) in env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// Give the child the default disposition of SIGINT and SIGQUIT, whatever the runtime
/// inherited.
///
/// An IGNORED signal survives `fork` and `exec` (POSIX), and a process started in the
/// background by a script — `cmd &`, `nohup cmd` — runs with SIGINT and SIGQUIT ignored.
/// Everything such a runtime spawned inherited that too, so interrupting a child did
/// nothing: an editor's `C-c C-c` sent SIGINT to a running shell command and the command
/// never noticed. A non-interactive `sh` cannot undo it either — POSIX forbids it from
/// resetting a signal that was ignored on entry. Interactive shells reset both for a
/// foreground job for exactly this reason; a spawned child is the runtime's foreground job.
#[cfg(unix)]
pub fn reset_inherited_signals(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: runs in the forked child before exec, where only async-signal-safe calls are
    // allowed; `signal` is one.
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
pub fn reset_inherited_signals(_command: &mut Command) {}

/// Put `args` on `command`. With `raw` on Windows each is written into the command line
/// exactly as given, unquoted — what `cmd.exe /d /s /c "LINE"` needs, since `cmd` does not
/// read the MSVC quoting `Command::arg` applies (a `"` in the line became `\"`), KI-364.
/// Elsewhere `raw` changes nothing: a Unix child gets its arguments as an array.
pub fn add_args(command: &mut Command, args: &[String], raw: bool) {
    #[cfg(windows)]
    if raw {
        use std::os::windows::process::CommandExt;
        for argument in args {
            command.raw_arg(argument);
        }
        return;
    }
    let _ = raw;
    command.args(args);
}

/// Windows's process group: a Job Object holding a child, which every process the child
/// starts joins too, so stopping the child stops its whole tree (ADR-592). Killing the
/// child alone left a `sh -c "…; sleep 60"` grandchild running and holding the output
/// pipes. Off Windows it holds nothing — the child leads its own process group there.
pub(crate) struct ProcessTree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: a job handle is a kernel handle, usable from any thread; it is closed once, on
// drop, and `terminate` only reads it.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}

impl ProcessTree {
    /// A job holding `child` — or `None` off Windows, or when the OS refuses one (the
    /// child is then stopped alone, as before).
    pub(crate) fn of(child: &Child) -> Option<ProcessTree> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW,
            };
            // SAFETY: a null name and default security make an anonymous job, owned here
            // and closed on drop.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return None;
            }
            let tree = ProcessTree { job };
            // SAFETY: both handles are live — the job just made, the child's held by
            // `child`.
            let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) };
            (assigned != 0).then_some(tree)
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            None
        }
    }

    /// End every process in the tree.
    pub(crate) fn terminate(&self) {
        // SAFETY: the job handle is live until drop.
        #[cfg(windows)]
        let _ = unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1) };
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        // SAFETY: owned, closed once. Closing does not end the processes (no
        // KILL_ON_JOB_CLOSE): a command that finished may leave a daemon running, as on Unix.
        let _ = unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
    }
}

/// `(os/detach prog args opts)` — start `prog` and let it go: no pipes (its stdio is the
/// null device), no handle, not stopped when its caller dies, and not in the caller's
/// process group or Job Object. What opening a URL or a file in another application wants:
/// the opener may start the browser itself, which must outlive the editor and must not
/// hold a pipe the editor waits on. The child is reaped on a thread of its own.
pub fn detach(
    prog: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &[EnvEntry],
    raw: bool,
) -> std::io::Result<()> {
    let mut command = Command::new(prog);
    add_args(&mut command, args, raw);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console of ours, no Ctrl-C of ours
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    apply_env(&mut command, env);
    reset_inherited_signals(&mut command);
    let mut child = command.spawn()?;
    // Reap it when it ends, so it leaves no zombie; a refused thread only costs that.
    let _ = std::thread::Builder::new()
        .name("brood-detached-wait".into())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

pub fn spawn(
    prog: &str,
    args: &[String],
    cwd: Option<&str>,
    env: &[EnvEntry],
    raw_args: bool,
    subscriber: u64,
) -> std::io::Result<u64> {
    let mut command = Command::new(prog);
    add_args(&mut command, args, raw_args);
    command
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
    apply_env(&mut command, env);
    reset_inherited_signals(&mut command);
    // Before the spawn: once there is a child, every failure would have to clean it up.
    let lifecycle = Arc::new(Lifecycle::new(2)?);
    let ChildThreads {
        writer: writer_thread,
        readers: mut reader_threads,
        waiter: waiter_thread,
    } = ChildThreads::reserve(2)?;
    let mut child = command.spawn()?;
    // Take the three pipe ends; piped() guarantees they are Some.
    let stdin: ChildStdin = child.stdin.take().expect("piped stdin");
    let stdout: ChildStdout = child.stdout.take().expect("piped stdout");
    let stderr: ChildStderr = child.stderr.take().expect("piped stderr");

    let (id, child, binary) = register_child(
        child,
        start_stdin_writer(writer_thread, stdin),
        &lifecycle,
        subscriber,
        None,
    );
    start_output_reader(
        next_reader(&mut reader_threads),
        id,
        "proc",
        stdout,
        lifecycle.clone(),
        subscriber,
        binary.clone(),
    );
    start_output_reader(
        next_reader(&mut reader_threads),
        id,
        "proc-err",
        stderr,
        lifecycle.clone(),
        subscriber,
        binary,
    );
    start_waiter(waiter_thread, id, child, lifecycle, subscriber);
    Ok(id)
}

/// Every thread a child needs, started before the child is: its stdin writer, one reader
/// per output pipe, and its waiter. A child spawned first and then refused a thread would
/// have nobody to read its pipes or reap it, and its lifecycle would wait forever on the
/// reader that never started.
struct ChildThreads {
    writer: IoThread,
    readers: Vec<IoThread>,
    waiter: IoThread,
}

impl ChildThreads {
    fn reserve(readers: usize) -> std::io::Result<ChildThreads> {
        let refused = |error: std::io::Error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot start a thread for the child's pipes: {error}"),
            )
        };
        Ok(ChildThreads {
            writer: reserve_io_thread("proc-stdin").map_err(refused)?,
            readers: (0..readers)
                .map(|_| reserve_io_thread("brood-proc-reader").map_err(refused))
                .collect::<std::io::Result<_>>()?,
            waiter: reserve_io_thread("brood-proc-wait").map_err(refused)?,
        })
    }
}

/// Enter a just-spawned child in the registry: its handle id, the shared reap slot and
/// the inbound decode flag, for the caller's readers and waiter. Infallible on purpose —
/// it runs with a live child, where any failure would leak it.
fn register_child(
    child: std::process::Child,
    writer: mpsc::SyncSender<Vec<u8>>,
    lifecycle: &Arc<Lifecycle>,
    subscriber: u64,
    pty: Option<i32>,
) -> (u64, Arc<ChildHandle>, Arc<AtomicBool>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Arc::new(ChildHandle {
        slot: Mutex::new(ChildSlot {
            tree: ProcessTree::of(&child),
            child,
            reaped: false,
        }),
    });
    let binary = Arc::new(AtomicBool::new(false));
    reg().insert(
        id,
        subscriber,
        Proc {
            writer,
            child: child.clone(),
            lifecycle: lifecycle.clone(),
            binary: binary.clone(),
            pty,
        },
    );
    (id, child, binary)
}

fn next_reader(reader_threads: &mut Vec<IoThread>) -> IoThread {
    reader_threads
        .pop()
        .expect("a reader reserved per output pipe")
}

/// `(proc-set-binary handle on)` — switch `handle` between text mode (default)
/// and binary mode. Binary mode is byte-faithful both directions: inbound
/// `[:proc …]`/`[:proc-err …]` data is a Latin-1 byte-string (one codepoint
/// 0–255 per byte) and `proc-send` writes codepoints as raw bytes. Errors if the
/// handle is unknown (already closed). Mirrors `net::set_binary`.
pub fn set_binary(id: u64, on: bool) -> std::io::Result<()> {
    let registry = reg();
    match registry.get(&id) {
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
fn start_stdin_writer<W: Write + Send + 'static>(
    thread: IoThread,
    mut stdin: W,
) -> mpsc::SyncSender<Vec<u8>> {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE_CAP);
    thread.run(move || {
        while let Ok(buf) = rx.recv() {
            if stdin.write_all(&buf).is_err() || stdin.flush().is_err() {
                break;
            }
        }
        // Explicit for the reader: this is the EOF the child waits for.
        drop(stdin);
    });
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
        let registry = reg();
        match registry.get(&id) {
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
    let removed = reg().remove(&id);
    if let Some(entry) = removed {
        shut_down(entry);
    }
}

/// Terminate a child whose entry has just left the registry (`close`, or its owner's
/// death via `close_process_procs`).
fn shut_down(entry: Proc) {
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
            if let Some(tree) = &slot.tree {
                tree.terminate();
            }
            let _ = slot.child.kill();
        }
    }
    // Stop the readers where they are: a descendant that left the group escaped the
    // kill and may hold a pipe open forever, and the handle must still finish. This
    // also releases a reader waiting on its inbound budget for an owner that is not
    // reading (or has died).
    entry.lifecycle.stop_readers();
    // `stdin` (in `entry`) drops here, sending EOF to the child too.
    release(Some(entry));
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
                ));
            }
        };
        let registry = reg();
        let p = registry.get(&id).ok_or_else(|| {
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
    // Take the entries under the lock, shut them down outside it.
    let owned = reg().remove_owned_by(pid);
    for (_, entry) in owned {
        shut_down(entry);
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

    /// The local modes of the terminal behind pty master `fd` that say how input is being
    /// read: whether it echoes (`ECHO`) and whether it is line-buffered (`ICANON`).
    pub(super) fn input_mode(fd: RawFd) -> std::io::Result<super::PtyInputMode> {
        // SAFETY: `termios` is plain old data, so all-zero is a valid value for
        // `tcgetattr` to overwrite; `fd` is a pty master we opened and still hold.
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: as above — a valid descriptor and a valid, writable termios.
        let rc = unsafe { libc::tcgetattr(fd, &mut termios) };
        if rc == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(super::PtyInputMode {
            echo: termios.c_lflag & libc::ECHO != 0,
            canonical: termios.c_lflag & libc::ICANON != 0,
        })
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
    env: &[EnvEntry],
    subscriber: u64,
    cols: u16,
    rows: u16,
) -> std::io::Result<u64> {
    use std::os::unix::process::CommandExt;

    // Before the spawn: once there is a child, every failure would have to clean it up.
    let lifecycle = Arc::new(Lifecycle::new(1)?);
    let ChildThreads {
        writer: writer_thread,
        readers: mut reader_threads,
        waiter: waiter_thread,
    } = ChildThreads::reserve(1)?;
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
    apply_env(&mut command, env);
    reset_inherited_signals(&mut command);
    // A terminal-oriented program wants to know it has one — unless `:env` named `TERM`,
    // to set it or (with nil) to remove it.
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
    // One fd, two directions: the reader and the writer each get their own `File` over
    // a dup, so EOF on one does not close the other out from under it, while the
    // registry keeps the master itself for resizing. Dup'd BEFORE the spawn: a `dup`
    // can fail (EMFILE), and after the spawn that failure would return with a running
    // child that nothing reads, waits for or reaps.
    let read_file = std::fs::File::from(master.try_clone()?);
    let write_file = std::fs::File::from(master.try_clone()?);

    let child = command.spawn();
    // The parent keeps only the master: the child has its own dups of the slave, and
    // dropping ours is what lets the master see EOF when the child exits.
    drop(slave);
    let child = child?;
    // From here to the end nothing can fail: a child now exists, so an early return
    // would leak it.

    // Hand the master to the registry as a raw fd — deliberately, and `close` releases
    // it. Everything above this line was owned, so no earlier exit could leak it.
    let master = std::os::fd::IntoRawFd::into_raw_fd(master);
    let (id, child, binary) = register_child(
        child,
        start_stdin_writer(writer_thread, write_file),
        &lifecycle,
        subscriber,
        Some(master),
    );
    start_output_reader(
        next_reader(&mut reader_threads),
        id,
        "proc",
        read_file,
        lifecycle.clone(),
        subscriber,
        binary,
    );
    start_waiter(waiter_thread, id, child, lifecycle, subscriber);
    Ok(id)
}

/// `(pty-resize handle cols rows)` — tell a pty child its terminal changed size.
/// A no-op error for a handle that is not a pty, or is already closed.
#[cfg(unix)]
pub fn pty_resize(id: u64, cols: u16, rows: u16) -> std::io::Result<()> {
    let registry = reg();
    let Some(p) = registry.get(&id) else {
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

/// How a pty child's terminal is reading input right now: the two termios local modes a
/// host showing the child's input itself needs (ADR-570).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyInputMode {
    /// `ECHO`: the line discipline echoes what is typed.
    pub echo: bool,
    /// `ICANON`: input is read a LINE at a time, edited by the line discipline. A program
    /// with its own line editor (readline, a raw-mode REPL) turns this off
    /// along with `ECHO` and echoes for itself; a password read (`getpass`, `sudo`, ssh's
    /// `readpassphrase`) turns off `ECHO` alone — which is what tells the two apart.
    pub canonical: bool,
}

/// `(pty-mode handle)` — how pty child `id`'s terminal is reading input: echo and
/// canonical (line) mode.
///
/// A program reading a password clears `ECHO` and keeps `ICANON` for that read, then
/// restores both. That pair is the one reliable signal a host showing the child's input
/// itself has that the next line is a secret — a prompt regexp is only a guess.
///
/// `tcgetattr` on the MASTER answers with the slave's termios on Linux and macOS (a
/// master's mode ioctls go to its linked slave), so no slave descriptor is kept open for
/// this. `Ok(None)` when the handle is unknown or already closed — the child is gone, so
/// there is no terminal to ask, and a caller checking around a prompt should not have to
/// race the child's exit. An error for a piped child (misuse) or a failed `tcgetattr`.
#[cfg(unix)]
pub fn pty_input_mode(id: u64) -> std::io::Result<Option<PtyInputMode>> {
    let registry = reg();
    let Some(p) = registry.get(&id) else {
        return Ok(None);
    };
    let Some(fd) = p.pty else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pty-mode: this child was spawned with pipes, not a pty",
        ));
    };
    pty::input_mode(fd).map(Some)
}

/// Off Unix there is no pty, so nothing is known about how it reads: `None`.
#[cfg(not(unix))]
pub fn pty_input_mode(_id: u64) -> std::io::Result<Option<PtyInputMode>> {
    Ok(None)
}

#[cfg(not(unix))]
pub fn spawn_pty(
    _prog: &str,
    _args: &[String],
    _cwd: Option<&str>,
    _env: &[EnvEntry],
    _subscriber: u64,
    _cols: u16,
    _rows: u16,
) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pty-spawn: a pseudo-terminal on Windows needs ConPTY, which this runtime does not drive yet (KI-369)",
    ))
}

#[cfg(not(unix))]
pub fn pty_resize(_id: u64, _cols: u16, _rows: u16) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pty-resize: pseudo-terminals are a Unix facility",
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod inherited_signal_tests {
    use super::*;

    /// The child's ignored-signal mask, read from its own `/proc/self/status`.
    fn child_ignored_mask(reset: bool) -> u64 {
        let mut command = Command::new("grep");
        command.args(["SigIgn", "/proc/self/status"]);
        if reset {
            reset_inherited_signals(&mut command);
        }
        let output = command.output().expect("grep runs");
        let text = String::from_utf8_lossy(&output.stdout);
        let hex = text.split_whitespace().nth(1).expect("a SigIgn line");
        u64::from_str_radix(hex, 16).expect("a hex mask")
    }

    /// A runtime started with SIGINT/SIGQUIT ignored (a script's `cmd &`, `nohup`) must
    /// not pass that on: a child it spawns has to be interruptible. The control spawn,
    /// without the reset, shows the inheritance is real — the mask bits are set there.
    #[test]
    fn a_spawned_child_does_not_inherit_ignored_interrupts() {
        let interrupt = 1u64 << (libc::SIGINT - 1);
        let quit = 1u64 << (libc::SIGQUIT - 1);
        // SAFETY: this test process is its own (nextest runs each test in a process).
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        }
        let inherited = child_ignored_mask(false);
        assert_eq!(
            inherited & (interrupt | quit),
            interrupt | quit,
            "control: ignores are inherited"
        );
        let reset = child_ignored_mask(true);
        assert_eq!(
            reset & (interrupt | quit),
            0,
            "the child must get default SIGINT/SIGQUIT"
        );
    }
}
