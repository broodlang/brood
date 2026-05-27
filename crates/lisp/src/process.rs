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

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};

use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::error::{LispError, LispResult};
use crate::eval;
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Symbol, Value};

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
    }
}

// ----- mailboxes & processes -------------------------------------------------

type Yielder0 = Yielder<(), ()>;
type Coro = Coroutine<(), (), ()>;

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
}

impl Mailbox {
    fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                waiter: None,
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
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.coro.resume(())));
    RUNNING.fetch_sub(1, Ordering::SeqCst);

    match outcome {
        Ok(CoroutineResult::Return(())) => deregister(proc.pid),
        Ok(CoroutineResult::Yield(())) => {
            // The coroutine suspended in `receive` because its mailbox looked
            // empty. Re-check under the lock: if a message arrived during the
            // suspend window, run again; otherwise park here for `send` to wake.
            let mut st = mailbox.state.lock().unwrap();
            if st.queue.is_empty() {
                st.waiter = Some(proc);
            } else {
                drop(st);
                enqueue(proc);
            }
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

/// `(receive)` — take the next message from this process's mailbox. A green
/// process **suspends** until one arrives; the root thread **blocks**.
pub fn receive(heap: &mut Heap) -> LispResult {
    let ctx = ensure_ctx();
    match ctx.yielder {
        // Root thread: block on the mailbox condvar.
        None => {
            let mut st = ctx.mailbox.state.lock().unwrap();
            loop {
                if let Some(msg) = st.queue.pop_front() {
                    drop(st);
                    return Ok(from_message(heap, &msg));
                }
                st = ctx.mailbox.cv.wait(st).unwrap();
            }
        }
        // Green process: suspend the coroutine until re-queued by `send`.
        Some(yptr) => loop {
            {
                let mut st = ctx.mailbox.state.lock().unwrap();
                if let Some(msg) = st.queue.pop_front() {
                    drop(st);
                    return Ok(from_message(heap, &msg));
                }
            }
            // SAFETY: the yielder lives on this coroutine's stack and is valid for
            // as long as the coroutine is running — which is exactly now, since
            // `receive` is called from within it. Suspending returns control to
            // the worker (see `run_one`), which parks us.
            unsafe { (*yptr).suspend(()) };
            // Resumed: the worker may have run other processes (overwriting the
            // thread-local) or resumed us on a different worker — re-establish.
            CURRENT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
        },
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
