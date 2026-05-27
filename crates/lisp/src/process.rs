//! Processes: share-nothing green-ish processes communicating by message
//! passing (`spawn`/`send`/`receive`/`self`).
//!
//! This is **step 4a** of the concurrency plan (see `docs/concurrency.md`): each
//! process is backed by its own OS thread with its own [`Heap`] — real
//! parallelism, real isolation. Turning these into lightweight green threads on
//! a small worker pool (M:N, work-stealing, a 2-core cap) is step 4b and needs
//! coroutine suspension; the `spawn`/`send`/`receive` *surface* won't change.
//!
//! **Code is shared, data is not.** Inner processes spawned from a runtime share
//! that runtime's code + global bindings (a shared `Arc<RuntimeCode>`), so a
//! `def` reaches them and `spawn` just hands over the (shared) function handle.
//! But a process's *data* lives in its own local heap, so a [`Value`] that
//! points there can't be read by another process: message **data** crosses as a
//! self-contained, `Send` [`Message`] (a deep copy), rebuilt into the receiver's
//! local heap. Symbols travel as their global interned id (the interner is
//! process-wide), so they stay consistent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Condvar, LazyLock, Mutex};

use crate::error::LispError;
use crate::eval;
use crate::heap::Heap;
use crate::value::{EnvId, Symbol, Value};

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

// ----- the process table -----

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
/// How many processes `spawn` has started since the program began. Each spawned
/// process is backed by one OS thread (step 4a), so this is also the count of
/// worker threads created. The runner reads it via `(spawn-count)` to report how
/// much concurrency a test run used.
static SPAWNED: AtomicU64 = AtomicU64::new(0);
static REGISTRY: LazyLock<Mutex<HashMap<u64, Sender<Message>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Total processes spawned (= worker OS threads created) since program start.
pub fn spawn_count() -> u64 {
    SPAWNED.load(Ordering::SeqCst)
}

/// A throttle on how many spawned threads may run *at once*. This is a stopgap
/// until step 4b (green M:N processes on a worker pool): it caps concurrency but
/// does not make processes cheap — each `spawn` is still its own OS thread, born
/// when a permit is free and gone when it finishes. Because `receive` blocks its
/// thread, the cap must exceed the depth of processes simultaneously blocked
/// waiting on a not-yet-running process, or the run deadlocks (so a suite whose
/// tests each spawn-and-wait on one child needs at least `cap = 2`).
struct Gate {
    cap: usize,  // max concurrent spawned threads; 0 = unlimited
    live: usize, // spawned threads currently running
    peak: usize, // high-water mark of `live`
}
static GATE: LazyLock<(Mutex<Gate>, Condvar)> =
    LazyLock::new(|| (Mutex::new(Gate { cap: 0, live: 0, peak: 0 }), Condvar::new()));

/// Cap the number of spawned threads allowed to run concurrently (0 = unlimited).
/// Set once at startup (e.g. from the CLI's `-j` flag) before any spawning.
pub fn set_max_parallel(n: usize) {
    let (lock, _) = &*GATE;
    lock.lock().unwrap().cap = n;
}

/// High-water mark of concurrently-running spawned threads — how much parallelism
/// was actually reached. (The runner/main thread is not counted.)
pub fn peak_threads() -> u64 {
    let (lock, _) = &*GATE;
    lock.lock().unwrap().peak as u64
}

/// Block until a permit is free, then take it (call before starting a thread).
fn gate_acquire() {
    let (lock, cv) = &*GATE;
    let mut g = lock.lock().unwrap();
    while g.cap != 0 && g.live >= g.cap {
        g = cv.wait(g).unwrap();
    }
    g.live += 1;
    if g.live > g.peak {
        g.peak = g.live;
    }
}

/// Return a permit (call when a spawned thread finishes) and wake a waiter.
fn gate_release() {
    let (lock, cv) = &*GATE;
    let mut g = lock.lock().unwrap();
    g.live -= 1;
    cv.notify_one();
}

struct ProcCtx {
    pid: u64,
    inbox: Receiver<Message>,
}

thread_local! {
    static CURRENT: RefCell<Option<ProcCtx>> = const { RefCell::new(None) };
}

/// Ensure the current thread has a process context (pid + mailbox). The first
/// time `self`/`receive`/`send` is used on a thread (e.g. the REPL), this
/// registers it as a process so it can participate in message passing.
fn ensure_ctx() -> u64 {
    CURRENT.with(|cell| {
        if let Some(ctx) = cell.borrow().as_ref() {
            return ctx.pid;
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        REGISTRY.lock().unwrap().insert(pid, tx);
        *cell.borrow_mut() = Some(ProcCtx { pid, inbox: rx });
        pid
    })
}

fn deregister(pid: u64) {
    REGISTRY.lock().unwrap().remove(&pid);
}

/// `(spawn f arg...)` — run `f` (a function) in a new process with copied args.
/// Returns the new pid.
pub fn spawn(heap: &Heap, f: Value, args: &[Value]) -> Result<u64, LispError> {
    // Promote the target into the shared RUNTIME region so its handle is valid
    // in the child, which shares this runtime's code via the Arc below. A
    // top-level function is already shared, so this is usually a no-op.
    let f = heap.promote(f);
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err("spawn: first argument must be a function"));
    }
    // Args are *data*, not code: ship them as messages, rebuilt into the child's
    // own local heap, so the two processes share no mutable data.
    let mut arg_msgs = Vec::with_capacity(args.len());
    for &a in args {
        arg_msgs.push(to_message(heap, a)?);
    }
    // The child shares this runtime's code + global table (the same Arc), so a
    // `def` here is visible to it on its next lookup; the prelude is shared
    // read-only. This is what makes a long-running spawned process pick up a
    // redefinition without being restarted (see docs/shared-code.md).
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    // Register the mailbox in the *parent*, before the thread starts, so a
    // `send` immediately after `spawn` can't race ahead of registration.
    let (tx, rx) = mpsc::channel();
    REGISTRY.lock().unwrap().insert(pid, tx);

    // Block here if we're at the concurrency cap, so an over-eager `spawn` can't
    // create more OS threads than `-j` allows. The permit is returned when the
    // thread finishes.
    gate_acquire();
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        // This thread *is* the process: adopt the pid + mailbox, share the
        // runtime's code, and start with a fresh local data heap.
        CURRENT.with(|cell| *cell.borrow_mut() = Some(ProcCtx { pid, inbox: rx }));
        let mut heap = Heap::with_regions(prelude, runtime);
        heap.set_global(EnvId::GLOBAL);
        let mut argv = Vec::with_capacity(arg_msgs.len());
        for m in &arg_msgs {
            argv.push(from_message(&mut heap, m));
        }
        if let Err(e) = eval::apply(&mut heap, f, &argv, EnvId::GLOBAL) {
            eprintln!("process {} died: {}", pid, e);
        }
        deregister(pid);
        gate_release();
    });

    Ok(pid)
}

/// `(send pid msg)` — copy `msg` into `pid`'s mailbox. Sending to a dead pid is
/// a silent no-op (Erlang semantics).
pub fn send(heap: &Heap, pid_val: Value, msg_val: Value) -> Result<(), LispError> {
    let pid = match pid_val {
        Value::Int(n) if n >= 0 => n as u64,
        _ => return Err(LispError::type_err("send: first argument must be a pid (integer)")),
    };
    let msg = to_message(heap, msg_val)?;
    let tx = REGISTRY.lock().unwrap().get(&pid).cloned();
    if let Some(tx) = tx {
        let _ = tx.send(msg);
    }
    Ok(())
}

/// `(receive)` — take the next message from this process's mailbox, blocking
/// until one arrives.
pub fn receive(heap: &mut Heap) -> Result<Value, LispError> {
    ensure_ctx();
    let received = CURRENT.with(|cell| cell.borrow().as_ref().unwrap().inbox.recv());
    match received {
        Ok(m) => Ok(from_message(heap, &m)),
        Err(_) => Err(LispError::runtime("receive: mailbox closed")),
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx()
}
