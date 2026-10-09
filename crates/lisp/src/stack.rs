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
//! 64 KiB red zone its call site asked for, and the nightly ASan job overflowed in the
//! middle of a segment from 2026-10-01 on. So the ASan build raises every red zone and
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
