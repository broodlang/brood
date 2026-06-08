//! JIT (ADR-101) — tier-1 template JIT via Cranelift, behind `--features jit`.
//!
//! **Stage 0 (plumbing only — no codegen yet).** This increment provides the
//! `extern "C"` **runtime-callback table** the JIT'd code will call. Later Stage-0
//! increments add the Cranelift `JITModule` scaffolding (Layer 1) and the platform
//! trampolines (Layer 3, `build.rs` + `.s` files); Stage 1 emits actual code.
//!
//! ## ABI (ADR-101 §6.2, adapted for the kept 16-byte enum `Value`)
//!
//! Brood keeps `Value` as a 16-byte enum — the measured decision in
//! [`docs/value-repr.md`](../../../docs/value-repr.md): a single-word `Value` gave
//! ~zero tier-1 speedup on the compute loops, so NaN-boxing isn't worth its
//! wide-scalar cost. Consequently a `Value` **never rides in a register**. Tier-1
//! JIT'd code keeps all live `Value`s in [`Heap::roots`] (the operand stack, the same
//! one the bytecode VM uses) and only holds *unboxed* `i64`/`f64`, extracted from a
//! root slot, in registers within a safepoint-free segment. So every runtime callback
//! takes the pinned `*mut Heap` context (r15/x28, ADR-101 §6.2) and operates on
//! `roots`/the heap — **not** `Value`-as-`u64` as the original ADR-101 sketch assumed
//! (that sketch presumed the NaN-box repr we declined).
//!
//! A safepoint can occur only inside one of these callbacks (allocation / explicit
//! safepoint / slow call), so between callbacks the JIT'd segment may keep unboxed
//! scalars in registers with no stack map — the single hardest part of JIT-ing under a
//! moving collector, sidestepped (ADR-101 §6.2).

use crate::core::heap::Heap;

use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::default_libcall_names;

/// The JIT backend (ADR-101, Layer 1). Owns a Cranelift [`JITModule`] — the executable
/// memory + symbol table that compiled arms live in.
///
/// **Stage 0: skeleton only.** [`Jit::new`] stands up the module for the host ISA and
/// registers the [runtime-callback table](self) by name, so Stage-1 codegen can emit
/// calls to `brood_rt_*`. No arm is compiled yet; [`Jit::module`] is the handle Stage 1
/// declares and defines functions through.
pub struct Jit {
    module: JITModule,
}

impl Jit {
    /// Stand up the Cranelift JIT module for the host ISA, with the runtime-callback
    /// table registered as absolute symbols (so emitted code resolves `brood_rt_*` to
    /// these Rust functions). No code is compiled here.
    #[allow(clippy::new_without_default)] // construction can fail on an unsupported host
    pub fn new() -> Self {
        let mut builder = JITBuilder::new(default_libcall_names())
            .expect("Cranelift JITBuilder for the host ISA");
        builder.symbol("brood_rt_tick", brood_rt_tick as *const u8);
        builder.symbol("brood_rt_gc_safepoint", brood_rt_gc_safepoint as *const u8);
        builder.symbol("brood_rt_global_epoch", brood_rt_global_epoch as *const u8);
        builder.symbol("brood_rt_alloc_pair", brood_rt_alloc_pair as *const u8);
        builder.symbol("brood_rt_call_slow", brood_rt_call_slow as *const u8);
        builder.symbol("brood_rt_roots_base", brood_rt_roots_base as *const u8);
        Jit { module: JITModule::new(builder) }
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
        self.module.define_function(id, &mut ctx).expect("define smoke fn");
        self.module.clear_context(&mut ctx);
        self.module.finalize_definitions().expect("finalize smoke fn");
        self.module.get_finalized_function(id)
    }
}

#[cfg(test)]
mod smoke {
    use super::*;

    /// End-to-end: Cranelift compiles a constant-returning function, we finalize it and
    /// call the resulting pointer. Validates the codegen pipeline (build + JITModule +
    /// fn-pointer call) before any real arm lowering. No asm, no heap access.
    #[test]
    fn jit_compiles_and_runs_a_constant_fn() {
        let mut jit = Jit::new();
        let ptr = jit.compile_return_const(42);
        let f: extern "C" fn(*mut Heap) -> i64 = unsafe { std::mem::transmute(ptr) };
        assert_eq!(f(std::ptr::null_mut()), 42);
        // `jit` (and its module-owned executable memory) stays alive through the call.
    }
}

/// Preemption poll (ADR-027). JIT'd loop back-edges call this; returns nonzero when the
/// process has spent its reduction budget and should yield. Mirrors the interpreter's
/// `tick_capture` at the bytecode loop top.
#[no_mangle]
pub extern "C" fn brood_rt_tick(_heap: *mut Heap) -> u8 {
    crate::process::tick_capture() as u8
}

/// GC safepoint check. JIT'd code calls this where the interpreter would collect (a
/// loop top / before an allocation burst): collect if due and not inside the compile
/// pass (mirrors the eval safepoint, ADR-061).
///
/// # Safety
/// `heap` must be the live, pinned context pointer for the current JIT'd call, with no
/// live `Value`s outside `Heap::roots` (the no-stack-map invariant, ADR-101 §6.2).
#[no_mangle]
pub unsafe extern "C" fn brood_rt_gc_safepoint(heap: *mut Heap) {
    let h = &mut *heap;
    if !crate::process::macro_block_active() && h.gc_due() {
        h.collect(&mut [], &mut []);
    }
}

/// Read the current global epoch. The JIT'd call-site / global-read inline cache
/// compares its cached epoch against this (`cmp [EPOCH_SLOT], r_epoch; jne slow`,
/// ADR-101 §6.2); a `def` hot-reload bumps the epoch, invalidating every JIT'd IC at
/// its next call exactly as it invalidates the interpreter IC.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_epoch(heap: *mut Heap) -> u64 {
    (*heap).global_epoch()
}

/// Allocate a cons cell from the top two operand-stack slots: `car` is the deeper slot,
/// `cdr` the top (the order `exec_chunk` pushes a 2-arg call's operands). Pops both and
/// pushes the pair. Operating through `roots` keeps the operands and the fresh pair
/// rooted across the allocation's own safepoint (the GC may run inside `alloc_pair`).
///
/// # Safety
/// `heap` must be the live context pointer; the operand stack must hold ≥2 slots.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_alloc_pair(heap: *mut Heap) {
    let h = &mut *heap;
    let cdr = h.pop_root().expect("brood_rt_alloc_pair: operand-stack underflow (cdr)");
    let car = h.pop_root().expect("brood_rt_alloc_pair: operand-stack underflow (car)");
    let pair = h.alloc_pair(car, cdr);
    h.push_root(pair);
}

/// Base pointer of the operand-stack/`roots` buffer. JIT'd code calls this once at
/// entry, then indexes a frame slot `k` directly at `roots_base + k *
/// size_of::<Value>()` (tag byte at +0, payload at +8). Valid for the arm's duration:
/// a tier-1 JIT'd arm keeps operands in registers (never `push`es `roots`) and the
/// int-arithmetic subset never allocates, so `roots` doesn't reallocate.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_roots_base(heap: *mut Heap) -> *mut u8 {
    (*heap).roots_base_ptr() as *mut u8
}

/// Slow-path call dispatch / deopt: when a JIT'd call site can't take its fast path (IC
/// miss, non-closure callee, arity mismatch), it falls back to the interpreter's
/// dispatch on the callee + args already staged in `roots`. The exact protocol (how
/// many roots, where the result lands) is finalized in **Stage 1** with the call
/// lowering; stubbed here so the callback table is complete and the symbol exists for
/// the trampoline/Cranelift module to resolve.
///
/// # Safety
/// `heap` must be the live context pointer.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_call_slow(_heap: *mut Heap, _argc: u32) {
    unimplemented!("brood_rt_call_slow: the JIT call protocol lands in Stage 1 (ADR-101)")
}
