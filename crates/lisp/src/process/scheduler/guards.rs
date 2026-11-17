//! Execution-safety guards for a worker thread — the GC-block / macro-block depth
//! counters and their RAII guards, and the worker stack size (ADR-061, ADR-751). Split out of `scheduler.rs`: these are self-contained
//! (their thread-locals are touched only by the accessors here; `install_ctx`
//! resets them via those accessors) and independent of the reduction/worker
//! scheduling core, which stays in the root. Reaches the parent's imports via
//! `use super::*`, like the other scheduler internals.
use super::*;

thread_local! {
    /// GC-block depth: how many `eval` / `macroexpand_all` frames are active on
    /// this thread. Since ADR-061 this no longer gates the GC safepoint (which now
    /// collects at any eval depth — see `MACRO_BLOCK` and the operand-stack rooting
    /// in `eval::eval`). It fed the stack-overflow byte guard, which ADR-751 removed;
    /// nothing reads it now, and the call sites that say it blocks a collection are
    /// owed an audit (handoff, known follow-ups).
    ///
    /// Per-process: reset to 0 at the start of each quantum (`install_ctx`), so workers
    /// multiplexing several processes don't leak each other's depths. The root
    /// thread doesn't multiplex, so its depth flows naturally.
    static GC_BLOCK: Cell<u32> = const { Cell::new(0) };

    /// Compile-pass depth (ADR-061): bumped by `macroexpand_all`'s
    /// [`MacroBlockGuard`] for the duration of macro expansion. The eval safepoint
    /// collects only when this is **zero** — i.e. never *during* the compile pass,
    /// which (unlike runtime eval) holds partially-built LOCAL forms in unrooted
    /// Rust locals. This is what lets the safepoint otherwise fire at ANY eval
    /// depth (the operand stack roots runtime transients; the compile pass opts
    /// out instead of being rooted). Reset to 0 at the start of each quantum
    /// (`install_ctx`), exactly like `GC_BLOCK`.
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
    if crate::diagnostics::debug_flags::trace_gcblock() {
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
        MACRO_BLOCK.update(|depth| depth + 1);
        MacroBlockGuard
    }
}

impl Drop for MacroBlockGuard {
    #[inline]
    fn drop(&mut self) {
        MACRO_BLOCK.update(|depth| depth.saturating_sub(1));
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
        if crate::diagnostics::debug_flags::trace_gcblock() {
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
        if crate::diagnostics::debug_flags::trace_gcblock() {
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
/// worker thread (ADR-100 §8.4 — no coroutine stack). The engines keep their call frames on
/// the heap, so this stack carries only native code and the nested activations a native
/// re-enters Brood through; near its end such an activation continues on a heap-backed
/// segment (`crate::stack::reenter`, ADR-751), so this size bounds no Brood recursion. The
/// pages are mmap'd lazily, so the tail costs nothing until it is used. The `brood`/`nest`
/// binaries re-home their root thread onto a stack of this same size (see `cli`/`nest`
/// `main`).
///
/// **wasm32 is not a worker:** it runs on a shadow stack in linear memory, ~1 MiB by
/// default. **AddressSanitizer** roughly doubles every Rust frame, so an instrumented
/// prelude boot nearly fills 16 MiB; that build gets a stack that matches the frames it
/// builds (`--cfg brood_asan`, from the `asan` make target).
#[cfg(all(not(target_arch = "wasm32"), brood_asan))]
pub const WORKER_STACK_BYTES: usize = 128 * 1024 * 1024;
#[cfg(all(not(target_arch = "wasm32"), not(brood_asan)))]
pub const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;
#[cfg(target_arch = "wasm32")]
pub const WORKER_STACK_BYTES: usize = 1024 * 1024;
