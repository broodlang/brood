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
use std::sync::{LazyLock, Mutex};

/// The process-wide JIT module (tiering, 1b). It owns every compiled arm's executable
/// code, which must outlive all installed fn-pointers — hence a single process-lifetime
/// instance. Compilation mutates it (`declare`/`define`/`finalize`), so it's behind a
/// `Mutex`; the resulting machine code lives in a shared executable mmap and is callable
/// from any worker thread once installed (`JITModule` is `Send`). For the int subset a
/// compiled arm is self-contained (no globals), so a process-wide module is correct;
/// arms that reference a runtime's globals bail today, so per-runtime isolation isn't
/// needed yet.
pub(crate) static GLOBAL_JIT: LazyLock<Mutex<Jit>> = LazyLock::new(|| Mutex::new(Jit::new()));

/// Sentinel in [`crate::eval::compile::CompiledArm`]`::jit_code` for an arm that was
/// tried and is out of the JIT's subset — distinct from null (untried) and a real,
/// 8-aligned code pointer.
pub(crate) const BAILED: *mut u8 = 1 as *mut u8;

/// Sentinel: the arm is hot and has been handed to the background compiler thread, but
/// its native code isn't installed yet. Callers run the VM until the real pointer
/// replaces this. Distinct from null/`BAILED`/a real (8-aligned) pointer.
pub(crate) const QUEUED: *mut u8 = 2 as *mut u8;

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
        builder.symbol("brood_rt_cons", brood_rt_cons as *const u8);
        builder.symbol("brood_rt_car", brood_rt_car as *const u8);
        builder.symbol("brood_rt_cdr", brood_rt_cdr as *const u8);
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
/// process should yield. Mirrors the VM's loop-top exactly: only a **capture-mode green
/// process** is preemptible (`tick_capture` decrements the reduction budget and yields at
/// zero); the root/eval thread (and any non-capture run) never preempts — it just keeps
/// going, like the VM's `tick()` else-branch. Gating here is load-bearing: without it a
/// JIT'd loop on the root thread would yield on its first iteration and bail to the VM,
/// so the JIT could never actually run the loop.
#[no_mangle]
pub extern "C" fn brood_rt_tick(_heap: *mut Heap) -> u8 {
    if crate::process::in_capture_run() {
        crate::process::tick_capture() as u8
    } else {
        0 // root / non-capture: never preempt (matches the VM)
    }
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

// ---- The handle ops: cons / car / cdr, by-value with an out-pointer. ----
//
// A `Value` is 24 bytes (3 i64 words: tag at 0, payload words at 8 and 16 — the layout
// the JIT reads/writes a roots slot through), so it can't be a C register-pair return.
// Instead the JIT passes an `out: *mut Value` (a stack slot it owns) and the callback
// writes the result there; the JIT reads the three words back into an `Op::Handle`. The
// operands likewise arrive as word triples the JIT read out of real `Value`s (a slot, an
// `Int` box, or a previous handle result), so `words_to_val` is the identity on their
// bytes. `alloc_pair` only grows the nursery (never collects), so a reconstructed operand
// can't go stale mid-`cons`; no `roots` is touched, so `roots_base` stays valid.
#[inline]
unsafe fn words_to_val(w0: i64, w1: i64, w2: i64) -> crate::core::value::Value {
    std::mem::transmute::<[i64; 3], crate::core::value::Value>([w0, w1, w2])
}

/// `cons` two `Value`s (each by word-triple), writing the fresh pair to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triples are bytes the JIT read out of real `Value`s.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_cons(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    c0: i64,
    c1: i64,
    c2: i64,
    d0: i64,
    d1: i64,
    d2: i64,
) {
    let h = &mut *heap;
    let car = words_to_val(c0, c1, c2);
    let cdr = words_to_val(d0, d1, d2);
    *out = h.alloc_pair(car, cdr);
}

/// `first` of a `Value` (by word-triple), writing its car to `*out`. The JIT **tag-checks
/// for `Pair` and deopts before calling**, so a non-pair (impossible by that contract)
/// yields `nil` rather than UB.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_car(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    let h = &mut *heap;
    *out = match words_to_val(w0, w1, w2) {
        crate::core::value::Value::Pair(id) => h.pair(id).0,
        _ => crate::core::value::Value::Nil,
    };
}

/// `rest` counterpart of [`brood_rt_car`] — writes the pair's cdr to `*out`.
///
/// # Safety
/// `heap`/`out` live; the word triple is a real `Value::Pair`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_cdr(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    w0: i64,
    w1: i64,
    w2: i64,
) {
    let h = &mut *heap;
    *out = match words_to_val(w0, w1, w2) {
        crate::core::value::Value::Pair(id) => h.pair(id).1,
        _ => crate::core::value::Value::Nil,
    };
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
