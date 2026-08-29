//! The backend contract — what any JIT backend must satisfy.
//!
//! Cranelift was always confined in practice (`docs/backend-seams.md` §1: 7 files, 128
//! references, with `eval/compile/ir.rs` entirely Cranelift-free), so the IR has been the
//! real seam for a long time. What was missing is *this*: the contract expressed as
//! something the compiler checks, instead of prose spread across `docs/jit-tier2.md` and
//! `docs/jit-optimizing-tier.md` and enforced only by tests passing.
//!
//! # The six obligations
//!
//! 1. **Input** — a [`CompiledArm`] plus `slot_tags: &[u8]`, the tier-time slot type
//!    profile. The backend does not decide *whether* to lower: the subset rule, the
//!    profitability gate, frame/checkpoint layout, and unboxing eligibility are all
//!    backend-independent decisions made above it (`eval/compile/jit_lower.rs` today; see
//!    `docs/backend-seams.md` §3 for the move that hoists them out).
//! 2. **Output** — [`JitArmFn`], i.e.
//!    `extern "C" fn(heap: *mut Heap, base: i64, out: *mut Value) -> i64`. The compiled fn
//!    reads its frame slots from `roots[base..]`, computes in registers, and on Done
//!    **writes its result through `out`** — the caller's slot, not `roots[base]`. Returning
//!    `None` from [`JitBackend::lower_arm`] means "out of my subset, keep running the VM" —
//!    never a failure.
//! 3. **Outcome codes** — the returned `i64` is `0` Done, `1` deopt, `2` preempt, `3` error
//!    (parked on the heap for `jit_take_error`), `4` tail (the callee tail-called; its
//!    `[callee, args…]` are staged in `roots` above the frame), and `5` **depth bail** — the
//!    code ran out of native stack and cannot drain mid-recursion, so the tiering layer must
//!    demote this function permanently rather than re-tier it (see
//!    [`JitBackend::note_depth_bail`]). A backend that never emits register-recursion never
//!    returns `5`. Callers branch on exactly these.
//! 4. **The callback table** — [`super::rt`]'s `brood_rt_*` functions are the *only* legal
//!    way to touch the heap, the GC, the globals, or the scheduler. A backend that reaches
//!    around them is unsound, not merely unidiomatic.
//! 5. **Roots-only value discipline** — a `Value` is a 16-byte enum and never rides in a
//!    register (`docs/value-repr.md`). Live `Value`s stay in `Heap::roots`; only *unboxed*
//!    `i64`/`f64` may sit in registers, and only within a safepoint-free segment. Since a
//!    safepoint can occur only inside an obligation-4 callback, that segment needs no stack
//!    map — which is precisely the hardest part of JIT-ing under a moving collector, and
//!    the reason this discipline is non-negotiable rather than a style choice.
//! 6. **Epoch guard and sentinels** — `CompiledArm::jit_code` is null (untried),
//!    [`super::BAILED`], [`super::QUEUED`], or a real 8-aligned code pointer, and native
//!    code is valid only at the `compile_epoch` it was produced at. Plus the deopt journal
//!    and resume-arm protocol (ADR-210): a deopt must resume in the chunk that wrote the
//!    journal.
//!
//! # The tiering advisories
//!
//! Separate from the six obligations, and *not* about emitted code: three questions the
//! **tiering** layer (`eval/compile/jit_runtime.rs`) has to ask a backend, because the answers
//! depend on which strategy that backend chose for an arm. They exist because the tiering glue
//! was reaching around this trait and calling into the Cranelift backend's unboxed-scalar
//! submodule directly — four calls that a second backend would have found meaningless.
//!
//! All three are **associated functions, deliberately not `&self`**: tiering consults them on
//! the per-activation path, and `&self` would mean locking [`super::GLOBAL_JIT`] there. That
//! lock is otherwise uncontended precisely because only the background compiler takes it, and
//! taking it per activation would be a real regression. Each has a default, so a backend with
//! no special strategies implements nothing.
//!
//! # Why this costs nothing
//!
//! A backend's entire output is a `*const u8`; everything downstream of that crosses the
//! obligation-2/3 ABI, which no trait is involved in. [`JitBackend::lower_arm`] runs once
//! per arm, on the background compiler thread, behind a `Mutex`; the advisories are static
//! calls to pure predicates. [`super::ActiveBackend`] is a `#[cfg]`-selected concrete type, so
//! every call here is static and monomorphic.
//!
//! The associated functions make this trait **not object-safe**, which is deliberate: a
//! `dyn JitBackend` would force the advisories back onto `&self` and reintroduce the lock. If
//! runtime (rather than build-time) backend selection is ever wanted, split the advisories into
//! their own static-only trait rather than making these methods.

use crate::core::value::Symbol;
use crate::eval::compile::CompiledArm;

/// A JIT backend: turns a decided-upon [`CompiledArm`] into native code satisfying the six
/// obligations in the [module docs](self).
///
/// Implemented once today, by [`super::cranelift::CraneliftBackend`]. The point of the trait
/// is not polymorphism — it is that a second backend has a compile-checked target and,
/// through the shared `rt` table and the shared gates
/// (`scripts/jit-lower-witness.sh`, `tests/jit.rs`), a conformance suite it inherits rather
/// than reinvents.
/// Deliberately *only* the two lowering entry points: no `name()`, no capability query, no
/// configuration hook. Each of those is a knob with no caller today, and per ADR-011 an
/// additive feature costs nothing to defer — whoever adds a second backend adds the ones
/// that turn out to be needed, with their actual uses.
/// The ABI of a JIT-compiled arm: `(heap, base, out) -> outcome`.
///
/// **This alias exists because the compiler cannot check this ABI.** Every caller reaches a
/// lowered arm through `mem::transmute` of a raw code pointer, so a signature that drifts
/// from what the backend emitted is not a type error — it is silent UB (an argument read
/// from a register nobody set, i.e. a store through a garbage pointer). Naming the type once
/// makes the arity and the parameter kinds a single-site fact.
///
/// `out` is where the Done result goes. It is deliberately *not* `roots[base]`: returning
/// through the roots stack meant the caller loaded back what the callee had just stored, and
/// `perf` (precise events) put that one `movups` at 16.4% of `jit_run_fast_link`. The caller
/// passes the slot it actually wants the value in, so the value is written once. `out` is
/// only written on outcome 0; every other outcome leaves it untouched.
///
/// **GC:** `out` is *not* a root. Nothing may allocate between the callee's store and the
/// consumer taking the value — the same discipline the `brood_rt_{cons,car,cdr}` out-pointer
/// ABI already runs under (`emit::call_handle`), and the outcome-0 path does no allocation.
pub(crate) type JitArmFn =
    extern "C" fn(*mut crate::core::heap::Heap, i64, *mut crate::core::value::Value) -> i64;

pub(crate) trait JitBackend {
    /// Lower `arm` to native code, or `None` to bail to the VM (obligations 1–3).
    fn lower_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8>;

    /// Lower `arm`'s **inlined** variant — the deferred second body two-stage tiering
    /// installs over the small one (self-inlining, leaf splicing, ADR-210). `None` when the
    /// spliced body falls out of the subset, or when the stored derivation's epoch no longer
    /// matches (obligation 6): the small native keeps running, which is always correct.
    fn lower_inlined_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8>;

    /// May this arm adopt native code **published by a peer process** of the same runtime
    /// (ADR-175/215's shared code)? Default yes.
    ///
    /// A backend says no when it has since demoted this function off the strategy that
    /// published code was compiled under — adopting it would reinstall exactly the code the
    /// demotion was meant to escape. Consulted per activation on the shared-lookup path.
    fn may_adopt_shared_code(_arm: &CompiledArm) -> bool {
        true
    }

    /// Should tiering **skip** the deferred inlined upgrade for this arm? Default no.
    ///
    /// Yes when the backend's small native is already the better code and the boxed depth-2
    /// upgrade would only swap in something inferior — for Cranelift, an arm running the
    /// unboxed-scalar register worker, which already recurses to full depth in registers.
    fn declines_inline_upgrade(_arm: &CompiledArm) -> bool {
        false
    }

    /// Record that a function's native code returned **outcome 5** (depth bail): it ran out of
    /// native stack and register recursion cannot drain to the VM mid-stack. Default no-op.
    ///
    /// The backend is expected to stop using that strategy for `name` from here on, so the
    /// re-tier that follows produces drainable code instead. Without it, a deep non-tail
    /// recursion deopts and re-tiers once per level — measured ~100× thrash.
    fn note_depth_bail(_name: Symbol) {}
}
