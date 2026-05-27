//! Processes: share-nothing green processes communicating by message passing
//! (`spawn`/`send`/`receive`/`self`).
//!
//! **Step 4b** (see `docs/scheduler.md`, ADR-018): processes are lightweight
//! *green* threads, not OS threads. Each runs inside a [`corosensei`] stackful
//! coroutine — its own parkable stack — so the native recursive evaluator runs
//! unchanged and `receive` on an empty mailbox **suspends** the coroutine instead
//! of blocking a thread. A small pool of worker OS threads (≈ `nproc`, a setting)
//! runs ready processes off a shared run queue; `send` wakes a parked process.
//!
//! **Code is shared, data is not.** A spawned process shares the runtime's code +
//! global table (the `Arc`s in its `Heap`), so a `def` reaches it; but its data
//! lives in its own LOCAL heap, so messages cross as a self-contained, `Send`
//! [`Message`] (a deep copy), rebuilt into the receiver's heap. Symbols travel as
//! their global interned id (the interner is process-wide).
//!
//! The thread that started the program (the REPL / file runner) is a *root*
//! process: it is not a coroutine, so its `receive` **blocks** on its mailbox
//! rather than yielding. Everything `spawn`ed is a green process that yields.

use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};
use std::time::{Duration, Instant};

use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;

/// A `Send`, self-contained copy of a value, for crossing heaps.
pub enum Message {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Sym(Symbol),
    Keyword(Symbol),
    List(Vec<Message>),
    Vector(Vec<Message>),
    Map(Vec<(Message, Message)>),
}

/// Deep-copy a value out of `heap` into a `Send` message. Functions can't be
/// sent (they're per-heap closures).
pub fn to_message(heap: &Heap, v: Value) -> Result<Message, LispError> {
    Ok(match v {
        Value::Nil => Message::Nil,
        Value::Bool(b) => Message::Bool(b),
        Value::Int(n) => Message::Int(n),
        Value::Float(f) => Message::Float(f),
        Value::Sym(s) => Message::Sym(s),
        Value::Keyword(s) => Message::Keyword(s),
        Value::Str(id) => Message::Str(heap.string(id).to_string()),
        Value::Pair(_) => {
            let items = heap.list_to_vec(v)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message(heap, item)?);
            }
            Message::List(out)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_message(heap, item)?);
            }
            Message::Vector(out)
        }
        Value::Map(id) => {
            let entries = heap.map(id).to_vec();
            let mut out = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                out.push((to_message(heap, k)?, to_message(heap, v)?));
            }
            Message::Map(out)
        }
        Value::Fn(_) | Value::Macro(_) | Value::Native(_) => {
            return Err(LispError::type_err("cannot send a function in a message"))
        }
    })
}

/// Rebuild a message into `heap`.
pub fn from_message(heap: &mut Heap, m: &Message) -> Value {
    match m {
        Message::Nil => Value::Nil,
        Message::Bool(b) => Value::Bool(*b),
        Message::Int(n) => Value::Int(*n),
        Message::Float(f) => Value::Float(*f),
        Message::Sym(s) => Value::Sym(*s),
        Message::Keyword(s) => Value::Keyword(*s),
        Message::Str(s) => heap.alloc_string(s),
        Message::List(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            heap.list(vals)
        }
        Message::Vector(items) => {
            let mut vals = Vec::with_capacity(items.len());
            for item in items {
                vals.push(from_message(heap, item));
            }
            heap.alloc_vector(vals)
        }
        Message::Map(entries) => {
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = from_message(heap, k);
                let v = from_message(heap, v);
                pairs.push((k, v));
            }
            heap.map_from_pairs(pairs)
        }
    }
}

// ----- mailboxes & processes -------------------------------------------------

/// Why a green process's coroutine yielded control back to its worker.
enum Suspend {
    /// Blocked in `receive` on an empty mailbox — park until a message (or timer).
    Receive,
    /// Preempted by the reduction counter — still runnable, re-queue immediately.
    Preempt,
}

type Yielder0 = Yielder<(), Suspend>;
type Coro = Coroutine<(), Suspend, ()>;

/// A process's mailbox. Guarded by one mutex so the "check empty → park" and
/// "deliver → wake" handshakes stay race-free (see `receive`/`send`/`run_one`).
struct Mailbox {
    state: Mutex<MailboxState>,
    /// Wakes a *root* process blocked in `receive` (greens are woken by being
    /// re-queued instead).
    cv: Condvar,
}

struct MailboxState {
    queue: VecDeque<Message>,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.)
    waiter: Option<Box<Process>>,
    /// How many leading messages the parked waiter already scanned and rejected
    /// (selective receive). The worker re-runs it only when a message arrives
    /// *beyond* this — not for ones it already skipped. 0 for a plain FIFO receive.
    scanned: usize,
}

impl Mailbox {
    fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                waiter: None,
                scanned: 0,
            }),
            cv: Condvar::new(),
        })
    }
}

/// A green process: its mailbox plus the coroutine carrying its computation
/// (which owns its `Heap`). `Send`, so any worker can run it.
struct Process {
    pid: u64,
    mailbox: Arc<Mailbox>,
    coro: Coro,
}

// SAFETY: corosensei marks `Coroutine` `!Send` conservatively. We move a process
// between worker threads only via the run queue, which owns it exclusively — it is
// never resumed on two threads at once, and corosensei supports resuming a
// coroutine on a different thread than the one it suspended on. Its captured state
// (heap, Arcs, message values) is all `Send`. So migrating a *parked* process is
// sound. (See docs/scheduler.md; swappable if we drop corosensei.)
unsafe impl Send for Process {}

/// What a running coroutine needs to find from deep inside `eval` (for
/// `receive`/`self`). Stored in a thread-local, set by the coroutine at start and
/// re-established after every suspend (so it survives the worker multiplexing
/// other processes, and migration to another worker).
#[derive(Clone)]
struct Ctx {
    pid: u64,
    mailbox: Arc<Mailbox>,
    /// `Some` for a green process (suspend via this yielder); `None` for the root
    /// thread (block on the mailbox condvar instead).
    yielder: Option<*const Yielder0>,
}

thread_local! {
    static CURRENT: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

// ----- reduction-counted preemption ------------------------------------------

thread_local! {
    /// Reductions left in the current process's scheduling quantum. The worker
    /// resets it to `REDUCTION_BUDGET` before each `resume` (see `run_one`); `eval`
    /// decrements it via `tick`, and the process yields when it hits zero.
    static REDUCTIONS: Cell<u32> = const { Cell::new(0) };
}

/// How many `eval` loop iterations a process runs before it must yield its worker
/// (cooperative fairness — the BEAM's mechanism). ~2000 ≈ the BEAM default; tunable.
const REDUCTION_BUDGET: u32 = 2000;

/// Called once per `eval` `'tail:` iteration. Cheap: a thread-local decrement; only
/// when the budget is exhausted does it touch `CURRENT` and (for a green process)
/// suspend. Bounds the work any one process does before peers get the worker, so a
/// CPU-bound process can't monopolise a core. The root thread is never preempted
/// (it has no yielder) — it just refreshes the budget.
#[inline]
pub fn tick() {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            preempt();
        } else {
            r.set(n - 1);
        }
    });
}

/// Budget exhausted: refresh it, then — if we're a green process — yield the worker
/// (re-queued Ready by `run_one`). Re-establishes `CURRENT` after resume, since the
/// worker may have run other processes meanwhile (cf. `receive`).
fn preempt() {
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    let ctx = match CURRENT.with(|c| c.borrow().clone()) {
        Some(c) => c,
        None => return, // no process context (e.g. prelude build) — nothing to yield to
    };
    if let Some(yptr) = ctx.yielder {
        // SAFETY: same invariant as `receive` — the yielder is valid while this
        // coroutine is running, which is now (tick runs inside eval, inside the
        // coroutine body). Suspending returns control to the worker (`run_one`).
        unsafe { (*yptr).suspend(Suspend::Preempt) };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
    }
    // Root thread (yielder None): budget refreshed, never suspends.
}

// ----- the run queue + worker pool -------------------------------------------

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicUsize = AtomicUsize::new(0); // processes inside `resume` right now
static PEAK_RUNNING: AtomicUsize = AtomicUsize::new(0);
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0); // 0 = default (≈ nproc)
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0); // worker threads actually started
static WORKERS_STARTED: Once = Once::new();

/// pid → mailbox, for `send` to find a target from any thread.
static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<Mailbox>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The shared run queue of ready processes + a condvar workers wait on.
static RUN: LazyLock<(Mutex<VecDeque<Box<Process>>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(VecDeque::new()), Condvar::new()));

/// Total processes spawned since program start (read by `(spawn-count)`).
pub fn spawn_count() -> u64 {
    SPAWNED.load(Ordering::SeqCst)
}

/// Set the worker-pool size (0 = default ≈ `nproc`). Call once at startup, before
/// any spawning. (Replaces the old per-spawn thread cap.)
pub fn set_max_parallel(n: usize) {
    WORKER_COUNT.store(n, Ordering::SeqCst);
}

/// High-water mark of processes running simultaneously (≤ pool size).
pub fn peak_threads() -> u64 {
    PEAK_RUNNING.load(Ordering::SeqCst) as u64
}

/// Worker OS threads in the scheduler pool (0 until the first `spawn` starts them).
pub fn worker_threads() -> u64 {
    ACTIVE_WORKERS.load(Ordering::SeqCst) as u64
}

fn worker_count() -> usize {
    match WORKER_COUNT.load(Ordering::SeqCst) {
        0 => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        n => n,
    }
}

fn deregister(pid: u64) {
    REGISTRY.lock().unwrap().remove(&pid);
}

/// Push a ready process onto the run queue and wake a worker.
fn enqueue(proc: Box<Process>) {
    let (lock, cv) = &*RUN;
    lock.lock().unwrap().push_back(proc);
    cv.notify_one();
}

/// Start the worker pool exactly once (on the first `spawn`).
fn ensure_workers() {
    WORKERS_STARTED.call_once(|| {
        let n = worker_count();
        ACTIVE_WORKERS.store(n, Ordering::SeqCst);
        for _ in 0..n {
            std::thread::spawn(worker_loop);
        }
    });
}

fn worker_loop() {
    loop {
        let proc = {
            let (lock, cv) = &*RUN;
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(p) = q.pop_front() {
                    break p;
                }
                q = cv.wait(q).unwrap();
            }
        };
        run_one(proc);
    }
}

/// Resume a process once, then either retire it (it finished) or, if it suspended
/// at `receive`, park it on its mailbox (or re-queue it if a message raced in).
fn run_one(mut proc: Box<Process>) {
    let mailbox = Arc::clone(&proc.mailbox);

    let live = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    PEAK_RUNNING.fetch_max(live, Ordering::SeqCst);
    // Fresh reduction budget for this scheduling quantum (decremented in eval's loop
    // via `tick`; at zero the process preempts itself — see `tick`/`preempt`).
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.coro.resume(())));
    RUNNING.fetch_sub(1, Ordering::SeqCst);

    match outcome {
        Ok(CoroutineResult::Return(())) => deregister(proc.pid),
        Ok(CoroutineResult::Yield(Suspend::Receive)) => {
            // The coroutine suspended in `receive`: it scanned the first `scanned`
            // messages with no match. Re-check under the lock — if a *new*
            // (unscanned) message arrived during the suspend window, run again;
            // otherwise park here for `send` (or the timer) to wake.
            let mut st = mailbox.state.lock().unwrap();
            if st.queue.len() > st.scanned {
                drop(st);
                enqueue(proc);
            } else {
                st.waiter = Some(proc);
            }
        }
        Ok(CoroutineResult::Yield(Suspend::Preempt)) => {
            // Preempted mid-computation (reduction budget hit). Still runnable —
            // re-queue at the back so peers get a turn on this worker (fairness).
            enqueue(proc);
        }
        Err(_) => {
            eprintln!("process {} panicked", proc.pid);
            deregister(proc.pid);
        }
    }
}

// ----- spawn / send / receive / self ----------------------------------------

/// `(spawn f arg...)` — run `f` (a function) as a new green process with copied
/// args. Returns the new pid.
pub fn spawn(heap: &Heap, f: Value, args: &[Value]) -> Result<u64, LispError> {
    // Promote the target into the shared RUNTIME region so its handle is valid in
    // the child (which shares this runtime's code via the Arcs below). A top-level
    // function is already shared, so this is usually a no-op.
    let f = heap.promote(f);
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err(
            "spawn: first argument must be a function",
        ));
    }
    // Args are *data*: ship them as messages, rebuilt into the child's own heap.
    let mut arg_msgs = Vec::with_capacity(args.len());
    for &a in args {
        arg_msgs.push(to_message(heap, a)?);
    }
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    let mailbox = Mailbox::new();
    REGISTRY.lock().unwrap().insert(pid, Arc::clone(&mailbox));

    let coro_mailbox = Arc::clone(&mailbox);
    let coro = Coroutine::new(move |yielder: &Yielder0, _input: ()| {
        // Establish this process's context so `receive`/`self` can find it.
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(Ctx {
                pid,
                mailbox: Arc::clone(&coro_mailbox),
                yielder: Some(yielder as *const Yielder0),
            });
        });
        let mut heap = Heap::with_regions(prelude, runtime);
        heap.set_global(EnvId::GLOBAL);
        let mut argv = Vec::with_capacity(arg_msgs.len());
        for m in &arg_msgs {
            argv.push(from_message(&mut heap, m));
        }
        if let Err(e) = eval::apply(&mut heap, f, &argv, EnvId::GLOBAL) {
            eprintln!("process {} died: {}", pid, e);
        }
        CURRENT.with(|c| *c.borrow_mut() = None);
    });

    ensure_workers();
    enqueue(Box::new(Process { pid, mailbox, coro }));
    Ok(pid)
}

/// `(send pid msg)` — copy `msg` into `pid`'s mailbox and wake it. Sending to a
/// dead pid is a silent no-op (Erlang semantics).
pub fn send(heap: &Heap, pid_val: Value, msg_val: Value) -> Result<(), LispError> {
    let pid = match pid_val {
        Value::Int(n) if n >= 0 => n as u64,
        _ => {
            return Err(LispError::type_err(
                "send: first argument must be a pid (integer)",
            ))
        }
    };
    let msg = to_message(heap, msg_val)?;
    let mailbox = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = mb.state.lock().unwrap();
        st.queue.push_back(msg);
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc); // wake a parked green process
        } else {
            mb.cv.notify_one(); // wake the root thread, if it's blocked in receive
        }
    }
    Ok(())
}

/// `(%receive matcher timeout on-timeout)` — selective receive. `matcher` is a unary
/// function: given a message value it returns a 0-arg thunk (the clause body, closing
/// over its bindings) on a match, or `nil` on no match. Scan the mailbox in order;
/// the first message a clause matches is removed and its thunk run. Non-matching
/// messages stay queued (Erlang selective receive). `timeout` is `nil` (wait forever)
/// or an integer of milliseconds; on expiry the `on-timeout` thunk runs (a `throw`
/// inside it propagates through `try`/`catch`, since both apply via this `?`). A
/// green process suspends while waiting; the root thread blocks.
pub fn receive_match(
    heap: &mut Heap,
    matcher: Value,
    timeout: Value,
    on_timeout: Value,
) -> LispResult {
    let deadline = match timeout {
        Value::Nil => None,
        Value::Int(ms) if ms >= 0 => Some(Instant::now() + Duration::from_millis(ms as u64)),
        _ => {
            return Err(LispError::type_err(
                "receive: timeout must be an integer (milliseconds) or nil",
            ))
        }
    };
    let ctx = ensure_ctx();
    let mut i = 0usize;
    loop {
        // Rebuild candidate `i` into the heap, then run the matcher *without* holding
        // the mailbox lock (the matcher calls eval). Only this process removes from
        // its own mailbox, so the scanned prefix is stable; `send` only appends.
        let candidate = {
            let st = ctx.mailbox.state.lock().unwrap();
            if i < st.queue.len() {
                Some(from_message(heap, &st.queue[i]))
            } else {
                None
            }
        };
        match candidate {
            Some(v) => {
                let thunk = eval::apply(heap, matcher, &[v], EnvId::GLOBAL)?;
                if matches!(thunk, Value::Fn(_)) {
                    // Matched — remove exactly this message, then run its body.
                    ctx.mailbox.state.lock().unwrap().queue.remove(i);
                    return eval::apply(heap, thunk, &[], EnvId::GLOBAL);
                }
                i += 1; // no clause matched — leave it queued, try the next message
            }
            None => {
                // Scanned every queued message with no match.
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        return if matches!(on_timeout, Value::Fn(_)) {
                            eval::apply(heap, on_timeout, &[], EnvId::GLOBAL)
                        } else {
                            Ok(Value::Nil)
                        };
                    }
                }
                wait_for_message(&ctx, i, deadline);
            }
        }
    }
}

/// Wait until a message beyond index `i` might be available, honouring `deadline`.
/// Green: record the scan position and suspend (arming a timer if there's a deadline,
/// so it wakes to check). Root: block on the mailbox condvar (with timeout). Returns
/// when the caller should re-scan from `i`.
fn wait_for_message(ctx: &Ctx, i: usize, deadline: Option<Instant>) {
    match ctx.yielder {
        // Root thread: block on the condvar (with timeout) until a send or deadline.
        None => {
            let st = ctx.mailbox.state.lock().unwrap();
            if st.queue.len() > i {
                return; // a message arrived between the scan and here — re-scan
            }
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now < d {
                        // Re-acquired guard dropped at end of scope (before we return).
                        let _g = ctx.mailbox.cv.wait_timeout(st, d - now);
                    }
                }
                None => {
                    let _g = ctx.mailbox.cv.wait(st);
                }
            }
        }
        // Green process: record how far we scanned (so the worker re-runs us only on
        // a *new* message — see `run_one`), then suspend. A timer wakes us at the
        // deadline; `send` wakes us on a new message.
        Some(yptr) => {
            {
                let mut st = ctx.mailbox.state.lock().unwrap();
                if st.queue.len() > i {
                    return; // raced — a message arrived; re-scan without suspending
                }
                st.scanned = i;
            }
            if let Some(d) = deadline {
                arm_timer(ctx.pid, d);
            }
            // SAFETY: the yielder is valid while this coroutine runs — which is now
            // (called from within eval, within the coroutine body). Suspending
            // returns control to the worker (`run_one`), which parks us.
            unsafe { (*yptr).suspend(Suspend::Receive) };
            // Resumed (by send or timer): the worker may have run others or migrated
            // us to another worker — re-establish the context.
            CURRENT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        }
    }
}

// ----- timers (receive deadlines) --------------------------------------------

/// Min-heap of `(deadline, pid)`: `Reverse` turns the max-heap into earliest-first.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
static TIMER_STARTED: Once = Once::new();

/// Arrange to wake green process `pid` at `deadline`. Lazily starts the timer thread
/// on first use (programs that never use a `receive` timeout never spawn it).
fn arm_timer(pid: u64, deadline: Instant) {
    TIMER_STARTED.call_once(|| {
        std::thread::spawn(timer_loop);
    });
    let (lock, cv) = &*TIMERS;
    lock.lock().unwrap().push(Reverse((deadline, pid)));
    cv.notify_one();
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
fn timer_loop() {
    let (lock, cv) = &*TIMERS;
    let mut q = lock.lock().unwrap();
    loop {
        match q.peek().copied() {
            None => q = cv.wait(q).unwrap(),
            Some(Reverse((deadline, _))) => {
                let now = Instant::now();
                if now < deadline {
                    q = cv.wait_timeout(q, deadline - now).unwrap().0;
                } else {
                    let mut due = Vec::new();
                    while let Some(&Reverse((d, pid))) = q.peek() {
                        if d <= now {
                            q.pop();
                            due.push(pid);
                        } else {
                            break;
                        }
                    }
                    drop(q);
                    for pid in due {
                        wake_for_timeout(pid);
                    }
                    q = lock.lock().unwrap();
                }
            }
        }
    }
}

/// Re-queue green process `pid` if it's still parked, so it wakes, re-scans, and —
/// finding its deadline passed — runs its `after` clause. A no-op if `send` already
/// woke it or it re-parked on another receive; the process always re-validates its
/// own deadline, so a stale timer is harmless (at most one spurious wakeup).
fn wake_for_timeout(pid: u64) {
    let mailbox = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = mb.state.lock().unwrap();
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc);
        }
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx().pid
}

/// The current process's context. A coroutine sets this itself; the first time a
/// *root* thread (the REPL / file runner) uses `self`/`receive`, register it as a
/// blocking-mailbox process so it can participate in message passing.
fn ensure_ctx() -> Ctx {
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            return ctx.clone();
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let mailbox = Mailbox::new();
        REGISTRY.lock().unwrap().insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx {
            pid,
            mailbox,
            yielder: None,
        };
        *c.borrow_mut() = Some(ctx.clone());
        ctx
    })
}
