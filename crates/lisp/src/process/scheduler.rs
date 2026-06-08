//! Green-process scheduler: the state-capture driver, the shared run queue,
//! the worker pool, and the public `spawn` / `self` / `pid-value` /
//! `spawn-count` / `peak-threads` / `set-max-parallel` surface.
//!
//! Each green process runs its 0-arg body's bytecode directly on a worker
//! thread (ADR-100 §8.4 — corosensei removed). `receive` on an empty mailbox
//! **captures** the process's continuation as relocatable heap data
//! (`Suspended`) and returns the worker to the pool, so a small pool of worker
//! OS threads (≈ `nproc`) multiplexes many processes — and a captured process,
//! carrying no native stack, may resume on *any* worker (live migration, §7).
//! The root thread (REPL / file runner) instead blocks on its mailbox condvar
//! (see [`super::mailbox::wait_for_message`]).
//!
//! ## Thread-locals
//! - [`CURRENT`] — the running process's [`Ctx`] (`pid`, `mailbox`, capture
//!   stack). Installed by `run_one` at the start of each quantum and read back
//!   after, so `(self)` / `receive` find their process even after the worker
//!   has run others, and survive migration to another worker.
//! - [`REDUCTIONS`] — countdown to the next preempt; [`tick`] decrements
//!   it from inside `eval`'s loop.
//! - [`GC_BLOCK`] — eval/macroexpand nesting depth; feeds the stack-overflow
//!   byte guard (no longer the GC safepoint — ADR-061). [`MACRO_BLOCK`] —
//!   compile-pass depth; the GC safepoint suppresses collection while it's
//!   nonzero. Both reset per quantum (each quantum runs on a fresh worker
//!   stack), so workers multiplexing several processes don't leak depths.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::process::keywords as pk;
use crate::error::LispError;

use super::mailbox::{wake_parked, Mailbox, REGISTRY, ST_RUNNABLE, ST_RUNNING};
use super::message::Message;
use super::links;
use super::monitor;

/// A green process (ADR-100 §8.4 — state capture, corosensei removed): its own `Heap`,
/// the 0-arg body thunk, and its parked/preempted continuation. The worker drives the
/// body's bytecode (`vm_run_bc`) directly — no coroutine — so a paused process is
/// **relocatable heap data** (`Suspended`), genuinely `Send`, and may resume on any
/// worker (live migration, §7). It is owned by exactly one worker at any instant: the
/// queue/waiter handoff serialises ownership (INV-2).
pub(super) struct Process {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
    /// The worker currently owning this process. Re-assigned on a wake (`wake_enqueue`)
    /// or steal — safe because a process has no native stack to migrate (§7).
    pub(super) worker_id: usize,
    /// This process's LOCAL data heap — travels with it across workers.
    heap: Heap,
    /// The 0-arg body thunk (a shared-runtime `Fn` handle, valid in `heap`).
    body: Value,
    /// The parked/preempted VM continuation, or `None` if not yet started.
    resume: Option<Box<crate::eval::compile::Suspended>>,
    /// The output-capture stack snapshot (the process carries it — no coroutine holds a
    /// `Ctx`). `run_one` installs it into `CURRENT` per quantum and reads it back after,
    /// so `begin_capture`/`take_capture` persist across `receive` suspends.
    capture: Vec<Arc<Mutex<String>>>,
}

/// What a running process needs to find from deep inside `eval` (for
/// `receive`/`self`). Stored in a thread-local, installed by `run_one` at the start of
/// each quantum and read back after (so it survives the worker multiplexing other
/// processes, and migration to another worker).
#[derive(Clone)]
pub(super) struct Ctx {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
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
    /// Per-process: reset to 0 at the start of each quantum (`install_ctx`), so workers
    /// multiplexing several processes don't leak each other's depths. The root
    /// thread doesn't multiplex, so its depth flows naturally.
    static GC_BLOCK: Cell<u32> = const { Cell::new(0) };

    /// Stack-pointer base for the [`stack_overflow_check`] byte guard: the sp of
    /// the *outermost* eval in this quantum. `0` = unset (established by the next
    /// eval). Reset to 0 at the start of each quantum (`install_ctx`): a captured
    /// process resumes on a fresh worker stack, so the base is re-established by its
    /// first eval rather than carried across the suspend.
    static STACK_BASE: Cell<usize> = const { Cell::new(0) };

    /// Compile-pass depth (ADR-061): bumped by `macroexpand_all`'s
    /// [`MacroBlockGuard`] for the duration of macro expansion. The eval safepoint
    /// collects only when this is **zero** — i.e. never *during* the compile pass,
    /// which (unlike runtime eval) holds partially-built LOCAL forms in unrooted
    /// Rust locals. This is what lets the safepoint otherwise fire at ANY eval
    /// depth (the operand stack roots runtime transients; the compile pass opts
    /// out instead of being rooted). Reset to 0 at the start of each quantum
    /// (`install_ctx`), exactly like `GC_BLOCK`/`STACK_BASE`.
    static MACRO_BLOCK: Cell<u32> = const { Cell::new(0) };
}

/// Current GC-block depth — feeds the stack-overflow byte guard's base
/// (`gc_block_depth() <= 1` = outermost eval). No longer gates the GC safepoint
/// (ADR-061); see `MACRO_BLOCK`.
#[inline]
pub fn gc_block_depth() -> u32 {
    GC_BLOCK.with(|d| d.get())
}

/// Write the GC-block depth — reset to 0 per quantum by `install_ctx` (each quantum
/// runs on a fresh worker stack, so the depth is re-established by its first eval).
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

/// Write the compile-pass depth — reset to 0 per quantum by `install_ctx`.
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

/// Stack size for each worker thread. A green process runs its body directly on the
/// worker thread (ADR-100 §8.4 — no coroutine stack), and the tree-walking eval recurses
/// one Rust frame per combination, so a debug-build evaluator running the in-language test
/// suite (which spawns processes that load many test files) needs a deep stack.
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
/// the budget below is uniform and safe on both the root thread and workers.
/// Tunable; bump if a feature lands with heavier frames.
pub const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Stack-budget guard against runaway *non-tail* recursion (ADR-043). The
/// evaluator is a native tree-walker: every nested `eval`/`macroexpand` frame
/// (i.e. every level of non-tail recursion) consumes real Rust stack, and an
/// unbounded one — `(defn boom (n) (+ 1 (boom (+ n 1))))` — would overflow the
/// [`WORKER_STACK_BYTES`] worker stack as a **`SIGSEGV` the host can't
/// `catch_unwind`**, taking down the whole REPL / `nest mcp` server. The guard
/// turns that into a clean, catchable [`STACK_DEPTH_EXCEEDED`] error.
///
/// We measure **stack bytes used**, not frame *count*. Frame count (the old
/// `GC_BLOCK`-ceiling approach) can't work: a heavy frame (`(+ 1 (boom …))`)
/// and a light one (`{:next (f …)}`) differ several-fold in bytes, so any single
/// frame-count ceiling is simultaneously too low for legitimate deep recursion
/// and too high to stop a heavy runaway before the real overflow. Bytes are the
/// thing the stack actually runs out of, so a byte budget is both safe and
/// permissive. See [`STACK_BASE`] for how the per-quantum base is tracked.
///
/// Default: [`WORKER_STACK_BYTES`] minus a margin generous enough to absorb the
/// frame we're in plus the error-construction path (`format!` + `LispError`)
/// without itself overflowing. Override with `BROOD_STACK_BUDGET=<size>`
/// (e.g. `6M`); `0` or malformed falls back to the default.
const STACK_BUDGET_MARGIN: usize = 4 * 1024 * 1024;

/// The active stack budget in bytes, read once from `BROOD_STACK_BUDGET` (or
/// derived from [`WORKER_STACK_BYTES`]). Cached so the per-`eval` check is a load
/// + compare on the hot path.
pub fn stack_budget() -> usize {
    use std::sync::LazyLock;
    static BUDGET: LazyLock<usize> = LazyLock::new(|| {
        std::env::var("BROOD_STACK_BUDGET")
            .ok()
            .and_then(|s| crate::core::alloc::parse_size(&s))
            .filter(|&n| n > 0)
            .unwrap_or(WORKER_STACK_BYTES.saturating_sub(STACK_BUDGET_MARGIN))
    });
    *BUDGET
}

/// `Some(used_bytes)` when the current stack usage has crossed [`stack_budget`],
/// else `None`. `sp` is the caller's stack-pointer probe (the address of a local
/// in the `eval` frame); the per-quantum base ([`STACK_BASE`]) is the sp of the
/// *outermost* eval in this quantum. Stack grows down, so `base - sp` is the
/// bytes consumed by the nested-eval recursion since the outermost frame.
///
/// Self-healing: the base is recorded the first time it's seen unset (`0`) and
/// reset to `0` at the start of each quantum (`install_ctx`), so a worker
/// multiplexing processes never compares against another process's base. As a
/// final backstop, an implausibly large `used` (> a whole stack — impossible
/// within one quantum) is treated as a stale base from a missed switch and
/// silently rebased rather than firing a false positive.
#[inline]
pub fn stack_overflow_check(sp: usize) -> Option<usize> {
    // Called from `eval` *after* its `GcBlockGuard` increment, so `gc_block_depth`
    // is this frame's depth (1 = the outermost eval in this quantum/thread).
    STACK_BASE.with(|b| {
        if gc_block_depth() <= 1 {
            // Outermost eval frame — (re)establish the base *here*, every time.
            // This is what keeps the root thread honest: the base set during
            // prelude load would otherwise be stale by the time a user form runs.
            // Re-stamping at every depth-1 entry fixes that, and is harmless on a
            // worker (its first eval each quantum is depth 1 anyway).
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
        if used > WORKER_STACK_BYTES {
            // Larger than any single worker stack: the base must be stale (a
            // suspend/resume path we didn't account for). Rebase rather than
            // reject a legitimate program.
            //
            // Acknowledged window: this treats "used > a whole stack" as *always* a
            // stale base, so a genuine runaway that somehow overshot a full stack
            // between two depth-1 re-stamps would rebase here instead of raising the
            // clean `STACK_DEPTH_EXCEEDED`. In practice it can't reach this branch:
            // `stack_budget()` (default `WORKER_STACK_BYTES − 4 MiB`) is *below*
            // `WORKER_STACK_BYTES`, and the tree-walker grows the stack one combination
            // frame at a time, so a real runaway trips the `used > stack_budget()`
            // check below — firing the clean error — well before `used` could exceed
            // a full stack. The overshoot would need a single eval step to jump from
            // under-budget to over-a-whole-stack, which the per-frame growth rules
            // out. If frames ever get heavy enough to leap >4 MiB in one step, narrow
            // this (e.g. count consecutive rebases) rather than widen the margin.
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

/// Write the stack base — reset to 0 per quantum by `install_ctx` so this quantum's
/// first eval establishes a fresh base on the worker stack (the byte-guard reference).
#[inline]
pub(super) fn stack_base_set(n: usize) {
    STACK_BASE.with(|b| b.set(n));
}

/// Called once per `eval` `'tail:` iteration. Cheap: a thread-local decrement; only
/// when the budget is exhausted does it touch `CURRENT`. The top-level VM driver does
/// its own capture-mode preemption (`tick_capture`); this `tick`/`preempt` path is the
/// fallback for non-driver runs (nested native callbacks, the tree-walker, the root
/// thread), which can't suspend, so it just refreshes the budget.
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

/// Reduction budget exhausted at a `tick()` site. With no coroutine, there's nothing to
/// yield to here: the **top-level** VM driver does capture-mode preemption itself
/// (`tick_capture` → capture the continuation + re-enqueue). This path is reached only by
/// runs that are *not* the body driver — a NESTED native callback's `vm_apply`, the
/// tree-walker, the root thread, the prelude build — so it just accumulates the quantum's
/// reductions into `process-info`'s `:reductions` (if a process ctx exists) and refreshes
/// the budget so the caller keeps running. (A long native callback thus runs as a "dirty"
/// section — not preempted mid-call — the §7.4 carve-out.)
fn preempt() {
    if let Some(c) = CURRENT.with(|c| c.borrow().clone()) {
        c.mailbox
            .reductions
            .fetch_add(REDUCTION_BUDGET as u64, Ordering::Relaxed);
    }
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
}

thread_local! {
    /// True while this worker is driving a green process body (`run_one`). The
    /// discriminator the `receive` path and the VM driver use to decide "capture the
    /// continuation" vs. "block the root": a green process running here can capture,
    /// while the root thread (which never enters `run_one`) must block on its mailbox.
    /// Set true around the `run_process_body` call, restored after (the worker
    /// multiplexes other processes between quanta).
    static CAPTURE_RUN: Cell<bool> = const { Cell::new(false) };
}

/// Are we inside a capture-mode green-process body run (ADR-100 §8)? The `receive`
/// suspend gate and the VM driver's loop-top preempt/kill capture both key off this.
pub(crate) fn in_capture_run() -> bool {
    CAPTURE_RUN.with(|c| c.get())
}

/// Set/clear the capture-run flag around a `run_process_body` call (`run_one`).
/// `pub(crate)` so the JIT's tests can simulate a green-process (preemptible) context.
pub(crate) fn set_capture_run(on: bool) {
    CAPTURE_RUN.with(|c| c.set(on));
}

thread_local! {
    /// This thread's worker id, set once at `worker_loop` entry; `None` off a worker
    /// (the root thread). Lets a worker mark *itself* dirty-blocked when it parks in a
    /// native-nested receive.
    static CURRENT_WORKER: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Per-worker "dirty-blocked" flag (ADR-100 §7.4): set while a worker is parked inside
/// a **native-nested** capture `receive` (the dirty-scheduler carve-out — it blocks the
/// thread, never returning to its run loop). A dirty worker is excluded from
/// `assign_worker` and its movable backlog is re-routed, so no process is stranded on a
/// worker that won't run it. Sized to match `WORKERS`.
static WORKER_DIRTY: LazyLock<Vec<AtomicBool>> =
    LazyLock::new(|| (0..WORKERS.len()).map(|_| AtomicBool::new(false)).collect());

/// Mark the current worker dirty-blocked and re-route its backlog, returning a guard that
/// clears the flag on drop. A no-op off a worker thread (the root, which owns no queue).
/// Called by the native-nested `receive` block (`wait_for_message`).
pub(crate) fn dirty_block() -> DirtyBlockGuard {
    match CURRENT_WORKER.with(|c| c.get()) {
        Some(wid) => {
            WORKER_DIRTY[wid].store(true, Ordering::Relaxed);
            drain_worker_queue(wid);
            DirtyBlockGuard(Some(wid))
        }
        None => DirtyBlockGuard(None),
    }
}

/// Clears the current worker's dirty-blocked flag when the native-nested receive's
/// blocking wait returns.
pub(crate) struct DirtyBlockGuard(Option<usize>);
impl Drop for DirtyBlockGuard {
    fn drop(&mut self) {
        if let Some(wid) = self.0 {
            WORKER_DIRTY[wid].store(false, Ordering::Relaxed);
        }
    }
}

/// Re-route **every** queued process off a dirty worker. A worker stuck in a native-nested
/// block won't return to its run loop, so anything queued behind it is stranded — and
/// since every process is now migratable (no native stack), the simplest correct move is
/// to push them all elsewhere (the mass-kill/monitor deadlock fix). `assign_worker`
/// already excludes `wid`, so they land on live workers.
fn drain_worker_queue(wid: usize) {
    // Every queued process is stranded on this dirty worker (it won't return to its run
    // loop) and is migratable (no native stack), so re-route them all off it.
    let stranded: Vec<Box<Process>> =
        crate::core::sync::lock(&WORKERS[wid].0).drain(..).collect();
    // These left a queue without going through `run_one` (the usual decrement site), so
    // account for the removal here; the `enqueue` below re-adds one each. Net zero — the
    // `STEALABLE` count stays equal to the processes actually sitting in queues.
    STEALABLE.fetch_sub(stranded.len(), Ordering::Relaxed);
    for mut proc in stranded {
        // Force off this (dirty) worker directly — `assign_worker` already excludes a
        // dirty worker, so this lands elsewhere. (Not `wake_enqueue`: its "migrate only
        // when home is busy" heuristic would depend on `WORKER_BUSY[wid]` being set,
        // which it is during a block, but relying on that coupling here is fragile —
        // the whole point is that nothing must stay on a worker that won't run it.)
        proc.worker_id = assign_worker();
        enqueue(proc);
    }
}

thread_local! {
    /// True while the **innermost** `vm_run_bc` is the *top-level* body driver — i.e.
    /// the running `receive` is reached purely through bytecode, with no native frame
    /// (a `%isolate`/`%try`/`map` callback) between it and the driver. A clean
    /// top-level receive can capture its continuation (and migrate); a **native-nested**
    /// receive cannot (the native frame can't be captured, and re-running it repeats
    /// side effects — the §8.1 footgun), so it falls back to **blocking** its worker
    /// like a BEAM dirty scheduler (§7.4). Set per `vm_run_bc` entry to that call's
    /// `top_level` and restored on exit, so it reflects the innermost driver.
    static CAPTURE_TOP_LEVEL: Cell<bool> = const { Cell::new(false) };
}

/// Is the innermost capture-mode VM driver the top-level body driver (so a `receive`
/// here is bytecode-reachable and may capture), as opposed to nested under a native
/// (must block instead)? See [`CAPTURE_TOP_LEVEL`].
pub(crate) fn capture_top_level() -> bool {
    CAPTURE_TOP_LEVEL.with(|c| c.get())
}

/// Set the top-level-driver flag, returning the previous value (so `vm_run_bc` can
/// restore it on exit — nested runs set it `false`, the outer run restores `true`).
pub(crate) fn set_capture_top_level(on: bool) -> bool {
    CAPTURE_TOP_LEVEL.with(|c| c.replace(on))
}

/// Reduction tick for the capture-mode VM driver: like [`tick`] but **returns**
/// whether the budget is exhausted (so the driver captures + yields a `Preempted`).
/// Decrements otherwise. The budget is refreshed by `run_one` at the next resume.
pub(crate) fn tick_capture() -> bool {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            true
        } else {
            r.set(n - 1);
            false
        }
    })
}

/// Is an untrappable hard `:kill` pending for the current process? The driver checks this
/// at a loop-top safepoint and stops. A *soft* exit isn't honoured here — it waits for
/// the next `receive` (checked when `run_one` would park).
pub(crate) fn capture_hard_kill_pending() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|ctx| ctx.mailbox.pending_kill())
            .is_some_and(|r| is_kill_reason(&r))
    })
}

/// Cooperatively yield so other ready work can make progress (`(yield)` / used by
/// `%isolate`'s reap to wait for killed orphans). A process can't free its worker
/// mid-eval (the continuation is only captured at a `receive`), so this hints the OS
/// scheduler (`std::thread::yield_now`) — the other worker threads run their processes,
/// so work the caller is spinning on (e.g. orphans being reaped on other workers) makes
/// progress — and refreshes the reduction budget so the caller isn't immediately
/// preempted on return. Not `std::thread::sleep`: a busy spinner shouldn't add fixed
/// latency per iteration.
pub fn yield_now() {
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    std::thread::yield_now();
}

// ----- the run queue + worker pool -------------------------------------------

pub(super) static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
/// How many processes have been work-stolen across worker threads since program start
/// (read by `(steal-count)`). A diagnostic of how much rebalancing the scheduler actually
/// did — 0 means placement-at-spawn kept the pool even and no thief ever needed to pull
/// work.
static STOLEN: AtomicU64 = AtomicU64::new(0);
/// How many times a process was re-assigned to a *different* worker when woken from a
/// park (`receive`/timer/exit) — i.e. a live migration of a mid-computation continuation
/// across worker threads (ADR-100 §7). Read by the live-migration regression test as
/// direct evidence that captured continuations actually crossed threads.
static MIGRATED: AtomicU64 = AtomicU64::new(0);
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
    /// Set by a process's body just before it returns, so `run_one` can read the
    /// exit reason (for monitor `[:down …]` delivery) once the driver returns on
    /// this same worker thread. Cleared at the start of every scheduling quantum.
    static EXIT_REASON: RefCell<Option<Message>> = const { RefCell::new(None) };
}

/// One worker's run queue + the condvar that parks it when the queue is empty.
type WorkerQueue = (Mutex<VecDeque<Box<Process>>>, Condvar);

/// Per-worker run queues. Index = `worker_id`. A worker drains its own queue
/// first; when empty, it may **steal any queued process** from a backed-up peer
/// (`try_steal`) — every process is migratable now that continuations are captured
/// to the heap (ADR-100 §8.4), so there's no pinning. Preempt re-enqueues to the
/// same worker (keep a hot process local); a wake may migrate (`wake_enqueue`). The
/// Vec is sized once at the first `ensure_workers` from `worker_count()`, then never
/// resized.
static WORKERS: LazyLock<Vec<WorkerQueue>> = LazyLock::new(|| {
    (0..worker_count())
        .map(|_| (Mutex::new(VecDeque::new()), Condvar::new()))
        .collect()
});

/// Count of processes currently sitting in some worker's queue — i.e. the pool of
/// stealable work. Incremented in `enqueue` (every queueing: spawn, wake, preempt)
/// and decremented in `run_one` (the single pulled-to-run site, whether the owner
/// drained it or a thief stole it).
/// A cheap, relaxed atomic gate: an idle worker checks it before scanning peer
/// queues, so a truly-idle pool (`STEALABLE == 0`) re-parks on one atomic load
/// instead of an O(workers) scan. May briefly over-count a process popped but
/// not yet in `run_one` (a wasted scan, self-correcting) — it is a hint, never a
/// correctness gate.
static STEALABLE: AtomicUsize = AtomicUsize::new(0);

/// How long an idle worker parks before re-attempting a steal, when it has no
/// work of its own. A backstop, not the primary wakeup: a worker is woken
/// immediately when a process is enqueued onto *its* queue (a preempt re-enqueue
/// or a spawn placed here), but it is *not* notified when a **peer's** queue
/// grows — so it re-checks for stealable work every `STEAL_BACKOFF`. Short
/// enough that a steal opportunity isn't missed for long; long enough that a
/// genuinely idle pool wakes rarely (each wake is a single `STEALABLE` load when
/// nothing is stealable). Tunable.
const STEAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(10);

/// Per-worker "is currently running a process" flag. Index = `worker_id`, sized
/// to match `WORKERS`. A worker runs at most one process at a time, so this is a
/// 0/1 gauge of in-flight work. `assign_worker` folds it into a worker's load:
/// a worker draining one CPU-bound process has an *empty queue* yet is saturated,
/// and queue length alone would wrongly read it as idle. Set/cleared around the
/// `resume` in `run_one`; read (lock-free) at spawn placement.
static WORKER_BUSY: LazyLock<Vec<AtomicBool>> =
    LazyLock::new(|| (0..WORKERS.len()).map(|_| AtomicBool::new(false)).collect());

/// Rotating start point for `assign_worker`'s least-loaded scan. Read +
/// incremented under relaxed ordering — the only requirement is approximate
/// rotation; an occasional duplicate or skipped index is fine.
static NEXT_WORKER: AtomicUsize = AtomicUsize::new(0);

/// Pick the worker a `Process` should be placed on — at spawn, on a wake migration, or
/// when a thief re-homes a stolen process. **Least-loaded with a rotating start:** scan the
/// queues beginning at a round-robin offset and choose the shortest, breaking
/// ties toward the rotation. When load is even (the common case — most queues
/// empty) this degrades to plain round-robin; when one worker is backed up (a
/// spawn burst, or uneven drain) processes steer to idle workers instead.
/// Queue lengths are sampled via `try_lock`, so a momentarily-contended queue is
/// skipped rather than blocking the spawner. Validated clean (incl. under
/// `BROOD_GC_STRESS`) in the Track-A experiment; replaces pure round-robin.
fn assign_worker() -> usize {
    // `WORKERS.len()`, not `worker_count()`: touching the LazyLock commits the
    // pool size, so the modulus always matches the queues we index — a
    // `set_max_parallel` after the pool starts can no longer skew the count
    // (latent OOB), and the old per-spawn `BROOD_J` env read (+ the global env
    // lock) is gone — `worker_count()` now runs once, at pool init.
    let n = WORKERS.len().max(1);
    // A worker's load is its runnable backlog (queue length) **plus** the process
    // it's currently running, if any: a worker draining one long CPU-bound process
    // has an empty queue but no spare capacity, so queue length alone would wrongly
    // steer a newcomer onto it. (Parked/blocked processes aren't in the queue — they
    // sit in mailbox waiter slots — so the queue already excludes them; the running
    // process is the one thing it misses.) Sampled via `try_lock`, so a momentarily
    // contended queue reads as `MAX` and is skipped rather than blocking the spawner.
    let load = |i: usize| -> usize {
        // A dirty-blocked worker (parked in a native-nested receive — §7.4) won't return
        // to its run loop, so never route work to it (it would be stranded there).
        if WORKER_DIRTY[i].load(Ordering::Relaxed) {
            return usize::MAX;
        }
        match WORKERS[i].0.try_lock() {
            Ok(q) => q
                .len()
                .saturating_add(WORKER_BUSY[i].load(Ordering::Relaxed) as usize),
            Err(_) => usize::MAX,
        }
    };
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    let mut best = start;
    let mut best_len = load(start);
    for off in 1..n {
        if best_len == 0 {
            break; // can't do better than an empty, idle worker
        }
        let i = (start + off) % n;
        let len = load(i);
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

/// Total processes work-stolen across worker threads since program start
/// (read by `(steal-count)`). See [`STOLEN`].
pub fn steal_count() -> u64 {
    STOLEN.load(Ordering::SeqCst)
}

/// Total live migrations of running processes across worker threads since
/// program start (ADR-100 §7). See [`MIGRATED`].
pub fn migrate_count() -> u64 {
    MIGRATED.load(Ordering::SeqCst)
}

/// Set the worker-pool size (0 = default ≈ `nproc`). Call once at startup, before
/// any spawning — once the `WORKERS` pool has initialised the size is committed
/// and this has no further effect (everything indexes by `WORKERS.len()`).
/// (Replaces the old per-spawn thread cap.)
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

/// Resolve the pool size: `BROOD_J` env override, else `set_max_parallel`'s
/// value, else ≈ `nproc`. Called exactly once — at the `WORKERS` LazyLock
/// init — so the env read never lands on the spawn hot path.
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
    // The dead process's own watches: drop entries where *it* was the watcher,
    // or they leak until each watched target dies (kernel audit). Takes
    // MONITORS sequentially like everything in this function — never nested.
    monitor::sweep_dead_watcher(pid);
    // Links (ADR-067), after monitors and with no table lock held: notify every
    // linked peer — a trappable `[:EXIT pid reason]` if it traps, else an abnormal
    // reason propagates as a hard kill that cascades through *its* links. Mirrors
    // the sequential lock discipline above (never holds REGISTRY/MONITORS here).
    links::notify_peers(pid, &reason);
}

/// The untrappable hard-kill reason — Erlang's `exit(pid, kill)`. A `:kill` exit
/// fires at the next reduction tick (`preempt`); any other reason is the soft
/// signal that waits for the next `receive` iteration.
pub(super) fn is_kill_reason(reason: &Message) -> bool {
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
    // `tick` (preempt) or re-enter `receive` on its own. Wake it by re-queueing it —
    // exactly how `send`/the timer wake a parked process — and it self-kills at
    // `receive_match`'s loop-top `kill_pending` check, on whichever worker runs it.
    // Taking the waiter (via the shared `wake_parked` — the same step
    // `deliver`/`wake_for_timeout` use) under the state lock serialises with
    // `run_one`'s park: either we take an already-parked process here, or `run_one`
    // sees `kill_pending` and retires it instead of parking (exactly one wins).
    let parked = wake_parked(&mut crate::core::sync::lock(&mailbox.state));
    if let Some(proc) = parked {
        wake_enqueue(proc); // a wake (to deliver the kill) — may migrate the process
    }
}

/// Push a ready process onto its owning worker's queue and wake that worker.
/// Preempt re-enqueue routes here so a hot process stays on its worker (cache
/// locality); a *woken-from-park* process may migrate instead — see [`wake_enqueue`].
pub(super) fn enqueue(proc: Box<Process>) {
    let wid = proc.worker_id;
    proc.mailbox.status.store(ST_RUNNABLE, Ordering::Relaxed); // queued, awaiting a worker turn
    // Count it as stealable runnable work (the `try_steal` fast-path hint). Balanced by
    // the single decrement in `run_one` when it's pulled to run (by its owner or a thief).
    STEALABLE.fetch_add(1, Ordering::Relaxed);
    let (lock, cv) = &WORKERS[wid];
    crate::core::sync::lock(lock).push_back(proc);
    cv.notify_one();
}

/// Enqueue a process that is **waking from a park** (a `receive`/timer/exit wake, or a
/// message that raced its park). The **live-migration** point (ADR-100 §7): the process
/// was idle and has no native stack, so it may resume on any worker. Migrate **only when
/// the home worker is busy** — a single atomic load, vs an O(workers) `assign_worker`
/// scan on every wake. If home is idle it runs the woken process right away, so keep it
/// there (cache locality, and no scan on the hot receive/reply path — ~all of the per-wake
/// cost). Migrate (to the least-loaded worker) only when home is busy: re-queueing there
/// would sit behind the running process. This also covers a home worker stuck in a
/// **dirty** block — it reads busy (its `run_one` hasn't returned) and `assign_worker`
/// excludes it, so the woken process is moved off it. (Preempt re-enqueue uses plain
/// [`enqueue`] instead, to keep a hot process local.)
pub(super) fn wake_enqueue(mut proc: Box<Process>) {
    if WORKER_BUSY[proc.worker_id].load(Ordering::Relaxed) {
        let new_wid = assign_worker();
        if new_wid != proc.worker_id {
            MIGRATED.fetch_add(1, Ordering::Relaxed);
        }
        proc.worker_id = new_wid;
    }
    enqueue(proc);
}

/// Steal one queued process from a backed-up peer's queue, re-assigning it to
/// `thief_wid`. Returns `None` if nothing is stealable. Since a process has no native
/// stack (state capture, ADR-100 §8.4), **any** process is safe to resume on the thief —
/// the cross-thread-resume hazard (KI-1b) that once forced fresh-only stealing is gone.
/// The queue handoff (`try_lock`) serialises ownership, so exactly one worker owns it at
/// a time (INV-2).
fn try_steal(thief_wid: usize) -> Option<Box<Process>> {
    // Fast path: nothing queued anywhere — re-park on one relaxed load.
    if STEALABLE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let n = WORKERS.len();
    // Rotating start so thieves spread their probes across victims rather than all
    // hammering worker 0 (shares `NEXT_WORKER` with `assign_worker`; only
    // approximate rotation is needed).
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    for off in 0..n {
        let victim = (start + off) % n;
        if victim == thief_wid {
            continue; // don't steal from ourselves
        }
        // `try_lock`: never block a would-be thief on a contended victim — skip it
        // and try the next. A momentarily-locked queue just isn't probed this pass;
        // the `STEAL_BACKOFF` timeout brings us back.
        let mut q = match WORKERS[victim].0.try_lock() {
            Ok(q) => q,
            Err(_) => continue,
        };
        // Take from the back (the owner pops the front): the process the owner is
        // least likely to run next. `STEALABLE` is only a hint, so an empty queue
        // is normal here.
        if let Some(mut proc) = q.pop_back() {
            drop(q);
            proc.worker_id = thief_wid; // re-assign: the thief owns it from now on
            STOLEN.fetch_add(1, Ordering::Relaxed);
            // `STEALABLE` is decremented by `run_one` (the single pulled-to-run
            // site the caller invokes next), not here — so the count stays balanced
            // whether a process is drained by its owner or stolen.
            return Some(proc);
        }
    }
    None
}

/// Start the worker pool exactly once (on the first `spawn`).
fn ensure_workers() {
    WORKERS_STARTED.call_once(|| {
        // Force the WORKERS LazyLock to initialise *now*, with the pool size
        // committed by the current `set_max_parallel` (or the default ≈ nproc).
        // A later `set_max_parallel` won't resize the pool — sized once.
        let n = WORKERS.len();
        ACTIVE_WORKERS.store(n, Ordering::SeqCst);
        // A process body runs directly on its worker thread (ADR-100 §8.4 — no coroutine
        // stack), and nested native / tree-walked sub-calls recurse here, so the worker
        // stack must be at least `stack_budget`'s reference size (`WORKER_STACK_BYTES`),
        // else a deep native recursion would overflow the default ~2 MiB thread stack
        // *before* the guard trips a clean error. The reservation is virtual/lazy.
        for wid in 0..n {
            let started = std::thread::Builder::new()
                .stack_size(WORKER_STACK_BYTES)
                .spawn(move || worker_loop(wid))
                .is_ok();
            if !started {
                std::thread::spawn(move || worker_loop(wid));
            }
        }
    });
}

fn worker_loop(wid: usize) {
    CURRENT_WORKER.with(|c| c.set(Some(wid)));
    loop {
        // 1. Our own queue first (FIFO).
        //
        //    Bind the pop to a `let` so the queue `MutexGuard` is dropped at the
        //    end of *this statement*, BEFORE `run_one`. In edition 2021 a guard
        //    held in an `if let` scrutinee lives to the end of the whole block, so
        //    `if let Some(p) = lock(..).pop_front() { run_one(p) }` would hold the
        //    queue lock across the run — and the running process's preempt/receive
        //    re-enqueue (which re-locks this same queue) would deadlock the worker.
        let own = crate::core::sync::lock(&WORKERS[wid].0).pop_front();
        if let Some(p) = own {
            run_one(p);
            continue;
        }
        // 2. Nothing of our own: steal any queued process from a backed-up peer
        //    (every process is migratable — no native stack). See `try_steal`.
        if let Some(p) = try_steal(wid) {
            run_one(p);
            continue;
        }
        // 3. Nothing runnable anywhere we can reach: park on our condvar. We're
        //    woken immediately when a process is enqueued onto *our* queue, but
        //    NOT when a peer's queue grows — so park with a `STEAL_BACKOFF`
        //    backstop and re-attempt the steal on timeout. Re-check our own queue
        //    under the lock first to close the enqueue/park lost-wakeup window.
        let (lock, cv) = &WORKERS[wid];
        let q = crate::core::sync::lock(lock);
        if q.is_empty() {
            let _ = cv.wait_timeout(q, STEAL_BACKOFF);
        }
    }
}

/// Resume a process once, then either retire it (it finished) or, if it suspended
/// at `receive`, park it on its mailbox (or re-queue it if a message raced in).
fn run_one(mut proc: Box<Process>) {
    let mailbox = Arc::clone(&proc.mailbox);
    let wid = proc.worker_id;
    // Pulled to run: the single `STEALABLE` decrement site, paired with the increment in
    // `enqueue`, whether its owner drained it or a thief stole it.
    STEALABLE.fetch_sub(1, Ordering::Relaxed);
    mailbox.status.store(ST_RUNNING, Ordering::Relaxed); // about to resume on this worker

    let live = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    PEAK_RUNNING.fetch_max(live, Ordering::SeqCst);
    // Mark this worker busy for `assign_worker`'s load metric while we're inside the run
    // (cleared in `finish_quantum`).
    WORKER_BUSY[wid].store(true, Ordering::Relaxed);
    // Fresh reduction budget for this scheduling quantum (decremented in the VM driver's
    // loop top via `tick_capture`; at zero the process captures + re-enqueues — preempt).
    REDUCTIONS.with(|r| r.set(REDUCTION_BUDGET));
    EXIT_REASON.with(|r| *r.borrow_mut() = None); // stale from a prior process on this worker

    // The worker drives the body's bytecode (`vm_run_bc`) directly — no coroutine — so a
    // paused process's continuation is relocatable heap data (`Suspended`) and can resume
    // on whichever worker `wake_enqueue` routes it to (live migration). No coroutine holds
    // the `Ctx`, so the worker installs it for the quantum (rebuilt each resume — the
    // worker multiplexes processes) and reads any capture-stack changes back afterwards.
    proc.install_ctx();
    set_capture_run(true);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.drive()));
    set_capture_run(false);
    proc.save_ctx();
    finish_quantum(&mailbox, wid);
    handle_capture_outcome(proc, &mailbox, outcome);
}

/// Shared post-quantum bookkeeping: drop the live-process gauge + worker-busy flag and
/// tally the reductions this quantum consumed (budget minus the remainder — a preempted
/// process left 0) into `process-info`'s `:reductions`. The quantum's eval shares this
/// worker's `REDUCTIONS` TLS, so its post-yield value is the remainder (Erlang counts
/// reductions the same way).
fn finish_quantum(mailbox: &Arc<Mailbox>, wid: usize) {
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    WORKER_BUSY[wid].store(false, Ordering::Relaxed);
    let used = REDUCTION_BUDGET.saturating_sub(REDUCTIONS.with(|r| r.get()));
    mailbox.reductions.fetch_add(used as u64, Ordering::Relaxed);
}

/// Handle a quantum's outcome (ADR-100 §8.3): `Done` retires `:normal`, an `Err` retires `[:error …]`,
/// `Killed` retires with the pending kill reason, `Preempted` stores the continuation
/// and re-queues (migrating), `Suspended` stores it and parks on the mailbox.
fn handle_capture_outcome(
    mut proc: Box<Process>,
    mailbox: &Arc<Mailbox>,
    outcome: std::thread::Result<Result<crate::eval::compile::VmOutcome, LispError>>,
) {
    use crate::eval::compile::VmOutcome;
    match outcome {
        Ok(Ok(VmOutcome::Done(_))) => {
            deregister(proc.pid, Message::Keyword(value::intern(pk::NORMAL)));
        }
        Ok(Ok(VmOutcome::Suspended(s))) => {
            // Store the parked continuation in the process, then park (the
            // `receive`-boundary kill check + raced-message recheck).
            // `receive_match` already set `scanned` and armed any timer.
            proc.store_resume(s);
            park_on_receive(proc, mailbox);
        }
        Ok(Ok(VmOutcome::Preempted(s))) => {
            // Budget hit: stash the continuation and re-queue on the **same** worker
            // (`enqueue`, not `wake_enqueue`) — a hot, actively-running process stays
            // put for cache locality; migration is for *idle* (parked) processes.
            proc.store_resume(s);
            enqueue(proc);
        }
        Ok(Ok(VmOutcome::Killed)) => {
            // A hard `:kill` was observed at a loop-top safepoint. Take its reason.
            let reason = crate::core::sync::lock(&mailbox.state)
                .kill
                .take()
                .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
            deregister(proc.pid, reason);
        }
        Ok(Err(e)) => {
            // An uncaught throw/error killed the process (Erlang let-it-crash).
            eprintln!("process {} died: {}", proc_descr(proc.pid), e.located());
            let reason = Message::Vector(vec![
                Message::Keyword(value::intern(pk::ERROR)),
                Message::Str(e.to_string()),
            ]);
            deregister(proc.pid, reason);
        }
        Err(_) => {
            eprintln!("process {} panicked", proc_descr(proc.pid));
            deregister(proc.pid, Message::Keyword(value::intern(pk::KILLED)));
        }
    }
}

/// Park a process that suspended in `receive` (both execution modes). It scanned the
/// first `scanned` messages with no match: re-check under the lock — if a hard `:kill`
/// raced in, die; if a *new* (unscanned) message arrived during the suspend window,
/// re-queue to run again; otherwise park as the mailbox waiter for `send`/the timer to
/// wake. The state lock serialises this with `exit`'s waiter-take, so a process can't
/// end up parked-with-a-pending-kill and stuck forever.
fn park_on_receive(proc: Box<Process>, mailbox: &Arc<Mailbox>) {
    let mut st = crate::core::sync::lock(&mailbox.state);
    if mailbox.kill_pending.load(Ordering::Relaxed) {
        let reason = st
            .kill
            .take()
            .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
        drop(st);
        deregister(proc.pid, reason);
        // `proc` dropped here → its captured continuation + LOCAL heap are freed.
    } else if st.queue.len() > st.scanned {
        // A message raced in during the park — resume instead of parking. This is a
        // wake, so the process may migrate (`wake_enqueue`).
        drop(st);
        wake_enqueue(proc);
    } else {
        st.waiter = Some(proc);
    }
}

impl Process {
    /// Drive the body one quantum: run fresh, or resume the parked continuation
    /// (`resume` is taken). The `&mut self` borrow ends when this returns, so `run_one`
    /// is then free to move/park/re-queue `self` on the outcome.
    fn drive(&mut self) -> Result<crate::eval::compile::VmOutcome, LispError> {
        let resume = self.resume.take().map(|b| *b);
        crate::eval::compile::run_process_body(&mut self.heap, self.body, resume)
    }

    /// Stash a captured continuation back into the process before it parks or re-queues
    /// (so the next `run_one` resumes from it).
    fn store_resume(&mut self, s: crate::eval::compile::Suspended) {
        self.resume = Some(Box::new(s));
    }

    /// Establish `CURRENT` for this quantum. Resets the per-quantum thread-locals
    /// (GC-block depth, stack base, macro block) to 0: each quantum runs on a fresh
    /// worker stack, so they are re-established here.
    fn install_ctx(&self) {
        let ctx = Ctx {
            pid: self.pid,
            mailbox: Arc::clone(&self.mailbox),
            capture: self.capture.clone(),
        };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(0);
        stack_base_set(0);
        macro_block_set(0);
    }

    /// Read the (possibly mutated) capture stack back out of `CURRENT` into the process
    /// and clear `CURRENT`, so `begin_capture`/`take_capture` done this quantum persist
    /// across the next `receive` suspend, and the worker's TLS doesn't leak this
    /// process's ctx into the next one it runs.
    fn save_ctx(&mut self) {
        if let Some(cap) = CURRENT.with(|c| c.borrow().as_ref().map(|ctx| ctx.capture.clone())) {
            self.capture = cap;
        }
        CURRENT.with(|c| *c.borrow_mut() = None);
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

    // State capture is the only engine now (ADR-100 §8.4 step 4 — corosensei removed):
    // the worker drives `vm_run_bc` directly, so a paused process is relocatable heap
    // data (migratable, no native stack). A VM-eligible body captures + migrates; a body
    // that defers to the tree-walker (vanishingly rare) runs tree-walked on the worker
    // with blocking `receive`s (`run_process_body`). `f` is a shared-runtime handle valid
    // in the child heap (same `runtime` Arc).
    let mut child = Heap::with_regions(prelude, runtime);
    child.set_global(EnvId::GLOBAL);

    ensure_workers();
    let worker_id = assign_worker();
    enqueue(Box::new(Process {
        pid,
        mailbox,
        worker_id,
        heap: child,
        body: f,
        resume: None,
        capture: inherited_capture,
    }));
    Ok(pid)
}


/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx().pid
}

/// Are we currently running inside a **green** (spawned) process — as opposed to the
/// *root* thread (the REPL / file runner / MCP dispatcher)? True when [`in_capture_run`]
/// is set (the worker is driving a process body). Used by the eval-time `unbound` raise
/// to attach a scheduler-race hint (the under-load failure mode
/// `docs/claude-demo-findings.md` flagged — concurrent prelude lookups racing).
pub fn in_green_process() -> bool {
    in_capture_run()
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

/// The current process's context. A green process has it installed by `run_one` each
/// quantum; the first time a *root* thread (the REPL / file runner) uses `self`/`receive`,
/// register it as a blocking-mailbox process so it can participate in message passing.
pub(super) fn ensure_ctx() -> Ctx {
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            return ctx.clone();
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let mailbox = Mailbox::new();
        crate::core::sync::lock(&REGISTRY).insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx { pid, mailbox, capture: Vec::new() };
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
