//! Green-process scheduler: the coroutine machinery, the shared run queue,
//! the worker pool, and the public `spawn` / `self` / `pid-value` /
//! `spawn-count` / `peak-threads` / `set-max-parallel` surface.
//!
//! Each green process is a [`corosensei`] stackful coroutine carrying its
//! own parkable stack — `receive` on an empty mailbox suspends the
//! coroutine instead of blocking a thread, so a small pool of worker OS
//! threads (≈ `nproc`) multiplexes many processes. The root thread (REPL /
//! file runner) is **not** a coroutine: it blocks on its mailbox condvar
//! instead of yielding (see [`super::mailbox::wait_for_message`]).
//!
//! ## Thread-locals
//! - [`CURRENT`] — the running process's [`Ctx`] (`pid`, `mailbox`,
//!   `yielder`). Set by the coroutine at start and re-established after
//!   every suspend, so `(self)` / `receive` can find their process even
//!   after the worker has run others or migrated us.
//! - [`REDUCTIONS`] — countdown to the next preempt; [`tick`] decrements
//!   it from inside `eval`'s loop.
//! - [`GC_BLOCK`] — eval/macroexpand nesting depth, consulted by the GC
//!   safepoint in `eval::eval`. Saved/restored around every suspend so
//!   workers multiplexing several processes don't leak each other's
//!   depths.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::LispError;
use crate::eval;

use super::mailbox::{Mailbox, REGISTRY};
use super::message::Message;
use super::monitor;

/// Why a green process's coroutine yielded control back to its worker.
pub(super) enum Suspend {
    /// Blocked in `receive` on an empty mailbox — park until a message (or timer).
    Receive,
    /// Preempted by the reduction counter — still runnable, re-queue immediately.
    Preempt,
}

pub(super) type Yielder0 = Yielder<(), Suspend>;
type Coro = Coroutine<(), Suspend, ()>;

/// A green process: its mailbox plus the coroutine carrying its computation
/// (which owns its `Heap`). `Send`, so any worker can run it.
pub(super) struct Process {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
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
pub(super) struct Ctx {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
    /// `Some` for a green process (suspend via this yielder); `None` for the root
    /// thread (block on the mailbox condvar instead).
    pub(super) yielder: Option<*const Yielder0>,
}

thread_local! {
    pub(super) static CURRENT: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

// ----- reduction-counted preemption ------------------------------------------

thread_local! {
    /// Reductions left in the current process's scheduling quantum. The worker
    /// resets it to `REDUCTION_BUDGET` before each `resume` (see `run_one`); `eval`
    /// decrements it via `tick`, and the process yields when it hits zero.
    static REDUCTIONS: Cell<u32> = const { Cell::new(0) };

    /// GC-block depth: how many `eval` / `macroexpand_all` frames are active on
    /// this thread. The eval safepoint runs GC iff this is **1** ("we are the
    /// outermost contributor — no other eval/macroexpand frame holds an
    /// unrooted LOCAL transient"). See `docs/memory-model.md` and the
    /// rooting-completeness argument in `eval::eval`.
    ///
    /// Per-process: reset to 0 at coroutine entry and saved/restored around
    /// suspend (see `spawn` / `preempt` / `wait_for_message`), so workers
    /// multiplexing several processes don't leak each other's depths. The root
    /// thread doesn't multiplex, so its depth flows naturally.
    static GC_BLOCK: Cell<u32> = const { Cell::new(0) };
}

/// Current GC-block depth — `eval::eval`'s safepoint compares this against 1.
#[inline]
pub fn gc_block_depth() -> u32 {
    GC_BLOCK.with(|d| d.get())
}

/// Read the GC-block depth for save/restore around a coroutine suspend (we want
/// to capture this process's value so a resume on any worker restores it).
#[inline]
pub(super) fn gc_block_save() -> u32 {
    GC_BLOCK.with(|d| d.get())
}

/// Write the GC-block depth — paired with `gc_block_save` around a suspend,
/// and used by a fresh coroutine to wipe the residual value left on the worker.
#[inline]
pub(super) fn gc_block_set(n: u32) {
    GC_BLOCK.with(|d| d.set(n));
}

// ----- resume slot (ADR-039 step 2 — supervised processes) ------------------
//
// The runtime's per-process supervisor catches a `LispError` that escaped the
// process's eval (a redefinition that throws, a bug in a worker iteration)
// and re-invokes the *last function call* — same callee, same args — so a
// long-running stateful loop survives a bad iteration. The "last function
// call" is captured here, in a thread-local, by the eval loop at every
// tail-call dispatch (see `eval::Value::Fn` branch).
//
// **Coarse-grained checkpointing.** We update only at the tail-call site,
// not at every nested call. Recovery semantics: retry the most recent
// iteration of the enclosing loop, not the innermost frame. This is what
// users actually want — and the per-call cost stays at one update per
// iteration, not one per frame.
//
// **Why a thread-local, not Process-owned**: the slot is hot-path; thread-
// local access is one TLS read. The slot is per-process-currently-running:
// when a worker multiplexes another process, the old process's slot would
// otherwise leak into the new one. Save/restore around suspends (the same
// pattern `GC_BLOCK` uses) keeps each process's slot isolated.

/// What the supervisor needs to re-invoke the most recent iteration. The
/// eval loop pushes `(callee, name, argv.clone())` here on every tail-call
/// entry (`Value::Fn(id)` branch, just before `continue 'tail`). The
/// supervisor in the coroutine body reads it via [`take_resume`] when an
/// error escapes.
///
/// `name` is the closure's `defn`-given name (when set). On retry, the
/// supervisor re-resolves it in the global env so a user's hot reload
/// (`(def my-loop …)` between iterations) takes effect on the retry —
/// without this, the supervisor would re-invoke the *old, throwing*
/// closure handle stored in `callee`, defeating the whole point of
/// integrating supervision with hot reload (ADR-039 × ADR-013). Falls
/// back to the stored handle if the name doesn't resolve to a `Fn`
/// anymore (rare — only if the user `def`'d it to something
/// non-callable mid-flight).
#[derive(Clone)]
pub(crate) struct ResumeSlot {
    pub(crate) callee: crate::core::value::Value,
    pub(crate) name: Option<crate::core::value::Symbol>,
    pub(crate) argv: smallvec::SmallVec<[crate::core::value::Value; 8]>,
}

thread_local! {
    /// The currently-running green process's resume slot. Updated by
    /// `record_resume` from the eval loop on every tail-call dispatch; read
    /// + cleared by the supervisor via `take_resume` on error. Saved /
    /// restored around suspends so a worker running A, then B, then A again
    /// doesn't leak B's slot into A's recovery.
    static RESUME_SLOT: RefCell<Option<ResumeSlot>> = const { RefCell::new(None) };
}

/// Eval-loop hook: record `(callee, name, argv)` as the current iteration's
/// resume point. Called by `eval::eval`'s `Value::Fn(id)` branch right
/// before `continue 'tail`. Reuses the slot's `SmallVec` capacity when
/// possible (clear + extend_from_slice) so a tight tail-loop pays one
/// `Value` overwrite + one memcpy per iteration, not a fresh allocation.
///
/// `name` is the closure's `defn`-given name (`heap.closure(id).name`);
/// the supervisor re-resolves it on retry so a hot reload picks up the new
/// definition (see [`ResumeSlot`]).
#[inline]
pub fn record_resume(
    callee: crate::core::value::Value,
    name: Option<crate::core::value::Symbol>,
    argv: &[crate::core::value::Value],
) {
    RESUME_SLOT.with(|s| {
        let mut s = s.borrow_mut();
        match s.as_mut() {
            Some(slot) => {
                slot.callee = callee;
                slot.name = name;
                slot.argv.clear();
                slot.argv.extend_from_slice(argv);
            }
            None => {
                let mut new_argv = smallvec::SmallVec::new();
                new_argv.extend_from_slice(argv);
                *s = Some(ResumeSlot {
                    callee,
                    name,
                    argv: new_argv,
                });
            }
        }
    });
}

/// Take (and clear) the current resume slot. Called by the supervisor on
/// error. Returns `None` if no tail-call recorded yet — the supervisor then
/// re-invokes the spawn entry instead (state-loss restart, the worst-case
/// recovery).
pub(super) fn take_resume() -> Option<ResumeSlot> {
    RESUME_SLOT.with(|s| s.borrow_mut().take())
}

/// Visit the current resume slot's live `Value`s — callee + each arg — by
/// calling `visit` once per value. The GC safepoint uses this to keep the
/// supervisor's recovery target alive: without it, a tracing collection
/// between the eval-loop's `record_resume` write and the supervisor's
/// `take_resume` read could free the closure / vector / pair the slot
/// points at, and the supervisor would call back into a reused slot.
/// Zero-allocation visit so the safepoint stays in the hot path.
#[inline]
pub fn for_each_resume_root(mut visit: impl FnMut(crate::core::value::Value)) {
    RESUME_SLOT.with(|s| {
        if let Some(slot) = s.borrow().as_ref() {
            visit(slot.callee);
            for &v in slot.argv.iter() {
                visit(v);
            }
        }
    });
}

/// Read-only snapshot of the slot for save/restore around a coroutine
/// suspend. Paired with [`resume_slot_set`] in the same pattern as
/// `gc_block_save` / `gc_block_set`.
#[inline]
pub(super) fn resume_slot_save() -> Option<ResumeSlot> {
    RESUME_SLOT.with(|s| s.borrow().clone())
}

/// Write the resume slot — paired with `resume_slot_save` around a suspend,
/// and called by a fresh coroutine to wipe the residual value left on the
/// worker by a previously-running process.
#[inline]
pub(super) fn resume_slot_set(v: Option<ResumeSlot>) {
    RESUME_SLOT.with(|s| *s.borrow_mut() = v);
}

/// RAII guard: increments `GC_BLOCK` on construction, decrements on `Drop`.
/// Acquired at the top of every `eval` call and every `macroexpand_all` call —
/// the two contexts that hold unrooted LOCAL transients between safepoints.
/// `Drop` runs on a normal return *and* on a panic unwind, so the depth never
/// leaks past a frame's lifetime.
pub struct GcBlockGuard;

impl GcBlockGuard {
    #[inline]
    pub fn enter() -> Self {
        GC_BLOCK.with(|d| d.set(d.get() + 1));
        GcBlockGuard
    }
}

impl Drop for GcBlockGuard {
    #[inline]
    fn drop(&mut self) {
        GC_BLOCK.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// How many `eval` loop iterations a process runs before it must yield its worker
/// (cooperative fairness — the BEAM's mechanism). ~2000 ≈ the BEAM default; tunable.
const REDUCTION_BUDGET: u32 = 2000;

/// Stack size for each green-process coroutine. corosensei's `DefaultStack` is
/// 128 KiB out of the box; the tree-walking eval recurses one Rust frame per
/// combination, so a debug-build evaluator running the in-language test suite
/// (which spawns processes that load many test files) can run close to that.
/// 1 MiB gives comfortable headroom in debug *and* survives any future eval
/// frames a refactor accidentally widens (cf. the post-module-split overflow
/// reproducible in `cargo test -p brood --test suite` — bd4aa2d → e8567285).
/// Tunable; bump if the user lands a feature whose frames are heavier.
const CORO_STACK_BYTES: usize = 1 * 1024 * 1024;

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
        // Save this process's per-thread state before yielding: a worker may
        // pick up another process whose eval/macroexpand changes these
        // thread-locals, and we need ours back when we resume. GC-block
        // depth is critical for safepoint correctness; the resume slot is
        // critical so the supervisor sees this process's last tail-call,
        // not whichever process happened to run on the worker between our
        // suspend and resume (ADR-039).
        let saved_block = gc_block_save();
        let saved_resume = resume_slot_save();
        // SAFETY: same invariant as `receive` — the yielder is valid while this
        // coroutine is running, which is now (tick runs inside eval, inside the
        // coroutine body). Suspending returns control to the worker (`run_one`).
        unsafe { (*yptr).suspend(Suspend::Preempt) };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(saved_block);
        resume_slot_set(saved_resume);
    }
    // Root thread (yielder None): budget refreshed, never suspends.
}

// ----- the run queue + worker pool -------------------------------------------

pub(super) static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
static RUNNING: AtomicUsize = AtomicUsize::new(0); // processes inside `resume` right now
static PEAK_RUNNING: AtomicUsize = AtomicUsize::new(0);
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0); // 0 = default (≈ nproc)
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0); // worker threads actually started
static WORKERS_STARTED: Once = Once::new();

thread_local! {
    /// Set by a process's coroutine just before it returns, so `run_one` can read
    /// the exit reason (for monitor `[:down …]` delivery) once `resume` returns on
    /// this same worker thread. Cleared at the start of every scheduling quantum.
    static EXIT_REASON: RefCell<Option<Message>> = const { RefCell::new(None) };
}

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

/// A process has finished (or crashed): drop its mailbox and fire any
/// monitors, delivering `[:down <mref> <pid> <reason>]` to each watcher —
/// `Local` watchers via `deliver` (in-process mailbox push), `Remote`
/// watchers via the dist layer (an ordinary `send` to a remote pid, which
/// routes over the link). Same `[:down …]` shape in both cases — the
/// receiver code on the wire side is unchanged from local.
fn deregister(pid: u64, reason: Message) {
    // The three tables are taken **sequentially**, not nested: REGISTRY first,
    // released, then NAMES, released, then MONITORS. `add_monitor` and
    // `spawn_or_get` take REGISTRY *nested* inside MONITORS / NAMES
    // respectively for their own atomic check-and-modify steps — both are
    // deadlock-free precisely because `deregister` never holds an outer
    // lock while reaching for REGISTRY. Don't introduce a function that
    // holds REGISTRY while taking NAMES or MONITORS, or this becomes a
    // genuine ordering hazard.
    crate::core::sync::lock(&REGISTRY).remove(&pid);
    // Drop any registered names that pointed at this pid — Erlang semantics
    // (a name lives only as long as its process). Without this, named-spawn
    // would see the stale entry as "already running" and never respawn.
    crate::dist::unregister_dead_pid(pid);
    let watchers = crate::core::sync::lock(&monitor::MONITORS)
        .remove(&pid)
        .unwrap_or_default();
    for w in watchers {
        monitor::fire_down(w, pid, reason.clone());
    }
}

/// Push a ready process onto the run queue and wake a worker.
pub(super) fn enqueue(proc: Box<Process>) {
    let (lock, cv) = &*RUN;
    crate::core::sync::lock(lock).push_back(proc);
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
            let mut q = crate::core::sync::lock(lock);
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
    EXIT_REASON.with(|r| *r.borrow_mut() = None); // stale from a prior process on this worker
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.coro.resume(())));
    RUNNING.fetch_sub(1, Ordering::SeqCst);

    match outcome {
        Ok(CoroutineResult::Return(())) => {
            // The coroutine set its exit reason just before returning (see `spawn`).
            let reason = EXIT_REASON
                .with(|r| r.borrow_mut().take())
                .unwrap_or_else(|| Message::Keyword(value::intern("normal")));
            deregister(proc.pid, reason);
        }
        Ok(CoroutineResult::Yield(Suspend::Receive)) => {
            // The coroutine suspended in `receive`: it scanned the first `scanned`
            // messages with no match. Re-check under the lock — if a *new*
            // (unscanned) message arrived during the suspend window, run again;
            // otherwise park here for `send` (or the timer) to wake.
            let mut st = crate::core::sync::lock(&mailbox.state);
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
            deregister(proc.pid, Message::Keyword(value::intern("killed")));
        }
    }
}

/// `(%spawn thunk)` — run `thunk` (a 0-arg function) as a new green process.
/// Returns the new pid. The user-facing `spawn` macro wraps an arbitrary
/// expression into such a thunk (`(spawn e)` → `(%spawn (fn () e))`), so the
/// expression's free locals are captured lexically rather than passed as args.
pub fn spawn(heap: &Heap, f: Value) -> Result<u64, LispError> {
    // Promote the thunk into the shared RUNTIME region so its handle (and any
    // captured local scope) is valid in the child, which shares this runtime's
    // code via the Arcs below. A top-level function is already shared (no-op).
    let f = heap.promote(f);
    if !matches!(f, Value::Fn(_)) {
        return Err(LispError::type_err("spawn: argument must be a function"));
    }
    let prelude = heap.prelude_arc();
    let runtime = heap.runtime_arc();

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    SPAWNED.fetch_add(1, Ordering::SeqCst);
    let mailbox = Mailbox::new();
    crate::core::sync::lock(&REGISTRY).insert(pid, Arc::clone(&mailbox));

    let coro_mailbox = Arc::clone(&mailbox);
    // SAFETY: `DefaultStack::new` rejects only an unreasonable size; our
    // constant is well within `usize` and the OS' anonymous-mmap limit.
    // The expect message names the constant so the failure is debuggable.
    let stack = DefaultStack::new(CORO_STACK_BYTES).expect("DefaultStack::new(CORO_STACK_BYTES)");
    let coro = Coroutine::with_stack(stack, move |yielder: &Yielder0, _input: ()| {
        // Establish this process's context so `receive`/`self` can find it.
        CURRENT.with(|c| {
            *c.borrow_mut() = Some(Ctx {
                pid,
                mailbox: Arc::clone(&coro_mailbox),
                yielder: Some(yielder as *const Yielder0),
            });
        });
        // Wipe the worker's residual thread-local state — a previous
        // coroutine on this worker may have left these nonzero. Our depth
        // starts fresh at 0 (incremented by the eval guard below); our
        // resume slot starts empty (the supervisor will populate it on the
        // first tail call).
        gc_block_set(0);
        resume_slot_set(None);
        let mut heap = Heap::with_regions(prelude, runtime);
        heap.set_global(EnvId::GLOBAL);
        let reason = supervise(&mut heap, pid, f);
        EXIT_REASON.with(|r| *r.borrow_mut() = Some(reason));
        CURRENT.with(|c| *c.borrow_mut() = None);
    });

    ensure_workers();
    enqueue(Box::new(Process { pid, mailbox, coro }));
    Ok(pid)
}

// ----- supervisor (ADR-039 step 2) -----------------------------------------
//
// Wraps a process's main eval in a catch-and-retry loop. The eval loop
// updates `RESUME_SLOT` at every tail-call dispatch (eval::Value::Fn
// branch); on an uncaught `LispError`, we log it, sleep an exponential
// backoff, take the slot, and re-invoke from there — *same callee, same
// argv*, so the iteration's accumulator state is preserved. The whole
// point: a freshly-saved redefinition that throws doesn't kill the
// running worker; the next retry runs the corrected code with the same
// state. See `docs/supervision.md` for the model.
//
// The circuit-breaker prevents an always-throwing entry from spinning the
// process. After `MAX_RESTARTS` failed invocations in a row, we give up
// and let the process exit; its monitor watchers see `[:down …]` with the
// last error.

const MAX_RESTARTS: u32 = 10;
const BACKOFF_BASE_MS: u64 = 1;
const BACKOFF_MAX_MS: u64 = 1000;

// ----- mode gate (ADR-039 step 3, brought forward into step 2) --------------
//
// Supervision is **off by default**: a process whose eval throws an uncaught
// `LispError` exits, monitors fire `[:down …]` immediately, no retries — the
// Erlang let-it-crash baseline. That matches what most code expects and what
// the existing test suite expects (e.g. `dynamic_test.blsp`'s `dt-crasher`
// waits 500 ms for `[:down …]`).
//
// **Dev mode turns it on** — `(set-supervision! true)` from the language, or
// `BROOD_SUPERVISE=1` from the environment (consulted on the first
// `is_supervision_enabled` call and cached). The intended UX: the REPL /
// `nest dev` flips this; `--release` builds and `nest test` leave it off so
// throws surface immediately rather than being silently retried.
//
// The mode is a `bool` *cached* on first read so the hot eval path can decide
// in one atomic load. Toggling it via `(set-supervision! …)` updates both
// the env-derived cache and the active value.

static SUPERVISION: AtomicUsize = AtomicUsize::new(SUPERVISION_UNSET);
const SUPERVISION_UNSET: usize = 0;
const SUPERVISION_OFF: usize = 1;
const SUPERVISION_ON: usize = 2;

/// Is per-process supervision on? Hot-path read for `eval`'s `record_resume`
/// call and for the supervise loop. First call resolves from
/// `BROOD_SUPERVISE` and caches; afterwards a single atomic load.
#[inline]
pub fn is_supervision_enabled() -> bool {
    match SUPERVISION.load(Ordering::Relaxed) {
        SUPERVISION_ON => true,
        SUPERVISION_OFF => false,
        _ => {
            let on = std::env::var_os("BROOD_SUPERVISE")
                .map(|v| v != "0" && v != "")
                .unwrap_or(false);
            SUPERVISION.store(
                if on { SUPERVISION_ON } else { SUPERVISION_OFF },
                Ordering::Relaxed,
            );
            on
        }
    }
}

/// Turn supervision on/off at runtime. Exposed to Brood as `set-supervision!`
/// so the REPL or a dev-mode bootstrap can opt in. Affects processes spawned
/// **after** the call; in-flight supervise loops keep running.
pub fn set_supervision(on: bool) {
    SUPERVISION.store(
        if on { SUPERVISION_ON } else { SUPERVISION_OFF },
        Ordering::Relaxed,
    );
}

/// The per-process supervisor body — replaces the bare `match eval::apply`
/// the original spawn body had. Loops until either the eval returns Ok
/// (process finished normally) or we exhaust the restart budget. The exit
/// reason returned here becomes the `[:down …]` reason monitors see.
///
/// When supervision is off (the default — see [`is_supervision_enabled`]),
/// this short-circuits to a single `eval::apply` and surfaces the error
/// directly: the let-it-crash baseline, indistinguishable from the
/// pre-supervisor behaviour the rest of the test suite expects.
fn supervise(heap: &mut Heap, pid: u64, entry: Value) -> Message {
    use std::time::Duration;
    if !is_supervision_enabled() {
        return match eval::apply(heap, entry, &[], EnvId::GLOBAL) {
            Ok(_) => Message::Keyword(value::intern("normal")),
            Err(e) => {
                eprintln!("process {} died: {}", pid, e);
                Message::Vector(vec![
                    Message::Keyword(value::intern("error")),
                    Message::Str(e.to_string()),
                ])
            }
        };
    }
    let mut callee = entry;
    let mut argv: smallvec::SmallVec<[Value; 8]> = smallvec::SmallVec::new();
    let mut restarts: u32 = 0;
    let mut backoff_ms = BACKOFF_BASE_MS;
    loop {
        // Each invocation is its own eval; if it tail-calls deeper, `record_resume`
        // updates RESUME_SLOT at every iteration boundary. The recovery cost on
        // the happy path is one TLS read + two writes per tail call.
        let result = eval::apply(heap, callee, &argv, EnvId::GLOBAL);
        match result {
            Ok(_) => return Message::Keyword(value::intern("normal")),
            Err(e) => {
                eprintln!("process {} caught: {}", pid, e);
                restarts += 1;
                if restarts > MAX_RESTARTS {
                    eprintln!(
                        "process {} gave up after {} restarts in a row; last error: {}",
                        pid, restarts, e
                    );
                    return Message::Vector(vec![
                        Message::Keyword(value::intern("error")),
                        Message::Str(e.to_string()),
                    ]);
                }
                // Exponential backoff. Capped at 1s so a process retrying a
                // transient external failure doesn't sleep too long between
                // attempts. Reset to BASE_MS on successful return (handled
                // implicitly: that path doesn't loop).
                std::thread::sleep(Duration::from_millis(backoff_ms));
                backoff_ms = (backoff_ms.saturating_mul(2)).min(BACKOFF_MAX_MS);
                // Take the slot and re-invoke from there. If no tail-call
                // recorded yet (the entry itself threw before recursing),
                // retry the original entry with no args — state-loss restart,
                // the worst-case recovery.
                match take_resume() {
                    Some(slot) => {
                        // Hot-reload integration (ADR-039 × ADR-013): if the
                        // closure had a `defn`-given name, re-resolve it in
                        // the global env. The user may have `(def name …)`'d
                        // a fix between throws — that's the *entire point*
                        // of supervision being on. Without this lookup the
                        // supervisor would keep calling the old, throwing
                        // closure handle stored in `slot.callee` forever
                        // (up to MAX_RESTARTS), defeating the hot reload.
                        // Fall back to the stored handle if the name no
                        // longer resolves to a `Fn` (someone `def`'d it to
                        // something non-callable, or `undef`'d it).
                        callee = slot.callee;
                        if let Some(name) = slot.name {
                            if let Some(latest) = heap.env_get(EnvId::GLOBAL, name) {
                                if matches!(latest, Value::Fn(_) | Value::Native(_)) {
                                    callee = latest;
                                }
                            }
                        }
                        argv.clear();
                        argv.extend_from_slice(&slot.argv);
                    }
                    None => {
                        // Re-invoke `entry` with no args. (callee already set
                        // to `entry` on first iteration; restore on
                        // subsequent fall-through.)
                        callee = entry;
                        argv.clear();
                    }
                }
            }
        }
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx().pid
}

/// Are we currently running inside a **green** (spawned) process — as opposed
/// to the *root* thread (the REPL / file runner / MCP dispatcher)? `true`
/// when `CURRENT` has a yielder, i.e. we entered through a coroutine. Used
/// by the eval-time `unbound` raise to attach a scheduler-race hint
/// (the under-load failure mode `docs/claude-demo-findings.md` flagged —
/// concurrent prelude lookups racing) **and** by the supervisor's
/// `record_resume` guard in the eval loop (ADR-039) — so this is called
/// from inside eval on every tail call.
///
/// Uses `try_borrow` rather than `borrow`: every site that mutates `CURRENT`
/// in this crate evaluates its RHS with no calls back into `in_green_process`
/// today (Arc::clone, struct construction — see the `borrow_mut` audit in
/// docs/devlog 2026-05-28), so the `try_borrow` should always succeed; but
/// if a future change adds a path where it doesn't, the supervisor's
/// hot-path guard would otherwise panic with "RefCell already borrowed"
/// halfway through a tail call. Returning `false` on a contended borrow
/// degrades gracefully — the recovery slot just isn't written for that one
/// call. Never panics; returns `false` if `CURRENT` is unset *or* if a
/// borrow is held.
pub fn in_green_process() -> bool {
    CURRENT.with(|c| {
        c.try_borrow()
            .ok()
            .and_then(|b| b.as_ref().map(|ctx| ctx.yielder.is_some()))
            .unwrap_or(false)
    })
}

/// Wrap a local process id in a [`Value::Pid`] tagged with this runtime's node
/// identity — what `self`/`spawn` hand back. The node part makes the pid routable
/// off-node once the holder is on another runtime.
pub fn pid_value(id: u64) -> Value {
    Value::Pid {
        node: crate::dist::local_node(),
        id,
    }
}

/// The current process's context. A coroutine sets this itself; the first time a
/// *root* thread (the REPL / file runner) uses `self`/`receive`, register it as a
/// blocking-mailbox process so it can participate in message passing.
pub(super) fn ensure_ctx() -> Ctx {
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            return ctx.clone();
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let mailbox = Mailbox::new();
        crate::core::sync::lock(&REGISTRY).insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx {
            pid,
            mailbox,
            yielder: None,
        };
        *c.borrow_mut() = Some(ctx.clone());
        ctx
    })
}
