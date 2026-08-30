//! The Cranelift backend — today's sole implementation of [`JitBackend`].
//!
//! This file owns the *executable memory*: a process-lifetime [`JITModule`] that every
//! compiled arm's code lives in, plus the symbol table that resolves `brood_rt_*` to the
//! Rust functions in [`super::rt`].
//!
//! **The lowering itself is not here.** It lives in `eval/compile/jit_lower*` — the bulk of
//! the backend by volume — because it reads `eval::compile`'s *private* IR internals
//! (`Node`, `Inst`, `Chunk`, `CompiledArm`) through the `use super::*` convention its sibling
//! compile modules share, and so has to be a child of `compile` rather than of `jit`. So the
//! seam runs like this:
//!
//! | module | role |
//! |---|---|
//! | `jit/backend.rs` | the contract — what any backend must satisfy |
//! | `jit/rt.rs` | the ABI — the callback table, backend-independent |
//! | `jit/cranelift.rs` | *this file* — the Cranelift module + the `JitBackend` impl |
//! | `eval/compile/jit_lower*` | the Cranelift lowering, inside `compile` for IR access |
//!
//! A second backend adds a file beside this one and an `impl JitBackend`, with its own
//! lowering wherever it needs to live. What it does *not* re-derive: `rt.rs` (the ABI) and
//! the decisions about whether and how an arm should lower, which are backend-independent.

use super::backend::JitBackend;
use super::rt::*;
use crate::core::value::Symbol;
use crate::eval::compile::{
    arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep, jit_lower_arm, jit_lower_arm_hot,
    jit_lower_inlined_arm, CompiledArm,
};

use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::default_libcall_names;

/// The Cranelift JIT backend (ADR-101, Layer 1). Owns a Cranelift [`JITModule`] — the
/// executable memory + symbol table that every compiled arm's code lives in.
///
/// [`CraneliftBackend::new`] stands up the module for the host ISA and registers the
/// [runtime-callback table](super::rt) by name, so emitted code resolves `brood_rt_*` to
/// those Rust functions; [`CraneliftBackend::module`] is the handle the lowering in
/// `eval/compile/jit_lower*` declares and defines functions through.
///
/// One instance per process, behind the `Mutex` in [`super::GLOBAL_JIT`]: the module owns
/// code that must outlive every installed fn-pointer, and compilation mutates it.
pub struct CraneliftBackend {
    module: JITModule,
}

impl CraneliftBackend {
    /// Stand up the Cranelift JIT module for the host ISA, with the runtime-callback
    /// table registered as absolute symbols (so emitted code resolves `brood_rt_*` to
    /// these Rust functions). No code is compiled here.
    #[allow(clippy::new_without_default)] // construction can fail on an unsupported host
    pub fn new() -> Self {
        // `opt_level=speed` turns on Cranelift's GVN + alias-aware redundant-load
        // elimination, which matters here: a hot loop body re-reads the same frame slot
        // several times (`(< i 1)`, `(- i 1)`, `(+ acc i)` all tag-check + load slot `i`),
        // and the default `opt_level=none` keeps every one of those loads + tag-checks.
        // The extra compile cost is paid on the background compiler thread, off the hot
        // path; the optimizations are semantics-preserving, so the GC discipline is
        // unaffected. Falls back to default flags if the host rejects the setting.
        // The fallback closure yields cranelift's own (large) error in its Err arm,
        // which trips `result_large_err`; it's an external type we don't control and
        // the value is discarded (`|_|`), so the size is irrelevant here.
        // **The CLIF verifier stays ON, deliberately — do not turn it off for the ~3.5%
        // compile-thread saving.** Tried 2026-08-29: `("enable_verifier", "false")` made
        // cranelift-codegen 0.133.1's own `remove_constant_phis` pass fail its internal
        // `assert_eq!(left: 1, right: 0)` on one of `json`'s arms — CLIF that verifies
        // clean — which the tiering layer caught and answered by switching the JIT off for
        // the whole process. Verifier back on, same tree: no panic. So on this Cranelift
        // the optimize pipeline is only exercised-and-sound WITH the verifier in the loop,
        // and the "waste" is buying pipeline behaviour we depend on. Re-attempt only on a
        // Cranelift upgrade, and only with the fuzz differential + every benchmark row's
        // stderr grepped for CODEGEN-PANICKED (the failure mode is a caught panic and a
        // silently interpreter-only process, not a wrong answer).
        #[allow(clippy::result_large_err)]
        let mut builder =
            JITBuilder::with_flags(&[("opt_level", "speed")], default_libcall_names())
                .or_else(|_| JITBuilder::new(default_libcall_names()))
                .expect("Cranelift JITBuilder for the host ISA");
        builder.symbol("brood_rt_tick", brood_rt_tick as *const u8);
        builder.symbol("brood_rt_tick_n", brood_rt_tick_n as *const u8);
        builder.symbol("brood_rt_gc_safepoint", brood_rt_gc_safepoint as *const u8);
        builder.symbol("brood_rt_cons", brood_rt_cons as *const u8);
        builder.symbol("brood_rt_vec2_room", brood_rt_vec2_room as *const u8);
        builder.symbol("brood_rt_make_closure", brood_rt_make_closure as *const u8);
        builder.symbol(
            "brood_rt_make_vector_n",
            brood_rt_make_vector_n as *const u8,
        );
        builder.symbol("brood_rt_car", brood_rt_car as *const u8);
        builder.symbol("brood_rt_cdr", brood_rt_cdr as *const u8);
        builder.symbol("brood_rt_push", brood_rt_push as *const u8);
        builder.symbol("brood_rt_push_room", brood_rt_push_room as *const u8);
        builder.symbol(
            "brood_rt_call_native_fl",
            brood_rt_call_native_fl as *const u8,
        );
        builder.symbol("brood_rt_global", brood_rt_global as *const u8);
        builder.symbol("brood_rt_global_probe", brood_rt_global_probe as *const u8);
        builder.symbol("brood_rt_global_ic", brood_rt_global_ic as *const u8);
        builder.symbol("brood_rt_call_slow", brood_rt_call_slow as *const u8);
        builder.symbol("brood_rt_note_deopt", brood_rt_note_deopt as *const u8);
        builder.symbol(
            "brood_rt_fastlink_base",
            brood_rt_fastlink_base as *const u8,
        );
        builder.symbol("brood_rt_fast_frame", brood_rt_fast_frame as *const u8);
        builder.symbol("brood_rt_xcall_latch", brood_rt_xcall_latch as *const u8);
        builder.symbol("brood_rt_xcall_cold", brood_rt_xcall_cold as *const u8);
        builder.symbol("brood_rt_vector_ref", brood_rt_vector_ref as *const u8);
        builder.symbol("brood_rt_table_has", brood_rt_table_has as *const u8);
        builder.symbol("brood_rt_table_get2", brood_rt_table_get2 as *const u8);
        builder.symbol("brood_rt_table_put", brood_rt_table_put as *const u8);
        builder.symbol("brood_rt_vector_base", brood_rt_vector_base as *const u8);
        builder.symbol(
            "brood_rt_table_dense_base",
            brood_rt_table_dense_base as *const u8,
        );
        builder.symbol("brood_rt_global_epoch", brood_rt_global_epoch as *const u8);
        builder.symbol(
            "brood_rt_dispatch_identity",
            brood_rt_dispatch_identity as *const u8,
        );
        builder.symbol("brood_rt_map_get", brood_rt_map_get as *const u8);
        builder.symbol(
            "brood_rt_i64_overflow_ptr",
            brood_rt_i64_overflow_ptr as *const u8,
        );
        builder.symbol(
            "brood_rt_global_epoch_ptr",
            brood_rt_global_epoch_ptr as *const u8,
        );
        #[cfg(debug_assertions)]
        builder.symbol(
            "brood_rt_dbg_set_staging",
            brood_rt_dbg_set_staging as *const u8,
        );
        #[cfg(debug_assertions)]
        builder.symbol(
            "brood_rt_dbg_check_slot",
            brood_rt_dbg_check_slot as *const u8,
        );
        builder.symbol("brood_rt_in_capture", brood_rt_in_capture as *const u8);
        builder.symbol("brood_rt_roots_base", brood_rt_roots_base as *const u8);
        builder.symbol("brood_rt_i64_throw", brood_rt_i64_throw as *const u8);
        builder.symbol(
            "brood_rt_pair_nursery_base",
            brood_rt_pair_nursery_base as *const u8,
        );
        builder.symbol(
            "brood_rt_pair_old_base",
            brood_rt_pair_old_base as *const u8,
        );
        builder.symbol(
            "brood_rt_vec_nursery_base",
            brood_rt_vec_nursery_base as *const u8,
        );
        builder.symbol("brood_rt_vec_old_base", brood_rt_vec_old_base as *const u8);
        builder.symbol("brood_rt_const_load", brood_rt_const_load as *const u8);
        // DEBUG (bug #2): print the callback addresses once, so an offline disasm of a
        // BROOD_DUMP_CODE'd arm can resolve each `movabs/call` target to a name.
        #[cfg(debug_assertions)]
        if std::env::var_os("BROOD_DUMP_CODE").is_some() {
            for (n, a) in [
                ("roots_base", brood_rt_roots_base as *const () as usize),
                ("call_slow", brood_rt_call_slow as *const () as usize),
                ("fast_frame", brood_rt_fast_frame as *const () as usize),
                ("xcall_latch", brood_rt_xcall_latch as *const () as usize),
                ("xcall_cold", brood_rt_xcall_cold as *const () as usize),
                (
                    "fastlink_base",
                    brood_rt_fastlink_base as *const () as usize,
                ),
                ("push", brood_rt_push as *const () as usize),
                ("car", brood_rt_car as *const () as usize),
                ("cdr", brood_rt_cdr as *const () as usize),
                ("cons", brood_rt_cons as *const () as usize),
                ("global", brood_rt_global as *const () as usize),
                ("global_probe", brood_rt_global_probe as *const () as usize),
                ("global_ic", brood_rt_global_ic as *const () as usize),
                ("const_load", brood_rt_const_load as *const () as usize),
                ("vector_ref", brood_rt_vector_ref as *const () as usize),
                ("vector_base", brood_rt_vector_base as *const () as usize),
                ("tick", brood_rt_tick as *const () as usize),
                ("gc_safepoint", brood_rt_gc_safepoint as *const () as usize),
                ("vec2_room", brood_rt_vec2_room as *const () as usize),
                ("make_closure", brood_rt_make_closure as *const () as usize),
                (
                    "global_epoch_ptr",
                    brood_rt_global_epoch_ptr as *const () as usize,
                ),
                (
                    "dbg_set_staging",
                    brood_rt_dbg_set_staging as *const () as usize,
                ),
            ] {
                eprintln!("[rt-addr] {n} = {a:#x}");
            }
        }
        CraneliftBackend {
            module: JITModule::new(builder),
        }
    }

    /// The Cranelift module to declare + define compiled arms through (Stage 1).
    pub fn module(&mut self) -> &mut JITModule {
        &mut self.module
    }

    /// Compile a trivial `extern "C" fn(heap: *mut Heap) -> i64` that ignores its arg
    /// and returns `n`, finalize it, and return the executable function pointer. The
    /// Stage-1 codegen pipeline smoke test (`docs/jit-stage1.md` §1a): it exercises the
    /// whole path — IR build → `define_function` → `finalize_definitions` →
    /// `get_finalized_function` — with no asm and no heap access. The returned pointer
    /// stays valid as long as `self` (the module owns the executable memory).
    ///
    /// Test-only: the real lowering path is `eval/compile/jit_lower*`, and this exists
    /// solely so a broken codegen pipeline fails as a small clear test rather than as a
    /// mysterious arm that won't lower.
    #[cfg(test)]
    pub fn compile_return_const(&mut self, n: i64) -> *const u8 {
        use cranelift_codegen::ir::{types, AbiParam, InstBuilder, UserFuncName};
        use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
        use cranelift_module::{Linkage, Module};

        let ptr = self.module.target_config().pointer_type();
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(ptr)); // heap: *mut Heap (unused here)
        sig.returns.push(AbiParam::new(types::I64));
        let id = self
            .module
            .declare_function("brood_jit_smoke", Linkage::Export, &sig)
            .expect("declare smoke fn");

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        ctx.func.name = UserFuncName::user(0, id.as_u32());
        {
            let mut fbctx = FunctionBuilderContext::new();
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
            let block = b.create_block();
            b.append_block_params_for_function_params(block);
            b.switch_to_block(block);
            b.seal_block(block);
            let v = b.ins().iconst(types::I64, n);
            b.ins().return_(&[v]);
            b.finalize();
        }
        self.module
            .define_function(id, &mut ctx)
            .expect("define smoke fn");
        self.module.clear_context(&mut ctx);
        self.module
            .finalize_definitions()
            .expect("finalize smoke fn");
        self.module.get_finalized_function(id)
    }
}

/// The whole seam, in one place: the tiering glue in `eval/compile/jit_runtime.rs` reaches
/// codegen only through these two methods, and each is a straight delegation to the
/// Cranelift lowering. Nothing decides anything here — the decisions live above
/// (`jit_lower_arm`'s pre-bail + profitability gate today; see `docs/backend-seams.md` §3).
impl JitBackend for CraneliftBackend {
    fn lower_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_arm(self, arm, slot_tags)
    }

    fn lower_arm_hot(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_arm_hot(self, arm, slot_tags)
    }

    fn lower_inlined_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_inlined_arm(self, arm, slot_tags)
    }

    /// No, once this function has been demoted off the unboxed-scalar register worker: the
    /// published pointer is (or may be) that worker's wrapper, and adopting it walks straight
    /// back into the depth bail the demotion exists to escape.
    fn may_adopt_shared_code(arm: &CompiledArm) -> bool {
        !arm_i64_too_deep(arm)
    }

    /// Yes for a scalar-register arm: its small native *is* the register worker, which already
    /// recurses to full depth unboxed, so the boxed depth-2 upgrade would only swap in inferior
    /// code. (`arm_i64_eligible` already accounts for a prior depth bail.)
    fn declines_inline_upgrade(arm: &CompiledArm) -> bool {
        arm_i64_eligible(arm)
    }

    /// Switch this function permanently to the boxed path, which drains deep recursion via
    /// `jit_native_depth`/`jit_force_vm`.
    fn note_depth_bail(name: Symbol) {
        i64_mark_too_deep(name);
    }
}

#[cfg(test)]
mod smoke {
    use super::*;
    use crate::core::heap::Heap;

    /// End-to-end: Cranelift compiles a constant-returning function, we finalize it and
    /// call the resulting pointer. Validates the codegen pipeline (build + JITModule +
    /// fn-pointer call) before any real arm lowering. No asm, no heap access.
    #[test]
    fn jit_compiles_and_runs_a_constant_fn() {
        let mut jit = CraneliftBackend::new();
        let ptr = jit.compile_return_const(42);
        let f: extern "C" fn(*mut Heap) -> i64 = unsafe { std::mem::transmute(ptr) };
        assert_eq!(f(std::ptr::null_mut()), 42);
        // `jit` (and its module-owned executable memory) stays alive through the call.
    }
}
