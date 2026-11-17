//! Native-stack growth for the recursive walkers — the one door to `stacker`.
//!
//! A walker over a value, a form or a type recurses once per nesting level, and what
//! it walks can be nested past any fixed stack (a runtime-built value, a macro
//! expansion). Each recursion step calls [`maybe_grow`]: inside the red zone it
//! continues on a fresh heap-backed segment instead of overflowing.
//!
//! The red zone is a promise that the next step's frames fit in it, and that promise
//! is about FRAME SIZES, which the build decides. Under AddressSanitizer every local
//! gets its own slot plus redzones, so frames grow several-fold: the checker's
//! `seq_aware_call_ty` is 4 KiB in a release build and 94 KiB under ASan, past the
//! 64 KiB red zone its call site asks for. So the ASan build raises every red zone and
//! segment to a floor here, once, instead of at sixty call sites. `clippy.toml` refuses
//! a direct `stacker::maybe_grow`, so a new walker cannot bypass the floor.

/// The smallest red zone the ASan build allows: comfortably above the largest
/// instrumented walker frame measured (94 KiB).
#[cfg(brood_asan)]
const ASAN_MIN_RED_ZONE: usize = 1024 * 1024;

/// The smallest segment the ASan build allows — several red zones, so a grow buys
/// real depth rather than one step.
#[cfg(brood_asan)]
const ASAN_MIN_SEGMENT: usize = 8 * 1024 * 1024;

/// Run `f`, first moving to a new `segment`-byte stack if fewer than `red_zone` bytes
/// remain on this one. `red_zone` must exceed the frames one recursion step of the
/// caller builds; the ASan build raises both sizes to its floor.
#[inline]
#[allow(clippy::disallowed_methods)] // the one sanctioned call
pub fn maybe_grow<R>(red_zone: usize, segment: usize, f: impl FnOnce() -> R) -> R {
    #[cfg(brood_asan)]
    let (red_zone, segment) = (
        red_zone.max(ASAN_MIN_RED_ZONE),
        segment.max(ASAN_MIN_SEGMENT),
    );
    stacker::maybe_grow(red_zone, segment, f)
}

// ===== The call stack: as deep as memory allows ===========================================
//
// A process's call stack is memory like its heap, and is bounded the same way: by what the
// runtime may use, not by a frame count (ADR-751). Its parts are the heap frames the engines
// keep (the VM's `BcFrame`s, the tree-walker's continuations, and the operand roots they
// hold) and the native stack a nested activation uses (a `try` body, an `&optional` default,
// a native callback such as `map`'s function, `eval`). The heap frames grow in a `Vec`; the
// native part grows on heap-backed segments ([`reenter`]) instead of failing at the end of
// the thread's stack. Together they may hold up to [`call_stack_ceiling`] bytes; past it the
// recursion fails, catchably, naming what it held.

/// The native stack one growth adds: big enough that a deep callback recursion grows rarely
/// (and, under AddressSanitizer, at least its floor).
#[cfg(all(not(target_arch = "wasm32"), not(brood_asan)))]
const SEGMENT_BYTES: usize = 8 * 1024 * 1024;
#[cfg(all(not(target_arch = "wasm32"), brood_asan))]
const SEGMENT_BYTES: usize = ASAN_MIN_SEGMENT;
#[cfg(target_arch = "wasm32")]
const SEGMENT_BYTES: usize = 1024 * 1024;

/// Grow when less than this much native stack is left: room for the deepest single re-entry
/// chain (`vm_apply` → `vm_run_bc` → `exec_chunk` → a native callback) before the next check.
#[cfg(not(brood_asan))]
pub const NATIVE_RED_ZONE_BYTES: usize = 512 * 1024;
#[cfg(brood_asan)]
pub const NATIVE_RED_ZONE_BYTES: usize = ASAN_MIN_RED_ZONE;

/// How many heap frames the engines push between two ceiling checks: the check is a sum and a
/// compare, and this keeps it off the per-call path.
pub const FRAME_CHECK_INTERVAL: usize = 4096;

thread_local! {
    /// The native stack this thread's current nest has grown onto, in bytes. A nested
    /// activation blocks its worker until it returns (it cannot park across native frames),
    /// so on any thread this belongs to the one process running there.
    static GROWN_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The most one process's call stack may hold, in bytes: a quarter of the memory this
/// runtime may use (the machine, the address-space limit, the container's cap, and the soft
/// memory limit when one is set), and no more than the process's own `:max-heap` when it
/// set one — a process's memory is its heap and its stack.
pub fn call_stack_ceiling(heap: &crate::core::heap::Heap) -> usize {
    let mut usable = crate::core::alloc::machine_memory();
    let soft = crate::core::alloc::soft_limit();
    if soft > 0 {
        usable = usable.min(soft);
    }
    let ceiling = usable / 4;
    match heap.proc_mem_limit() {
        Some(limit) if limit > 0 => ceiling.min(limit),
        _ => ceiling,
    }
}

/// The native stack grown so far on this thread, to add to the heap frames when measuring.
pub fn grown_native_bytes() -> usize {
    GROWN_BYTES.with(|grown| grown.get())
}

/// The error a call stack past its ceiling raises: what it held, in frames and bytes.
pub fn call_stack_error(
    heap: &crate::core::heap::Heap,
    frames: usize,
    bytes: usize,
) -> crate::error::LispError {
    let ceiling = call_stack_ceiling(heap);
    crate::error::LispError::runtime(format!(
        "recursion too deep: the call stack holds {bytes} bytes in {frames} pending calls, \
         past this process's {ceiling}-byte ceiling (a quarter of the memory the runtime may \
         use, or the process's :max-heap)"
    ))
    .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
    .with_hint(
        "a non-tail call holds its frame until it returns and a tail call holds none, so a \
         recursion this deep is usually one that never stops; if it is meant to, make it a \
         tail call (an accumulator) or give the process more memory",
    )
}

/// Check heap frames against the ceiling: `frames` pending calls holding `frame_bytes`, plus
/// this thread's grown native stack. `Err` past the ceiling. Call it every
/// [`FRAME_CHECK_INTERVAL`] frames, not per frame.
pub fn check_frames(
    heap: &crate::core::heap::Heap,
    frames: usize,
    frame_bytes: usize,
) -> Result<(), crate::error::LispError> {
    let bytes = frame_bytes + grown_native_bytes();
    if bytes > call_stack_ceiling(heap) {
        Err(call_stack_error(heap, frames, bytes))
    } else {
        Ok(())
    }
}

/// Run a nested activation — native code re-entering Brood — on this stack while it has
/// room, and on a fresh heap-backed segment when it does not, so recursion through a `try`
/// body or a callback is as deep as memory allows instead of as deep as one thread's stack.
/// The segment counts toward the call stack's ceiling; past it this fails instead of growing.
///
/// The JIT's stack-limit stamp is an address on ONE stack, so it is put back when the
/// segment is left: a limit stamped on the segment means nothing on the stack below it.
pub fn reenter<R>(
    heap: &mut crate::core::heap::Heap,
    run: impl FnOnce(&mut crate::core::heap::Heap) -> Result<R, crate::error::LispError>,
) -> Result<R, crate::error::LispError> {
    match stacker::remaining_stack() {
        Some(left) if left < NATIVE_RED_ZONE_BYTES => {}
        // Room here, or a platform that cannot tell: run in place.
        _ => return run(heap),
    }
    let grown = grown_native_bytes() + SEGMENT_BYTES;
    if grown > call_stack_ceiling(heap) {
        return Err(call_stack_error(heap, 0, grown));
    }
    GROWN_BYTES.with(|bytes| bytes.set(grown));
    let stamped = heap.jit_stack_limit;
    let result = stacker::grow(SEGMENT_BYTES, || run(heap));
    heap.jit_stack_limit = stamped;
    GROWN_BYTES.with(|bytes| bytes.set(grown - SEGMENT_BYTES));
    result
}
