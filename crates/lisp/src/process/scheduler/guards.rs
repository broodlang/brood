//! Execution-safety guards for a worker thread — the GC-block / macro-block depth
//! counters and their RAII guards, and the non-tail-recursion stack-overflow byte
//! guard (ADR-043/061). Split out of `scheduler.rs`: these are self-contained
//! (their thread-locals are touched only by the accessors here; `install_ctx`
//! resets them via those accessors) and independent of the reduction/worker
//! scheduling core, which stays in the root. Reaches the parent's imports via
//! `use super::*`, like the other scheduler internals.
use super::*;

thread_local! {
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
