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
        // `opt_level=speed` turns on Cranelift's GVN + alias-aware redundant-load
        // elimination, which matters here: a hot loop body re-reads the same frame slot
        // several times (`(< i 1)`, `(- i 1)`, `(+ acc i)` all tag-check + load slot `i`),
        // and the default `opt_level=none` keeps every one of those loads + tag-checks.
        // The extra compile cost is paid on the background compiler thread, off the hot
        // path; the optimizations are semantics-preserving, so the GC discipline is
        // unaffected. Falls back to default flags if the host rejects the setting.
        let mut builder = JITBuilder::with_flags(
            &[("opt_level", "speed")],
            default_libcall_names(),
        )
        .or_else(|_| JITBuilder::new(default_libcall_names()))
        .expect("Cranelift JITBuilder for the host ISA");
        builder.symbol("brood_rt_tick", brood_rt_tick as *const u8);
        builder.symbol("brood_rt_gc_safepoint", brood_rt_gc_safepoint as *const u8);
        builder.symbol("brood_rt_cons", brood_rt_cons as *const u8);
        builder.symbol("brood_rt_car", brood_rt_car as *const u8);
        builder.symbol("brood_rt_cdr", brood_rt_cdr as *const u8);
        builder.symbol("brood_rt_push", brood_rt_push as *const u8);
        builder.symbol("brood_rt_global", brood_rt_global as *const u8);
        builder.symbol("brood_rt_global_ic", brood_rt_global_ic as *const u8);
        builder.symbol("brood_rt_call_slow", brood_rt_call_slow as *const u8);
        builder.symbol("brood_rt_vector_ref", brood_rt_vector_ref as *const u8);
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

/// `vector-ref` / `nth` of a dense vector by an `Int` index, writing the element to
/// `*out` and returning `0`; returns `1` (deopt to the VM) for a non-vector receiver, a
/// non-`Int` index, or an out-of-range index — the VM then applies the exact semantics
/// (`vector-ref`'s bounds error, or `nth`'s `default`). Reads the slab only; never
/// allocates, so it is not a safepoint (a `Handle` produced here is consumed before any
/// collection).
///
/// # Safety
/// `heap`/`out` live; the word triples are real `Value`s.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_vector_ref(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    v0: i64,
    v1: i64,
    v2: i64,
    i0: i64,
    i1: i64,
    i2: i64,
) -> i64 {
    use crate::core::value::Value;
    let h = &mut *heap;
    let (vid, idx) = match (words_to_val(v0, v1, v2), words_to_val(i0, i1, i2)) {
        (Value::Vector(id), Value::Int(n)) => (id, n),
        _ => return 1,
    };
    let v = h.vector(vid);
    if idx < 0 || idx as usize >= v.len() {
        return 1;
    }
    *out = v[idx as usize];
    0
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

/// Push a `Value` (by word-triple) onto the operand stack (`roots`). The JIT stages a
/// Brood→Brood call's callee + args here, in the VM's `Inst::Call` layout, before
/// [`brood_rt_call_slow`]. Goes through `push_root` so the `roots` length/capacity are
/// maintained; a growth may reallocate the buffer, so the JIT re-fetches
/// [`brood_rt_roots_base`] after the call.
///
/// # Safety
/// `heap` must be the live context pointer; the word triple is bytes the JIT read out
/// of a real `Value` (a slot, an `Int` box, or a handle result).
#[no_mangle]
pub unsafe extern "C" fn brood_rt_push(heap: *mut Heap, w0: i64, w1: i64, w2: i64) {
    (*heap).push_root(words_to_val(w0, w1, w2));
}

/// Resolve a free global `sym` (a JIT'd call's callee-loading `Inst::Global`/`GlobalIc`,
/// or a global read in value position), writing the value to `*out`. Returns 0 on
/// success, 1 if unbound — in which case the error is parked for the arm to propagate
/// (it returns the error outcome, 3). Reads the *live* env, so a `def` rebind is seen
/// immediately (late binding, exactly like the VM's `Inst::Global`).
///
/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
) -> i64 {
    match crate::eval::compile::jit_resolve_global(&mut *heap, sym) {
        Some(v) => {
            *out = v;
            0
        }
        None => 1,
    }
}

/// Resolve a free global through the per-site global inline cache (the same
/// [`Heap::vm_global_ics`] the VM's `Inst::GlobalIc` uses), keyed by `site`. On a
/// process-global env this serves a cached, epoch-stamped value instead of walking
/// `env_get` every call — the difference between a hot recursive callee (`fib`) costing
/// one cached read vs. a full name resolution per call. Late binding is preserved by the
/// epoch stamp: a `def` bumps the global epoch, the probe misses, and it re-resolves
/// (and the JIT'd arm is itself invalidated by the same epoch). 0 on success, 1 if
/// unbound (error parked).
///
/// # Safety
/// `heap`/`out` must be live; `sym` is an interned [`crate::core::value::Symbol`].
#[no_mangle]
pub unsafe extern "C" fn brood_rt_global_ic(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    sym: u32,
    site: u32,
) -> i64 {
    match crate::eval::compile::jit_resolve_global_ic(&mut *heap, sym, site) {
        Some(v) => {
            *out = v;
            0
        }
        None => 1,
    }
}

/// Run a JIT'd arm's **non-tail** Brood→Brood call. The callee + `argc` args have been
/// staged on the operand stack (`roots`) in the VM's `Inst::Call` layout
/// (`[.., callee, arg0 .. arg_{argc-1}]`); this mirrors the non-tail `Inst::Call` path —
/// read them, dispatch through the interpreter to completion, truncate the operands,
/// and write the result to `*out`. Returns 0 on success, 1 on error (parked for the arm
/// to propagate). The callee runs as a **nested, non-top-level** VM apply, so it can't
/// preempt/suspend across this native boundary (the §7.4 dirty carve-out) — exactly
/// like a Rust builtin calling back into Brood. See
/// [`crate::eval::compile::jit_dispatch_call`].
///
/// # Safety
/// `heap`/`out` must be live; `argc` callee+args are staged on `roots`.
#[no_mangle]
pub unsafe extern "C" fn brood_rt_call_slow(
    heap: *mut Heap,
    out: *mut crate::core::value::Value,
    argc: u32,
) -> i64 {
    match crate::eval::compile::jit_dispatch_call(&mut *heap, argc as usize) {
        Some(v) => {
            *out = v;
            0
        }
        None => 1,
    }
}
