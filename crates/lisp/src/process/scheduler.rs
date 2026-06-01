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
//! - [`GC_BLOCK`] — eval/macroexpand nesting depth; feeds the stack-overflow
//!   byte guard (no longer the GC safepoint — ADR-061). [`MACRO_BLOCK`] —
//!   compile-pass depth; the GC safepoint suppresses collection while it's
//!   nonzero. Both saved/restored around every suspend so workers
//!   multiplexing several processes don't leak each other's depths.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};

use corosensei::stack::DefaultStack;
use corosensei::{Coroutine, CoroutineResult, Yielder};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::process::keywords as pk;
use crate::error::LispError;
use crate::eval;

use super::mailbox::{Mailbox, REGISTRY, ST_RUNNABLE, ST_RUNNING};
use super::message::Message;
use super::links;
use super::monitor;

/// Why a green process's coroutine yielded control back to its worker.
pub(super) enum Suspend {
    /// Blocked in `receive` on an empty mailbox — park until a message (or timer).
    Receive,
    /// Preempted by the reduction counter — still runnable, re-queue immediately.
    Preempt,
    /// An exit signal (`(exit pid reason)`) targeted this process. `run_one`
    /// `deregister`s it with `reason` and drops it — corosensei force-unwinds the
    /// suspended coroutine (running destructors), and it is never re-enqueued.
    /// Untrappable **by construction**: it fires at the scheduler level, below
    /// Brood's `%try`, so a `:kill` exit can't be caught.
    Kill(Message),
}

pub(super) type Yielder0 = Yielder<(), Suspend>;
type Coro = Coroutine<(), Suspend, ()>;

/// A green process: its mailbox plus the coroutine carrying its computation
/// (which owns its `Heap`). Pinned to a single worker thread at spawn time
/// (`worker_id`); the scheduler routes every re-enqueue back to that worker,
/// so a coroutine is only ever resumed on the OS thread that first ran it.
/// `Send` for the moment from spawn → worker (the queue owns it exclusively
/// in transit), and never again across threads.
pub(super) struct Process {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
    /// Which worker owns this process for its lifetime. Assigned at spawn
    /// (round-robin); preempt/receive re-enqueue routes back to the same
    /// worker's queue. Prevents corosensei cross-thread resume hazards
    /// (KI-1b in docs/known-issues.md — clobbered return addresses under
    /// preempt-induced migration).
    pub(super) worker_id: usize,
    coro: Coro,
}

// SAFETY: corosensei marks `Coroutine` `!Send` conservatively. A process is
// only ever moved across threads *once* — from `spawn` (caller's thread) into
// its assigned worker's queue (via `enqueue`). After that, the assigned worker
// is the only thread that ever pops it; preempt and receive both re-enqueue to
// the *same* worker's queue. So at every `resume` site the proc is owned
// exclusively by one thread, and no cross-thread `resume` ever happens. The
// captured state (heap, Arcs, message values) is all `Send`.
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
    /// The **output-capture stack**. Empty means no capture; output goes to real
    /// stdout. When non-empty, this process's `print` / terminal output appends to
    /// the **top** buffer instead (see builtins' `capture_write`). It's a *stack*
    /// so captures **nest**: `begin_capture` pushes a fresh buffer, `take_capture`
    /// pops the top and returns its text — so a `with-out-str` running inside a
    /// `nest mcp` `tools/call` (which itself installs a capture) drains only its
    /// own buffer and the MCP envelope's capture survives underneath. A SPAWNED
    /// child **inherits** a snapshot of the parent's stack (the same `Arc`s), so a
    /// process tree the dispatcher ran under a watchdog still diverts off the
    /// JSON-RPC channel even on a worker thread. Each `Arc` is minted fresh per
    /// `begin_capture`, so concurrent captures never share a buffer. Rides
    /// `CURRENT`, so it's saved/restored across suspend for free.
    pub(super) capture: Vec<Arc<Mutex<String>>>,
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

    /// An eval **deadline** (wall clock) for this thread, or `None`. The `nest mcp`
    /// dispatcher sets it around an `eval`/`load` so a runaway (an infinite Brood
    /// loop) is aborted — see [`deadline_exceeded`], checked in eval's `'tail:`
    /// loop — instead of wedging the server. Inline (no spawn), so the dispatcher's
    /// error / panic / output-capture handling is untouched; a *native* blocking
    /// call still can't be interrupted (it never reaches the check — the same limit
    /// `(exit … :kill)` has).
    static DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    /// Call counter so [`deadline_exceeded`] reads the clock only every ~1024 ticks;
    /// the no-deadline fast path is a single `Cell` get, so eval's loop pays ~nothing.
    static DEADLINE_TICK: Cell<u32> = const { Cell::new(0) };

    /// GC-block depth: how many `eval` / `macroexpand_all` frames are active on
    /// this thread. Since ADR-061 this no longer gates the GC safepoint (which now
    /// collects at any eval depth — see `MACRO_BLOCK` and the operand-stack rooting
    /// in `eval::eval`); it survives only to feed the stack-overflow byte guard,
    /// which establishes its base at the outermost eval (`gc_block_depth() <= 1`).
    ///
    /// Per-process: reset to 0 at coroutine entry and saved/restored around
    /// suspend (see `spawn` / `preempt` / `wait_for_message`), so workers
    /// multiplexing several processes don't leak each other's depths. The root
    /// thread doesn't multiplex, so its depth flows naturally.
    static GC_BLOCK: Cell<u32> = const { Cell::new(0) };

    /// Stack-pointer base for the [`stack_overflow_check`] byte guard: the sp of
    /// the *outermost* eval on this coroutine. `0` = unset (established by the
    /// next eval). Reset to 0 at coroutine entry and saved/restored across every
    /// suspend exactly like `GC_BLOCK`, because a stackful coroutine resumes on
    /// its own stack — the base is constant for the coroutine's life, but a
    /// worker running other coroutines in between must not clobber it.
    static STACK_BASE: Cell<usize> = const { Cell::new(0) };

    /// Compile-pass depth (ADR-061): bumped by `macroexpand_all`'s
    /// [`MacroBlockGuard`] for the duration of macro expansion. The eval safepoint
    /// collects only when this is **zero** — i.e. never *during* the compile pass,
    /// which (unlike runtime eval) holds partially-built LOCAL forms in unrooted
    /// Rust locals. This is what lets the safepoint otherwise fire at ANY eval
    /// depth (the operand stack roots runtime transients; the compile pass opts
    /// out instead of being rooted). Reset to 0 at coroutine entry and
    /// saved/restored across suspend, exactly like `GC_BLOCK`/`STACK_BASE`, since
    /// expansion can suspend (its inner evals `tick`).
    static MACRO_BLOCK: Cell<u32> = const { Cell::new(0) };
}

/// Current GC-block depth — feeds the stack-overflow byte guard's base
/// (`gc_block_depth() <= 1` = outermost eval). No longer gates the GC safepoint
/// (ADR-061); see `MACRO_BLOCK`.
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
    #[cfg(debug_assertions)]
    if std::env::var_os("BROOD_TRACE_GCBLOCK").is_some() {
        eprintln!(
            "[gcblock] SET({}) thread={:?}",
            n,
            std::thread::current().id()
        );
    }
}

/// True while the macro-expansion compile pass is on the stack — the eval
/// safepoint suppresses collection then (see `MACRO_BLOCK`).
#[inline]
pub fn macro_block_active() -> bool {
    MACRO_BLOCK.with(|d| d.get() > 0)
}

/// Read the compile-pass depth for save/restore around a coroutine suspend.
#[inline]
pub(super) fn macro_block_save() -> u32 {
    MACRO_BLOCK.with(|d| d.get())
}

/// Write the compile-pass depth — paired with `macro_block_save` around a
/// suspend, and used by a fresh coroutine to wipe the worker's residual value.
#[inline]
pub(super) fn macro_block_set(n: u32) {
    MACRO_BLOCK.with(|d| d.set(n));
}

/// RAII guard: increments `MACRO_BLOCK` for the lifetime of a `macroexpand_all`
/// call, so the eval safepoint won't collect during the compile pass (whose
/// transients aren't operand-stack rooted). `Drop` runs on every return path.
pub struct MacroBlockGuard;

impl MacroBlockGuard {
    #[inline]
    pub fn enter() -> Self {
        MACRO_BLOCK.with(|d| d.set(d.get() + 1));
        MacroBlockGuard
    }
}

impl Drop for MacroBlockGuard {
    #[inline]
    fn drop(&mut self) {
        MACRO_BLOCK.with(|d| d.set(d.get().saturating_sub(1)));
    }
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
        let new = GC_BLOCK.with(|d| {
            let n = d.get() + 1;
            d.set(n);
            n
        });
        #[cfg(debug_assertions)]
        if std::env::var_os("BROOD_TRACE_GCBLOCK").is_some() {
            eprintln!(
                "[gcblock] enter -> {} thread={:?}",
                new,
                std::thread::current().id()
            );
        }
        #[cfg(not(debug_assertions))]
        let _ = new;
        GcBlockGuard
    }
}

impl Drop for GcBlockGuard {
    #[inline]
    fn drop(&mut self) {
        let new = GC_BLOCK.with(|d| {
            let n = d.get().saturating_sub(1);
            d.set(n);
            n
        });
        #[cfg(debug_assertions)]
        if std::env::var_os("BROOD_TRACE_GCBLOCK").is_some() {
            eprintln!(
                "[gcblock] drop -> {} thread={:?}",
                new,
                std::thread::current().id()
            );
        }
        #[cfg(not(debug_assertions))]
        let _ = new;
    }
}

/// How many `eval` loop iterations a process runs before it must yield its worker
/// (cooperative fairness — the BEAM's mechanism). ~2000 ≈ the BEAM default; tunable.
const REDUCTION_BUDGET: u32 = 2000;

/// Stack size for each green-process coroutine. corosensei's `DefaultStack` is
/// 128 KiB out of the box; the tree-walking eval recurses one Rust frame per
/// combination, so a debug-build evaluator running the in-language test suite
/// (which spawns processes that load many test files) can run close to that.
/// **16 MiB**: debug eval frames are heavy (no inlining + poison checks) — one
/// nested `eval` frame is several KiB, and non-tail recursion stacks ~2 of them
/// per level, so a few hundred levels of legitimate non-tail recursion already
/// costs low-double-digit MiB of stack. We want the [`stack_budget`] guard to
/// allow building structures at least as deep as `MAX_MESSAGE_DEPTH` (256) with
/// headroom, and still fire a clean [`STACK_DEPTH_EXCEEDED`] error well before
/// the real guard page (with room for the error-construction frames). The pages
/// are mmap'd lazily, so unused tail pages stay uncommitted — the higher ceiling
/// costs ~0 until the depth actually needs it (a shallow process resides a few
/// KiB; only deep recursion commits more, and a runaway is killed by the guard
/// before it commits much past the budget). The `brood`/`nest` binaries re-home
/// their root thread onto a stack of this same size (see `cli`/`nest` `main`), so
/// the budget below is uniform and safe on both the root thread and coroutines.
/// Tunable; bump if a feature lands with heavier frames.
pub const CORO_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Stack-budget guard against runaway *non-tail* recursion (ADR-043). The
/// evaluator is a native tree-walker: every nested `eval`/`macroexpand` frame
/// (i.e. every level of non-tail recursion) consumes real Rust stack, and an
/// unbounded one — `(defn boom (n) (+ 1 (boom (+ n 1))))` — would overflow the
/// [`CORO_STACK_BYTES`] coroutine stack as a **`SIGSEGV` the host can't
/// `catch_unwind`**, taking down the whole REPL / `nest mcp` server. The guard
/// turns that into a clean, catchable [`STACK_DEPTH_EXCEEDED`] error.
///
/// We measure **stack bytes used**, not frame *count*. Frame count (the old
/// `GC_BLOCK`-ceiling approach) can't work: a heavy frame (`(+ 1 (boom …))`)
/// and a light one (`{:next (f …)}`) differ several-fold in bytes, so any single
/// frame-count ceiling is simultaneously too low for legitimate deep recursion
/// and too high to stop a heavy runaway before the real overflow. Bytes are the
/// thing the stack actually runs out of, so a byte budget is both safe and
/// permissive. See [`STACK_BASE`] for how the per-coroutine base is tracked.
///
/// Default: [`CORO_STACK_BYTES`] minus a margin generous enough to absorb the
/// frame we're in plus the error-construction path (`format!` + `LispError`)
/// without itself overflowing. Override with `BROOD_STACK_BUDGET=<size>`
/// (e.g. `6M`); `0` or malformed falls back to the default.
const STACK_BUDGET_MARGIN: usize = 4 * 1024 * 1024;

/// The active stack budget in bytes, read once from `BROOD_STACK_BUDGET` (or
/// derived from [`CORO_STACK_BYTES`]). Cached so the per-`eval` check is a load
/// + compare on the hot path.
pub fn stack_budget() -> usize {
    use std::sync::LazyLock;
    static BUDGET: LazyLock<usize> = LazyLock::new(|| {
        std::env::var("BROOD_STACK_BUDGET")
            .ok()
            .and_then(|s| crate::core::alloc::parse_size(&s))
            .filter(|&n| n > 0)
            .unwrap_or(CORO_STACK_BYTES.saturating_sub(STACK_BUDGET_MARGIN))
    });
    *BUDGET
}

/// `Some(used_bytes)` when the current stack usage has crossed [`stack_budget`],
/// else `None`. `sp` is the caller's stack-pointer probe (the address of a local
/// in the `eval` frame); the per-coroutine base ([`STACK_BASE`]) is the sp of the
/// *outermost* eval on this coroutine. Stack grows down, so `base - sp` is the
/// bytes consumed by the nested-eval recursion since the outermost frame.
///
/// Self-healing: the base is recorded the first time it's seen unset (`0`) and
/// reset to `0` at coroutine entry, and saved/restored across every suspend (so
/// a worker multiplexing coroutines never compares against another coroutine's
/// base). As a final backstop, an implausibly large `used` (> a whole stack —
/// impossible within one coroutine) is treated as a stale base from a missed
/// switch and silently rebased rather than firing a false positive.
#[inline]
pub fn stack_overflow_check(sp: usize) -> Option<usize> {
    // Called from `eval` *after* its `GcBlockGuard` increment, so `gc_block_depth`
    // is this frame's depth (1 = the outermost eval on this coroutine/thread).
    STACK_BASE.with(|b| {
        if gc_block_depth() <= 1 {
            // Outermost eval frame — (re)establish the base *here*, every time.
            // This is what keeps the root thread honest: it never resets the base
            // at a coroutine boundary (it isn't a coroutine), and the base set
            // during prelude load would otherwise be stale by the time a user
            // form runs. Re-stamping at every depth-1 entry fixes that, and is
            // harmless in coroutines (their first eval is depth 1 anyway).
            b.set(sp);
            return None;
        }
        let base = b.get();
        if base == 0 || sp > base {
            // No base yet, or we're somehow shallower than it — rebase, fail safe.
            b.set(sp);
            return None;
        }
        let used = base - sp;
        if used > CORO_STACK_BYTES {
            // Larger than any single coroutine stack: the base must be stale (a
            // suspend/resume path we didn't account for). Rebase rather than
            // reject a legitimate program.
            b.set(sp);
            return None;
        }
        if used > stack_budget() {
            Some(used)
        } else {
            None
        }
    })
}

/// Read the per-coroutine stack base for save/restore around a suspend (paired
/// with [`stack_base_set`], mirroring [`gc_block_save`]).
#[inline]
pub(super) fn stack_base_save() -> usize {
    STACK_BASE.with(|b| b.get())
}

/// Write the per-coroutine stack base — paired with [`stack_base_save`] around a
/// suspend, and called with `0` at coroutine entry so the coroutine's first eval
/// establishes a fresh base instead of inheriting the worker's residual value.
#[inline]
pub(super) fn stack_base_set(n: usize) {
    STACK_BASE.with(|b| b.set(n));
}

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

/// Set (or clear with `None`) this thread's eval deadline. Paired set/clear by the
/// `nest mcp` dispatcher around a guarded `eval`/`load`. Thread-local: only the
/// thread running the guarded eval is affected.
pub fn set_deadline(at: Option<std::time::Instant>) {
    DEADLINE.with(|d| d.set(at));
    DEADLINE_TICK.with(|c| c.set(0));
}

/// True iff a deadline is set and has passed. The clock is read only every ~1024
/// calls, so the common (no-deadline) path is one `Cell` get — eval's loop checks
/// this every combination but pays almost nothing when no deadline is armed.
pub fn deadline_exceeded() -> bool {
    DEADLINE.with(|d| match d.get() {
        None => false,
        Some(at) => DEADLINE_TICK.with(|c| {
            let n = c.get().wrapping_add(1);
            c.set(n);
            n % 1024 == 0 && std::time::Instant::now() >= at
        }),
    })
}

/// Budget exhausted: refresh it, then — if we're a green process — yield the worker
/// (re-queued Ready by `run_one`). Re-establishes `CURRENT` after resume, since the
/// worker may have run other processes meanwhile (cf. `receive`).
fn preempt() {
    let ctx = match CURRENT.with(|c| c.borrow().clone()) {
        Some(c) => {
            // The budget is exhausted, so a full quantum's worth of reductions was
            // consumed — accumulate it into the per-process `:reductions` total
            // before refreshing. (`run_one` adds the *partial* final quantum for a
            // `receive`/exit yield, where `preempt` isn't called; the two paths are
            // mutually exclusive per quantum, so no double-count.) Without this the
            // count stayed 0 for CPU-bound processes — exactly the ones the observer
            // most wants to flag.
            c.mailbox
                .reductions
                .fetch_add(REDUCTION_BUDGET as u64, Ordering::Relaxed);
            c
        }
        None => {
            // No process context (e.g. prelude build) — nothing to yield to; just
            // refresh the budget so the caller keeps running.
            REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
            return;
        }
    };
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    if let Some(yptr) = ctx.yielder {
        // Hard exit (`(exit pid :kill)`): die now, untrappably. Checked here at the
        // reduction boundary so a tight CPU loop — which never reaches `receive` —
        // still dies. (Soft exits wait for the next `receive`; see `receive_match`.)
        // We never resume after a `Kill` suspend, so no thread-local save/restore.
        if let Some(reason) = ctx.mailbox.pending_kill() {
            if is_kill_reason(&reason) {
                // SAFETY: same invariant as the `Preempt` suspend just below — the
                // yielder is valid while this coroutine runs (tick → here, inside eval).
                unsafe { (*yptr).suspend(Suspend::Kill(reason)) };
            }
        }
        // Save this process's per-thread state before yielding: a worker may
        // pick up another process whose eval/macroexpand changes these
        // thread-locals, and we need ours back when we resume. GC-block
        // depth is critical for safepoint correctness.
        let saved_block = gc_block_save();
        let saved_base = stack_base_save();
        let saved_macro = macro_block_save();
        // SAFETY: same invariant as `receive` — the yielder is valid while this
        // coroutine is running, which is now (tick runs inside eval, inside the
        // coroutine body). Suspending returns control to the worker (`run_one`).
        unsafe { (*yptr).suspend(Suspend::Preempt) };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(saved_block);
        stack_base_set(saved_base);
        macro_block_set(saved_macro);
    }
    // Root thread (yielder None): budget refreshed, never suspends.
}

// ----- the run queue + worker pool -------------------------------------------

pub(super) static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
/// child pid → the pid that `spawn`ed it. Populated at `spawn`, removed at
/// `deregister` (a parent record lives only as long as the child). Backs
/// `process-info`'s `:parent` (and a future process-tree view). A side table
/// rather than a `Process` field because the `Process` isn't reachable from the
/// registry while it runs; this is.
static PARENTS: LazyLock<Mutex<HashMap<u64, u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The pid that spawned `pid`, or `None` for the root process (or a dead pid).
pub fn parent_of(pid: u64) -> Option<u64> {
    crate::core::sync::lock(&PARENTS).get(&pid).copied()
}
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

/// One worker's run queue + the condvar that parks it when the queue is empty.
type WorkerQueue = (Mutex<VecDeque<Box<Process>>>, Condvar);

/// Per-worker run queues. Index = `worker_id`. Each worker pops only from its
/// own queue (no shared queue, no work stealing). A process is assigned to one
/// worker at spawn time and stays there for its lifetime — preempt and receive
/// re-enqueue to the same worker's queue. The Vec is sized once at the first
/// `ensure_workers` from `worker_count()`, then never resized.
static WORKERS: LazyLock<Vec<WorkerQueue>> = LazyLock::new(|| {
    (0..worker_count())
        .map(|_| (Mutex::new(VecDeque::new()), Condvar::new()))
        .collect()
});

/// Rotating start point for `assign_worker`'s least-loaded scan. Read +
/// incremented under relaxed ordering — the only requirement is approximate
/// rotation; an occasional duplicate or skipped index is fine.
static NEXT_WORKER: AtomicUsize = AtomicUsize::new(0);

/// Pick the worker that a fresh `Process` should be pinned to (it stays there for
/// life — no migration, so the KI-1b cross-thread-resume hazard never arises; see
/// docs/concurrency-v2.md). **Least-loaded with a rotating start:** scan the
/// queues beginning at a round-robin offset and choose the shortest, breaking
/// ties toward the rotation. When load is even (the common case — most queues
/// empty) this degrades to plain round-robin; when one worker is backed up (a
/// spawn burst, or uneven drain) fresh processes steer to idle workers instead.
/// Queue lengths are sampled via `try_lock`, so a momentarily-contended queue is
/// skipped rather than blocking the spawner. Validated clean (incl. under
/// `BROOD_GC_STRESS`) in the Track-A experiment; replaces pure round-robin.
fn assign_worker() -> usize {
    let n = worker_count().max(1);
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    let mut best = start;
    let mut best_len = WORKERS[start]
        .0
        .try_lock()
        .map(|q| q.len())
        .unwrap_or(usize::MAX);
    for off in 1..n {
        if best_len == 0 {
            break; // can't do better than an empty queue
        }
        let i = (start + off) % n;
        let len = WORKERS[i]
            .0
            .try_lock()
            .map(|q| q.len())
            .unwrap_or(usize::MAX);
        if len < best_len {
            best_len = len;
            best = i;
        }
    }
    best
}

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
    if let Some(s) = std::env::var_os("BROOD_J") {
        if let Some(n) = s.to_str().and_then(|t| t.parse::<usize>().ok()) {
            if n > 0 {
                return n;
            }
        }
    }
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
/// A human descriptor for a process in death/crash diagnostics: its registered
/// name plus pid when it has one (`ticker (pid 6)`), else the bare pid (`6`).
/// Read the name *before* `deregister` clears it. Used only on the cold death
/// path, so the `name_for_pid` scan is fine. (Keyword and symbol registrations
/// share the interner, so the name prints without a leading `:`.)
fn proc_descr(pid: u64) -> String {
    match crate::dist::name_for_pid(pid) {
        Some(name) => format!("{} (pid {})", value::symbol_name(name), pid),
        None => pid.to_string(),
    }
}

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
    // Balances the `live_process_inc` in `spawn` (see the process-count-aware
    // `gc_floor`). `deregister` runs exactly once per spawned green process.
    crate::core::heap::live_process_dec();
    crate::core::sync::lock(&PARENTS).remove(&pid);
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
    // Links (ADR-067), after monitors and with no table lock held: notify every
    // linked peer — a trappable `[:EXIT pid reason]` if it traps, else an abnormal
    // reason propagates as a hard kill that cascades through *its* links. Mirrors
    // the sequential lock discipline above (never holds REGISTRY/MONITORS here).
    links::notify_peers(pid, &reason);
}

/// The untrappable hard-kill reason — Erlang's `exit(pid, kill)`. A `:kill` exit
/// fires at the next reduction tick (`preempt`); any other reason is the soft
/// signal that waits for the next `receive` iteration.
fn is_kill_reason(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == value::intern(pk::KILL))
}

/// `(exit pid reason)` — deliver an exit signal to a green process (Erlang
/// `exit/2`). `reason = :kill` is the **untrappable hard** kill: the target dies at
/// its next reduction tick (`preempt`), or immediately if it's parked. Any other
/// reason is the **soft** signal: the target dies at its next `receive` iteration
/// (a tight non-`receive` loop won't honour it — cooperative). Monitors fire
/// `[:down mref pid reason]`. A no-op for an unknown / already-dead pid, so it's
/// idempotent (double-exit, exit-of-dead are safe).
pub fn exit(pid: u64, reason: Message) {
    let mailbox = match crate::core::sync::lock(&REGISTRY).get(&pid).cloned() {
        Some(mb) => mb,
        None => return, // already dead / never existed
    };
    mailbox.request_kill(reason);
    // If the target is parked in `receive` it isn't running, so it'll never reach a
    // `tick` (preempt) or re-enter `receive` on its own. Wake it by re-queueing onto
    // **its own worker** (`enqueue` routes by `worker_id`) — exactly how `send`/the
    // timer wake a parked process — and it self-kills at `receive_match`'s loop-top
    // `kill_pending` check, on the worker that owns it. We must NOT drop it here:
    // dropping force-unwinds its coroutine, and that would resume the coroutine on
    // *this* (the exiter's) thread, not its owning worker — the cross-thread-resume
    // hazard (KI-1b). Taking the waiter under the state lock serialises with
    // `run_one`'s park: either we take an already-parked process here, or `run_one`
    // sees `kill_pending` and retires it instead of parking (exactly one wins).
    let parked = crate::core::sync::lock(&mailbox.state).waiter.take();
    if let Some(proc) = parked {
        enqueue(proc);
    }
}

/// Push a ready process onto its owning worker's queue and wake that worker.
/// Routing by `proc.worker_id` keeps the coroutine on the same OS thread for
/// its lifetime — no cross-thread `resume`, no KI-1b migration hazard.
pub(super) fn enqueue(proc: Box<Process>) {
    let wid = proc.worker_id;
    proc.mailbox.status.store(ST_RUNNABLE, Ordering::Relaxed); // queued, awaiting a worker turn
    let (lock, cv) = &WORKERS[wid];
    crate::core::sync::lock(lock).push_back(proc);
    cv.notify_one();
}

/// Start the worker pool exactly once (on the first `spawn`).
fn ensure_workers() {
    WORKERS_STARTED.call_once(|| {
        // Force the WORKERS LazyLock to initialise *now*, with the pool size
        // committed by the current `set_max_parallel` (or the default ≈ nproc).
        // A later `set_max_parallel` won't resize the pool — sized once.
        let n = WORKERS.len();
        ACTIVE_WORKERS.store(n, Ordering::SeqCst);
        for wid in 0..n {
            std::thread::spawn(move || worker_loop(wid));
        }
    });
}

fn worker_loop(wid: usize) {
    loop {
        let proc = {
            let (lock, cv) = &WORKERS[wid];
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
    mailbox.status.store(ST_RUNNING, Ordering::Relaxed); // about to resume on this worker

    let live = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    PEAK_RUNNING.fetch_max(live, Ordering::SeqCst);
    // Fresh reduction budget for this scheduling quantum (decremented in eval's loop
    // via `tick`; at zero the process preempts itself — see `tick`/`preempt`).
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    EXIT_REASON.with(|r| *r.borrow_mut() = None); // stale from a prior process on this worker
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.coro.resume(())));
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    // Accumulate the reductions this quantum consumed (budget minus what's left;
    // a preempted process left 0) into the per-process total for `process-info`'s
    // `:reductions`. The coroutine shares this worker's `REDUCTIONS` TLS, so its
    // post-yield value is the remainder. Erlang counts reductions the same way.
    let used = REDUCTION_BUDGET.saturating_sub(REDUCTIONS.with(|r| r.get()));
    mailbox.reductions.fetch_add(used as u64, Ordering::Relaxed);

    match outcome {
        Ok(CoroutineResult::Return(())) => {
            // The coroutine set its exit reason just before returning (see `spawn`).
            let reason = EXIT_REASON
                .with(|r| r.borrow_mut().take())
                .unwrap_or_else(|| Message::Keyword(value::intern(pk::NORMAL)));
            deregister(proc.pid, reason);
        }
        Ok(CoroutineResult::Yield(Suspend::Receive)) => {
            // The coroutine suspended in `receive`: it scanned the first `scanned`
            // messages with no match. Re-check under the lock — if a *new*
            // (unscanned) message arrived during the suspend window, run again;
            // otherwise park here for `send` (or the timer) to wake.
            let mut st = crate::core::sync::lock(&mailbox.state);
            if mailbox.kill_pending.load(Ordering::Relaxed) {
                // An `(exit …)` raced in while we were heading to park. Die instead
                // of parking — the state lock serialises this with `exit`'s
                // waiter-take, so a process can't end up parked-with-a-pending-kill
                // and stuck forever. (Without this check, `exit` running in the
                // window between `suspend(Receive)` and this `Some(proc)` park would
                // set the flag, find no waiter, and return — leaving us parked.)
                let reason = st
                    .kill
                    .take()
                    .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
                drop(st);
                deregister(proc.pid, reason);
                // `proc` dropped here → corosensei force-unwinds the parked coroutine.
            } else if st.queue.len() > st.scanned {
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
        Ok(CoroutineResult::Yield(Suspend::Kill(reason))) => {
            // `(exit pid reason)` fired at a reduction (`preempt`) or `receive`
            // safepoint. Retire the process with `reason`; dropping `proc` at the end
            // of this arm force-unwinds its suspended coroutine (runs destructors).
            deregister(proc.pid, reason);
        }
        Err(_) => {
            eprintln!("process {} panicked", proc_descr(proc.pid));
            deregister(proc.pid, Message::Keyword(value::intern(pk::KILLED)));
        }
    }
}

/// `(%spawn thunk)` — run `thunk` (a 0-arg function) as a new green process.
/// Returns the new pid. The user-facing `spawn` macro wraps an arbitrary
/// expression into such a thunk (`(spawn e)` → `(%spawn (fn () e))`), so the
/// expression's free locals are captured lexically rather than passed as args.
/// Erlang-style let-it-crash: an uncaught throw kills the process, monitors
/// fire `[:down :error …]` immediately.
pub fn spawn(heap: &Heap, f: Value) -> Result<u64, LispError> {
    // The spawner is the parent. Captured before minting the child pid so the
    // root (whose ctx/pid is lazily minted here on its first spawn) gets the
    // lower id. `ensure_ctx` needs no heap.
    let parent = self_pid();
    // Inherit a snapshot of the spawner's capture stack (the same `Arc`s), so a
    // child of an MCP-watchdog'd handler still diverts its output off the JSON-RPC
    // channel. An empty stack (the common case) clones to an empty `Vec`.
    let inherited_capture =
        CURRENT.with(|c| c.borrow().as_ref().map(|ctx| ctx.capture.clone()).unwrap_or_default());
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
    // Live green-process gauge: drives the process-count-aware `gc_floor` so a
    // fan-out of many churny processes doesn't each climb to the single-process
    // GC ceiling. Balanced by the `live_process_dec` in `deregister`.
    crate::core::heap::live_process_inc();
    crate::core::sync::lock(&PARENTS).insert(pid, parent);
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
                capture: inherited_capture,
            });
        });
        // Wipe the worker's residual GC-block depth and stack base — a previous
        // coroutine on this worker may have left them nonzero. Our depth starts
        // fresh at 0 (incremented by the eval guard below), and our stack base is
        // re-established by this coroutine's first `eval` (it runs on our own
        // freshly-allocated coroutine stack, not the worker's).
        gc_block_set(0);
        stack_base_set(0);
        macro_block_set(0);
        let mut heap = Heap::with_regions(prelude, runtime);
        heap.set_global(EnvId::GLOBAL);
        // Run the process body. Its memory stays bounded with no help from the
        // author: the body runs at the depth-1 eval safepoint where Stage B's
        // automatic copying GC fires (ADR-055), so a long-running tail / receive
        // loop reclaims its per-iteration garbage. (Pre-ADR-055 a `(hibernate)`
        // sentinel was caught here and flushed the arena manually; automatic GC
        // made it redundant and the primitive was removed — docs/memory-review.md.)
        //
        // Route through the VM when enabled (ADR-076): `eval::apply` is the
        // tree-walk entry, so without this a green process ran its whole body
        // tree-walked even under `BROOD_VM=1` — ~4–5× slower than the same code at
        // top level (which goes through the VM). `apply_value` runs a VM-eligible
        // body on the bytecode engine and falls back to `eval::apply` otherwise.
        let body = if eval::compile::vm_enabled() {
            eval::compile::apply_value(&mut heap, f, &[], EnvId::GLOBAL)
        } else {
            eval::apply(&mut heap, f, &[], EnvId::GLOBAL)
        };
        let reason = match body {
            Ok(_) => Message::Keyword(value::intern(pk::NORMAL)),
            Err(e) => {
                eprintln!("process {} died: {}", proc_descr(pid), e.located());
                Message::Vector(vec![
                    Message::Keyword(value::intern(pk::ERROR)),
                    Message::Str(e.to_string()),
                ])
            }
        };
        EXIT_REASON.with(|r| *r.borrow_mut() = Some(reason));
        CURRENT.with(|c| *c.borrow_mut() = None);
    });

    ensure_workers();
    let worker_id = assign_worker();
    enqueue(Box::new(Process {
        pid,
        mailbox,
        worker_id,
        coro,
    }));
    Ok(pid)
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
/// concurrent prelude lookups racing). Never panics; returns `false` if
/// `CURRENT` is unset.
pub fn in_green_process() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|ctx| ctx.yielder.is_some())
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
            capture: Vec::new(),
        };
        *c.borrow_mut() = Some(ctx.clone());
        ctx
    })
}

/// Push a fresh output-capture buffer onto the current process's capture stack
/// (minting its ctx if needed). While it's the top of the stack, this process's —
/// and any it `spawn`s — `print` / terminal output appends to that buffer instead
/// of real stdout (see builtins' `capture_write`). Captures **nest**: an inner
/// `begin_capture` shadows an outer one until its matching `take_capture`. The
/// `nest mcp` dispatcher uses this so a tool handler's output (even a handler it
/// runs in a spawned, killable process) can't corrupt the JSON-RPC stdout stream;
/// a `with-out-str` *inside* such a handler nests cleanly on top. A fresh `Arc` per
/// call → concurrent captures never collide.
pub fn begin_capture() {
    ensure_ctx();
    let buf = Arc::new(Mutex::new(String::new()));
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow_mut().as_mut() {
            ctx.capture.push(buf);
        }
    });
}

/// Pop the top capture buffer and return what was written to it, or `None` if no
/// capture was active. Drains the buffer (a spawned child wrote to the same `Arc`);
/// an outer capture, if any, is uncovered and resumes catching subsequent output.
pub fn take_capture() -> Option<String> {
    let arc = CURRENT.with(|c| c.borrow_mut().as_mut().and_then(|ctx| ctx.capture.pop()));
    arc.map(|a| std::mem::take(&mut *crate::core::sync::lock(&a)))
}

/// If the current process has an active capture, append `s` to the **top** buffer
/// and return `true`; otherwise `false` (output goes to real stdout). The fast path
/// — no capture — is a thread-local borrow + a `Vec::last` check; the `print` hot
/// path pays no lock unless capturing.
pub fn capture_append(s: &str) -> bool {
    CURRENT.with(|c| match c.borrow().as_ref().and_then(|ctx| ctx.capture.last()) {
        Some(arc) => {
            crate::core::sync::lock(arc).push_str(s);
            true
        }
        None => false,
    })
}
