//! CLIF emit helpers for `jit_lower_arm_inner`, extracted from the closures that
//! took `b: &mut FunctionBuilder` and captured the `deopt` block. Part of the
//! `jit_lower_arm_inner` decomposition; jit-only. `Op` is the shared operand model
//! (module scope in `jit_lower.rs`).
#![cfg(feature = "jit")]
use super::*;
use crate::core::heap::VecStore as VS;
use crate::core::value::jit_layout::{
    PAYLOAD_OFFSET, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_KEYWORD, TAG_SYM, TAG_VECTOR,
};
use cranelift_codegen::ir::{
    condcodes::IntCC, types, Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, StackSlot, Type,
};
use cranelift_frontend::{FunctionBuilder, Variable};

/// The runtime-call context threaded into the extracted call/read helpers (and,
/// as the decomposition proceeds, the per-`Inst` arm bodies): the heap pointer
/// param, the scratch out-slot for the out-pointer ABI, the target pointer type,
/// the arm's shared `error` exit block, and the runtime-callback `FuncRef`s the
/// helpers dispatch through. All `Copy`, so it threads freely alongside [`Frame`].
#[derive(Clone, Copy)]
pub(super) struct Funcs {
    pub ptr_ty: Type,
    pub heap: cranelift_codegen::ir::Value,
    pub out_slot: StackSlot,
    /// The arm's error-exit block (outcome 3 — a parked `jit_pending_error`).
    pub error: Block,
    /// `brood_rt_vec_nursery_base` / `_old_base` — the vector-slab base fetchers.
    pub vnbase: FuncRef,
    pub vobase: FuncRef,
    /// `brood_rt_vector_ref` — the bounds-checked FFI fallback read.
    pub vref: FuncRef,
    /// `brood_rt_car` / `brood_rt_cdr` — the `first`/`rest` handle ops (FFI fallback).
    pub car: FuncRef,
    pub cdr: FuncRef,
    /// `brood_rt_cons` — pair allocation.
    pub cons: FuncRef,
    /// `brood_rt_vec2_room(heap, out) -> *mut Value` — allocate a 2-element vector and
    /// return its element storage, so the arm writes the elements in place.
    pub vec2room: FuncRef,
    /// `brood_rt_make_closure(heap, out, inst) -> status` — build a `(fn …)` literal's
    /// closure (exec_chunk's arm verbatim; captures staged on `roots`).
    pub mkclo: FuncRef,
    pub makevecn: FuncRef,
    /// `brood_rt_table_has` / `_get2` / `_put` — the table primitives (FFI fallback).
    pub thas: FuncRef,
    pub tget: FuncRef,
    pub tput: FuncRef,
    /// `brood_rt_map_get` — the CHAMP probe behind [`PrimOp::MapGet`]. Same
    /// `(heap, out, 3 words, 3 words) -> status` shape as the table reads, and the same
    /// `table_prim` helper drives it: status 0 hands back the value, status 1 deopts to the
    /// VM, which owns every branch of `get` this declines.
    pub mget: FuncRef,
    /// `brood_rt_roots_base` — re-fetch the frame base after a call may realloc `roots`.
    pub rb: FuncRef,
    /// `brood_rt_global_ic` — resolve a free global through the per-site inline cache.
    pub globic: FuncRef,
    /// `brood_rt_push_room(heap, n) -> *mut Value` — reserve n argument slots on `roots`
    /// and return them, so operands are stored in place rather than copied across.
    pub pushroom: FuncRef,
    /// `brood_rt_call_slow` — the general Brood→Brood dispatch (the fast-link miss path).
    pub callslow: FuncRef,
    /// `brood_rt_call_native_fl` — direct builtin call for a native flat-cell hit.
    pub natfl: FuncRef,
    /// `brood_rt_fastlink_base` / `brood_rt_fast_frame` — the in-IR epoch-guarded fast link.
    pub flbase: FuncRef,
    pub fastframe: FuncRef,
    /// `brood_rt_xcall_latch` / `brood_rt_xcall_cold` — the inline fast-frame path's
    /// cold callbacks (§7.5, `BROOD_XCALL=1`).
    pub xlatch: FuncRef,
    pub xcold: FuncRef,
    /// The [`crate::jit::JitArmFn`] signature `(heap, base, out) -> outcome`, for the
    /// inline path's `call_indirect` straight into a callee's native code.
    pub armfn_sig: cranelift_codegen::ir::SigRef,
    /// Emit the inline fast-frame path in THIS lowering (§7.5): the flag is armed and
    /// this body is one whose compile cost is already deferred (the inlined upgrade) —
    /// the small first body keeps the callback so short runs never pay the fatter IR.
    pub xcall: bool,
    /// `brood_rt_gc_safepoint` / `brood_rt_tick_n` — the self-loop back-edge callbacks.
    pub sp: FuncRef,
    pub tickn: FuncRef,
    /// DEBUG: `brood_rt_dbg_set_staging` — record the staging call site.
    #[cfg(debug_assertions)]
    pub dbg_staging: FuncRef,
}

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// BEAM-style reduction batch for the self-tail loop: the in-register countdown that
/// gates the back-edge preemption poll + hoisted-global epoch guard. Shared by the
/// entry-block initializer (`jit_lower_arm_inner`) and the `SelfCall` back-edge
/// (`call::emit_self_call`), so both must agree.
pub(super) const TICK_BATCH: i64 = 128;

/// The frame-access context the slot helpers need: the `roots` base variable, the
/// arm's frame base offset, the slot count, the shared `deopt` block, and the
/// register-carried param-slot table. All `Copy`, so it threads freely.
#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    pub rb_var: Variable,
    pub base: cranelift_codegen::ir::Value,
    /// Where a Done result is written — the caller's `out` pointer (arm ABI parameter 2, see
    /// [`crate::jit::JitArmFn`]). It lives on `Frame` rather than being threaded to the exit
    /// helpers because there is **more than one Done exit** (`exit_done` in `jit_lower.rs`
    /// and the `t == len` arm of [`control::emit_jump`]), and the first migration of this ABI
    /// missed the second one: the `if`/loop arms kept writing `roots[base]` that nobody read
    /// any more, and every such arm returned `nil`. Carrying it here makes "which exits must
    /// be updated" a question the type system answers.
    pub out_ptr: cranelift_codegen::ir::Value,
    pub nslots: usize,
    pub deopt: cranelift_codegen::ir::Block,
    pub carry_vars: &'a [Option<(Variable, bool)>],
    /// Per-slot "holds a `Value::Float`" / "holds a `Value::Bool`" flags, tracked
    /// across the single lowering pass so a later slot read picks the right arith /
    /// block-arg representation. Shared (`RefCell`) — set by stores, read by loads.
    pub slot_float: &'a std::cell::RefCell<Vec<bool>>,
    pub slot_bool: &'a std::cell::RefCell<Vec<bool>>,
    /// Per-slot "the tier-time profile saw an `Int` here" (from `slot_tags`). Read only by
    /// [`param_repr`], to decide how an operand crosses a block boundary: a profiled-Int
    /// slot keeps the unboxed i64 carry (the `fib`/`collatz` fast path), anything else is
    /// carried as [`ParamRepr::Slot`] instead of being forced through `as_int` — which is
    /// KI-49, where a matcher's message vector deopted at every merge.
    pub slot_int_profile: &'a [bool],
    /// Base frame slot of the **block-argument spill** region (KI-49). An operand that must
    /// cross a block boundary but is not a profiled `Int` is stored at
    /// `blockarg_spill_base + <its operand-stack index>` and carried as `ParamRepr::Slot`.
    /// Indexed by stack POSITION, not allocation order, so every predecessor of a block
    /// names the same slot — otherwise `record_block_flags` rejects the edge.
    pub blockarg_spill_base: usize,
    /// How many block-argument spill slots exist (`max_leader_depth`). An operand deeper
    /// than this cannot be spilled, and the arm bails rather than writing out of bounds.
    pub blockarg_spill_len: usize,
    /// Per-slot cache of the unboxed `f64` SSA value last stored via `store_op`,
    /// valid within a block: a later `as_f64` read returns it directly, skipping the
    /// store→load→bitcast round-trip. `None` for slots not written as a float (params
    /// are always `None`). Shared (`RefCell`) — see `as_f64` for the invalidation rules.
    pub slot_f64_cache: &'a std::cell::RefCell<Vec<Option<cranelift_codegen::ir::Value>>>,
}

/// How one operand-stack entry crosses a block boundary. Block params are `I64`, so the
/// representation has to be agreed by every predecessor (`record_block_flags`); a
/// disagreeing edge is routed to `deopt`, exactly as it was when this was a plain `bool`.
///
/// `Slot` is the KI-49 fix. Previously every carried operand went through `as_int`, which
/// tag-checks `Int` and deopts otherwise — so an arm carrying a non-Int value across a
/// merge (any tagged-tuple matcher: it tests `vector?`, then `vector-length`, then `nth`
/// on the same message) deopted on *every* activation and was latched to the interpreter.
/// Carrying "the value is in frame slot k" instead needs no unboxing and cannot deopt.
///
/// Int is kept as the default for profiled-`Int` slots so the unboxed integer carry —
/// what makes `fib`/`collatz` fast — is untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ParamRepr {
    Int,
    Bool,
    /// The value lives in frame slot `k`; the arg word is a placeholder. Every predecessor
    /// must name the *same* slot, which `record_block_flags` enforces by equality.
    Slot(usize),
}

/// The representation `op` at operand-stack index `idx` will cross a block boundary as.
///
/// `idx` selects the block-argument spill slot for a `Handle`, so it must be the operand's
/// position in the abstract stack — the same value at every predecessor.
pub(super) fn param_repr(b: &FunctionBuilder, op: Op, idx: usize, f: Frame) -> ParamRepr {
    if is_bool_op(b, op, f) {
        return ParamRepr::Bool;
    }
    match op {
        // A slot the tier-time profile did NOT see an Int in: carry it as a slot reference
        // rather than unboxing it (which would deopt for a vector/map/string/…).
        Op::Slot(k) if !f.slot_int_profile.get(k).copied().unwrap_or(false) => ParamRepr::Slot(k),
        // A boxed value with no slot of its own — the KI-49 case (a `MakeVector` result
        // crossing an `if` merge). Spill it to the block-argument region at its stack index.
        Op::Handle(..) if idx < f.blockarg_spill_len => {
            ParamRepr::Slot(f.blockarg_spill_base + idx)
        }
        _ => ParamRepr::Int,
    }
}

/// Does `op` carry a `Value::Float`? (An `Op::Float`, or a `Slot` flagged float.)
pub(super) fn op_is_float(op: Op, f: Frame) -> bool {
    match op {
        Op::Float(_) => true,
        Op::Slot(k) => f.slot_float.borrow().get(k).copied().unwrap_or(false),
        _ => false,
    }
}

/// Mark frame slot `dst` as holding (or not) a `Value::Float`.
pub(super) fn set_slot_float(dst: i64, v: bool, f: Frame) {
    if let Some(s) = f.slot_float.borrow_mut().get_mut(dst as usize) {
        *s = v;
    }
}

/// Mark frame slot `dst` as holding (or not) a `Value::Bool`.
pub(super) fn set_slot_bool(dst: i64, v: bool, f: Frame) {
    if let Some(s) = f.slot_bool.borrow_mut().get_mut(dst as usize) {
        *s = v;
    }
}

/// Does `op` carry a `Value::Bool`? (An `Op::Bool`, an `i8` comparison `Op::Int`,
/// or a `Slot` flagged bool.) Used to type block-param edges at a join.
pub(super) fn is_bool_op(b: &FunctionBuilder, op: Op, f: Frame) -> bool {
    matches!(op, Op::Bool(_))
        || matches!(op, Op::Int(v) if b.func.dfg.value_type(v) == types::I8)
        || matches!(op, Op::Slot(k) if f.slot_bool.borrow().get(k).copied().unwrap_or(false))
}

/// Integer arithmetic/comparison lowering (overflow-checked → deopt to BigInt).
/// Returns the SSA result, or None if `op` is not lowered as unboxed integer arith.
pub(super) fn emit_arith(
    b: &mut FunctionBuilder,
    op: PrimOp,
    x: cranelift_codegen::ir::Value,
    y: cranelift_codegen::ir::Value,
    deopt: cranelift_codegen::ir::Block,
) -> Option<cranelift_codegen::ir::Value> {
    let checked = |b: &mut FunctionBuilder, r: cranelift_codegen::ir::Value, ov| {
        let cont = b.create_block();
        let __dr = b.ins().iconst(types::I32, 15);
        b.ins().brif(ov, deopt, &[BlockArg::Value(__dr)], cont, &[]);
        b.switch_to_block(cont);
        r
    };
    Some(match op {
        PrimOp::Add => {
            let (r, ov) = b.ins().sadd_overflow(x, y);
            checked(b, r, ov)
        }
        PrimOp::Sub => {
            let (r, ov) = b.ins().ssub_overflow(x, y);
            checked(b, r, ov)
        }
        PrimOp::Mul => {
            let (r, ov) = b.ins().smul_overflow(x, y);
            checked(b, r, ov)
        }
        PrimOp::Lt => b.ins().icmp(IntCC::SignedLessThan, x, y),
        PrimOp::Le => b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y),
        PrimOp::Eq => b.ins().icmp(IntCC::Equal, x, y),
        // Integer division family (`rem`/`quot`/`%div`). Cranelift's `sdiv`/`srem`
        // *trap* on a zero divisor and on the `i64::MIN / -1` overflow, so both must
        // be guarded → deopt before the op (the VM's inline path defers exactly these
        // edges to the native, matching). `%div` additionally yields an `Int` only on
        // an exact quotient — a nonzero remainder means a `Float` the native builds,
        // so deopt then too. Operand order is already `(x, y)` (map applied).
        PrimOp::Rem | PrimOp::Quot | PrimOp::Div => {
            let zero = b.ins().iconst(types::I64, 0);
            let div0 = b.ins().icmp(IntCC::Equal, y, zero);
            let c0 = b.create_block();
            let __dr = b.ins().iconst(types::I32, 16);
            b.ins().brif(div0, deopt, &[BlockArg::Value(__dr)], c0, &[]);
            b.switch_to_block(c0);
            // (x == i64::MIN) && (y == -1) — the one signed-division overflow.
            let min = b.ins().iconst(types::I64, i64::MIN);
            let neg1 = b.ins().iconst(types::I64, -1);
            let x_min = b.ins().icmp(IntCC::Equal, x, min);
            let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
            let ov = b.ins().band(x_min, y_m1);
            let c1 = b.create_block();
            let __dr = b.ins().iconst(types::I32, 17);
            b.ins().brif(ov, deopt, &[BlockArg::Value(__dr)], c1, &[]);
            b.switch_to_block(c1);
            match op {
                PrimOp::Rem => b.ins().srem(x, y),
                PrimOp::Quot => b.ins().sdiv(x, y),
                PrimOp::Div => {
                    // Exact only: a nonzero remainder → Float → deopt to the native.
                    let r = b.ins().srem(x, y);
                    let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                    let c2 = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 18);
                    b.ins()
                        .brif(inexact, deopt, &[BlockArg::Value(__dr)], c2, &[]);
                    b.switch_to_block(c2);
                    b.ins().sdiv(x, y)
                }
                _ => unreachable!(),
            }
        }
        PrimOp::Max => {
            let cond = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, x, y);
            b.ins().select(cond, x, y)
        }
        PrimOp::Min => {
            let cond = b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y);
            b.ins().select(cond, x, y)
        }
        PrimOp::BitAnd => b.ins().band(x, y),
        PrimOp::BitOr => b.ins().bor(x, y),
        PrimOp::BitXor => b.ins().bxor(x, y),
        PrimOp::Cons => return None, // allocates — never in the JIT subset
        PrimOp::VectorRef => return None, // heap slab read — not lowered; out of subset
        // Table ops: not an int-arith op — lowered as a runtime callback in the
        // Inst::Prim2 arm below (never through this integer emitter).
        PrimOp::TableHas | PrimOp::TableGet => return None,
        // CHAMP probe through the runtime callback, like the table ops — not integer
        // arithmetic, so it is lowered in `prim.rs`, not here.
        PrimOp::MapGet => return None,
    })
}

/// Float arithmetic/comparison lowering (f64). Returns an `Op::Float`/`Op::Int`,
/// or None if `op` is not lowered for floats (e.g. structural `=`).
pub(super) fn emit_float_arith(
    b: &mut FunctionBuilder,
    op: PrimOp,
    x: cranelift_codegen::ir::Value,
    y: cranelift_codegen::ir::Value,
    deopt: cranelift_codegen::ir::Block,
) -> Option<Op> {
    use cranelift_codegen::ir::condcodes::FloatCC;
    Some(match op {
        PrimOp::Add => Op::Float(b.ins().fadd(x, y)),
        PrimOp::Sub => Op::Float(b.ins().fsub(x, y)),
        PrimOp::Mul => Op::Float(b.ins().fmul(x, y)),
        PrimOp::Div => {
            // Brood float `/` raises on a zero divisor (matches the VM — `(/ x 0.0)`
            // errors), so guard `y == 0.0` and deopt (the VM then raises); otherwise
            // `fdiv`. `fcmp Equal 0.0` catches +0.0 and -0.0 alike.
            let zero = b.ins().f64const(0.0);
            let is_zero = b.ins().fcmp(FloatCC::Equal, y, zero);
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 19);
            b.ins()
                .brif(is_zero, deopt, &[BlockArg::Value(__dr)], cont, &[]);
            b.switch_to_block(cont);
            Op::Float(b.ins().fdiv(x, y))
        }
        PrimOp::Lt => Op::Int(b.ins().fcmp(FloatCC::LessThan, x, y)),
        PrimOp::Le => Op::Int(b.ins().fcmp(FloatCC::LessThanOrEqual, x, y)),
        // `=` is NOT lowered for floats: Brood `=` is *structural*, so a Float
        // is never equal to an Int (`(= 2.0 2)` is false), but IEEE `fcmp Equal`
        // — after the int-literal-to-f64 coercion the `Prim2SlotInt` float path
        // applies — would return true for `(= 2.0 2)`. Returning `None` bails the
        // arm to the VM, whose `prim_apply_float` likewise returns `None` for `Eq`
        // and defers to the structural native `prim_eq`. (Lt/Le are safe: ordering
        // coerces int↔float identically on both engines.)
        _ => return None,
    })
}

/// Box an unboxed scalar SSA value into a `(tag, payload)` pair: an `i64` is an
/// `Int`; anything narrower (an `i8` comparison result) is a `Bool` (uext to i64).
pub(super) fn box_scalar(
    b: &mut FunctionBuilder,
    v: cranelift_codegen::ir::Value,
) -> (u8, cranelift_codegen::ir::Value) {
    if b.func.dfg.value_type(v) == types::I64 {
        (TAG_INT, v)
    } else {
        (TAG_BOOL, b.ins().uextend(types::I64, v))
    }
}

/// Load frame slot `k` as an unboxed `i64`, tag-checking `Int` first (a non-Int
/// operand branches to `deopt`). A register-carried param slot skips the check.
pub(super) fn load_slot_int(
    b: &mut FunctionBuilder,
    k: i64,
    f: Frame,
) -> cranelift_codegen::ir::Value {
    if let Some((var, false)) = f.carry_vars.get(k as usize).copied().flatten() {
        return b.use_var(var);
    }
    let roots_base = b.use_var(f.rb_var);
    let idx = b.ins().iadd_imm_s(f.base, k);
    let off = b.ins().imul_imm_s(idx, STRIDE);
    let addr = b.ins().iadd(roots_base, off);
    let tag = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
    let is_int = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_INT as i64);
    let cont = b.create_block();
    let __dr = b.ins().iconst(types::I32, 20);
    b.ins()
        .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(cont);
    b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        addr,
        PAYLOAD_OFFSET as i32,
    )
}

/// Store an unboxed scalar into frame slot `k`, boxing via [`box_scalar`].
pub(super) fn store_int(
    b: &mut FunctionBuilder,
    k: i64,
    v: cranelift_codegen::ir::Value,
    f: Frame,
) {
    debug_assert!(
        (k as usize) < f.nslots,
        "[jit-slot] store_int slot {} >= nslots {}",
        k,
        f.nslots
    );
    let (tag_byte, payload) = box_scalar(b, v);
    let roots_base = b.use_var(f.rb_var);
    let idx = b.ins().iadd_imm_s(f.base, k);
    let off = b.ins().imul_imm_s(idx, STRIDE);
    let addr = b.ins().iadd(roots_base, off);
    let tag = b.ins().iconst(types::I8, tag_byte as i64);
    b.ins().store(MemFlagsData::trusted(), tag, addr, 0);
    b.ins().store(
        MemFlagsData::trusted(),
        payload,
        addr,
        PAYLOAD_OFFSET as i32,
    );
}

/// Copy a whole `Value` (all words, handle-safe) from frame slot `src` to `dst`.
pub(super) fn copy_value(b: &mut FunctionBuilder, src: i64, dst: i64, f: Frame) {
    debug_assert!(
        (src as usize) < f.nslots && (dst as usize) < f.nslots,
        "[jit-slot] copy_value src {} dst {} vs nslots {}",
        src,
        dst,
        f.nslots
    );
    let roots_base = b.use_var(f.rb_var);
    let saddr = {
        let i = b.ins().iadd_imm_s(f.base, src);
        let o = b.ins().imul_imm_s(i, STRIDE);
        b.ins().iadd(roots_base, o)
    };
    let daddr = {
        let i = b.ins().iadd_imm_s(f.base, dst);
        let o = b.ins().imul_imm_s(i, STRIDE);
        b.ins().iadd(roots_base, o)
    };
    let mut off = 0i32;
    while (off as i64) < STRIDE {
        let w = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), saddr, off);
        b.ins().store(MemFlagsData::trusted(), w, daddr, off);
        off += 8;
    }
}

/// Read an operand as the three raw words of a whole `Value` (`[tag, w1, w2]`):
/// an unboxed scalar is boxed, a `Slot` is loaded, a `Handle`/hoisted value moves
/// its words verbatim. The workhorse behind pushing an operand onto a runtime
/// call, a self-call arg slot, or a return.
pub(super) fn read_words(
    b: &mut FunctionBuilder,
    op: Op,
    f: Frame,
) -> [cranelift_codegen::ir::Value; 3] {
    match op {
        Op::Int(v) => {
            // Box as `Int` or (a comparison `i8`) `Bool`; both payloads are `i64`, so
            // the triple is a valid `[i64; 3]` whole `Value`.
            let (tag_byte, payload) = box_scalar(b, v);
            let tag = b.ins().iconst(types::I64, tag_byte as i64);
            let zero = b.ins().iconst(types::I64, 0);
            [tag, payload, zero]
        }
        Op::Slot(k) => {
            // DEBUG: a real/spill slot must be inside the frame [0, nslots). A k >= nslots
            // reads past the frame into staging/stale memory — the bug #2 slot-count gap.
            debug_assert!(
                k < f.nslots,
                "[jit-slot] read_words Op::Slot({k}) >= nslots {} — slot count undercounted",
                f.nslots
            );
            let roots_base = b.use_var(f.rb_var);
            let i = b.ins().iadd_imm_s(f.base, k as i64);
            let o = b.ins().imul_imm_s(i, STRIDE);
            let addr = b.ins().iadd(roots_base, o);
            let w0 = b.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
            let w1 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32,
            );
            let w2 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32 + 8,
            );
            // NOTE: an in-IR validation call here (dbg_check_slot_ref) PERTURBS codegen —
            // it forces register spills around the call that mask the very register-liveness
            // bug we're hunting (#2). Validation lives in the Rust-side `brood_rt_push`.
            [w0, w1, w2]
        }
        Op::Float(v) => {
            // Box an unboxed `f64` as a whole `Value::Float`: [TAG_FLOAT, bits, 0].
            let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), v);
            let tag = b.ins().iconst(types::I64, TAG_FLOAT as i64);
            let zero = b.ins().iconst(types::I64, 0);
            [tag, bits, zero]
        }
        Op::Bool(v) => {
            // A crossed-boundary boolean (already `i64` 0/1) → `Value::Bool`.
            let tag = b.ins().iconst(types::I64, TAG_BOOL as i64);
            let zero = b.ins().iconst(types::I64, 0);
            [tag, v, zero]
        }
        Op::Handle(w0, w1, w2) => {
            // NOTE: no in-IR validation call here — it would perturb codegen and mask the
            // bug (see Op::Slot above). Register handles flow to brood_rt_push for checking.
            [w0, w1, w2]
        }
        // A hoisted global vector used as a whole `Value` (any non-`VectorRef`
        // consumer): its entry-resolved words move verbatim, exactly like a `Handle`.
        Op::HoistedVec { w0, w1, w2, .. } => [w0, w1, w2],
        // Same for a hoisted global table used as a whole `Value`.
        Op::HoistedTable { w0, w1, w2, .. } => [w0, w1, w2],
    }
}

/// Store the three words of a `Value` into frame slot `dst`.
pub(super) fn store_words(
    b: &mut FunctionBuilder,
    dst: i64,
    w: [cranelift_codegen::ir::Value; 3],
    f: Frame,
) {
    debug_assert!(
        (dst as usize) < f.nslots,
        "[jit-slot] store_words slot {dst} >= nslots {}",
        f.nslots
    );
    let roots_base = b.use_var(f.rb_var);
    let i = b.ins().iadd_imm_s(f.base, dst);
    let o = b.ins().imul_imm_s(i, STRIDE);
    let addr = b.ins().iadd(roots_base, o);
    b.ins().store(MemFlagsData::trusted(), w[0], addr, 0);
    b.ins()
        .store(MemFlagsData::trusted(), w[1], addr, PAYLOAD_OFFSET as i32);
    b.ins().store(
        MemFlagsData::trusted(),
        w[2],
        addr,
        PAYLOAD_OFFSET as i32 + 8,
    );
}

/// Materialise an operand to an unboxed `i64`: a register value as-is, a tag-checked
/// load of a frame slot, or a tag-checked extract of a `Handle`'s payload (a `Handle`
/// used as a number — e.g. `(+ (first xs) 1)` — must be an `Int` at runtime or deopt).
///
/// **A boolean is not a number here, and must not be admitted as one.** Every boxed shape
/// (`Slot`, `Handle`, `HoistedVec`) is tag-checked and `Float` deopts, but `Op::Bool` and an
/// `i8`-typed `Op::Int` (a comparison result — `icmp`/`fcmp` produce `i8`, and `box_scalar`
/// boxes those as `Value::Bool`) used to pass through raw. That was two bugs at once:
///
///   * **an engine divergence.** A `Const` boolean materialises as `Op::Bool(iconst 0/1)`
///     (`jit_lower.rs`), so a lowered `(+ acc true)` computed `acc + 1` where the tree-walker
///     and the VM both raise `+: expected number, got bool` (the VM's `prim_apply` returns
///     `None` for a non-numeric operand and dispatches the real `+` native, which errors).
///     Guarded by `tests/jit_bool_arith_test.blsp`.
///   * **an `i8` into `i64` arithmetic.** A comparison used directly as an arithmetic operand
///     (`(+ (< x 0) 1)`) reached `emit_arith` as `sadd_overflow(i8, i64)`, which Cranelift
///     refuses — the whole arm then declined to lower with no diagnostic beyond
///     `reason=lowering-returned-none`, so a cold nonsense branch silently cost the *hot* arm
///     its native code.
///
/// Deopting is right for both: the VM re-runs the arm and produces the real type error (or,
/// for a branch that never actually executes, never gets there). A bool crossing a block
/// boundary is a different question and does NOT come through here — see [`as_block_arg`].
pub(super) fn as_int(b: &mut FunctionBuilder, op: Op, f: Frame) -> cranelift_codegen::ir::Value {
    // A boolean where a number is required: deopt (the VM raises the type error). Covers both
    // spellings — an explicit `Op::Bool` and the `i8` comparison result carried as `Op::Int`.
    if matches!(op, Op::Bool(_))
        || matches!(op, Op::Int(v) if b.func.dfg.value_type(v) == types::I8)
    {
        let __dr = b.ins().iconst(types::I32, 27);
        b.ins().jump(f.deopt, &[BlockArg::Value(__dr)]);
        let dead = b.create_block();
        b.switch_to_block(dead);
        return b.ins().iconst(types::I64, 0);
    }
    match op {
        Op::Int(v) => v,
        Op::Bool(v) => v, // unreachable: handled above
        Op::Slot(k) => load_slot_int(b, k as i64, f),
        Op::Handle(w0, w1, _) => {
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_int = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_INT as i64);
            let cont = b.create_block();
            // KI-49: carry the OBSERVED tag in the low byte of the reason id, so the deopt
            // says not just "a Handle wasn't an Int here" but which type actually arrived.
            let __base = b.ins().iconst(types::I32, 21 << 8);
            let __t32 = b.ins().ireduce(types::I32, tagb);
            let __dr = b.ins().bor(__base, __t32);
            b.ins()
                .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(__dr)]);
            b.switch_to_block(cont);
            w1
        }
        // A hoisted global vector/table used as an int (neither is one) — tag-check
        // its word like a `Handle` and deopt; sound, never expected to fire.
        Op::HoistedVec { w0, w1, .. } | Op::HoistedTable { w0, w1, .. } => {
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_int = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_INT as i64);
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 22);
            b.ins()
                .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(__dr)]);
            b.switch_to_block(cont);
            w1
        }
        // A float where an int is required (a mixed-type op the lowering didn't
        // specialize) — deopt to the VM. Shouldn't arise once arith dispatches by
        // operand type, but kept sound. (Dead block after the unconditional jump.)
        Op::Float(_) => {
            let __dr = b.ins().iconst(types::I32, 23);
            b.ins().jump(f.deopt, &[BlockArg::Value(__dr)]);
            let dead = b.create_block();
            b.switch_to_block(dead);
            b.ins().iconst(types::I64, 0)
        }
    }
}

/// Materialise an operand as a block argument. Block params are declared `I64`
/// (see `leader_block`), but a comparison result is an `i8`; passing it raw would
/// be an `I8`-into-`I64`-param type mismatch the Cranelift verifier rejects, which
/// bailed *every* arm that carried a comparison across a block boundary — i.e. every
/// `(and …)`/`(or …)` (they short-circuit a bool through a merge). Zero-extend the
/// `i8` (0/1 → bool); the target reconstructs it as `Op::Bool` via the `bool_param`
/// flag recorded at this jump, so it branches with correct Brood truthiness. Every
/// other `as_int` result is already `i64`.
pub(super) fn as_block_arg(
    b: &mut FunctionBuilder,
    op: Op,
    idx: usize,
    f: Frame,
) -> cranelift_codegen::ir::Value {
    // A slot proven to hold a `Value::Bool` (`slot_bool`): load its payload byte (0/1)
    // as the i64 arg — the target reconstructs `Op::Bool` via the `bool_param` flag
    // (`is_bool_op` is true for it too, so every predecessor agrees). `as_int` would
    // instead tag-check `Int` and deopt on the `Bool`.
    if let Op::Slot(k) = op {
        if f.slot_bool.borrow().get(k).copied().unwrap_or(false) {
            let roots_base = b.use_var(f.rb_var);
            let i = b.ins().iadd_imm_s(f.base, k as i64);
            let o = b.ins().imul_imm_s(i, STRIDE);
            let addr = b.ins().iadd(roots_base, o);
            let pl = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32,
            );
            return b.ins().band_imm_s(pl, 0xff);
        }
    }
    // KI-49: an operand crossing as `ParamRepr::Slot` is NOT materialised — the target
    // rebuilds `Op::Slot(k)` and reads the frame when it needs the value. Forcing it
    // through `as_int` here is what deopted every tagged-tuple matcher.
    if let ParamRepr::Slot(k) = param_repr(b, op, idx, f) {
        // A Handle has to be materialised INTO the slot first; a Slot operand already
        // lives in one (and `k` is that same slot, so this would be a self-copy).
        if matches!(op, Op::Handle(..)) {
            store_op(b, k as i64, op, f);
        }
        return b.ins().iconst(types::I64, 0);
    }
    // A bool crossing the edge is legitimate (an `(and …)`/`(or …)` short-circuits its bound
    // operand through the merge) and must NOT go through `as_int`, which deopts on a boolean
    // operand: the target reconstructs `Op::Bool` from the `bool_param` flag, so what crosses
    // is the 0/1 word, not a number. `param_repr` already typed this edge `ParamRepr::Bool`.
    match op {
        Op::Bool(v) => return v, // already an i64 0/1
        Op::Int(v) if b.func.dfg.value_type(v) == types::I8 => {
            return b.ins().uextend(types::I64, v)
        }
        _ => {}
    }
    let v = as_int(b, op, f);
    if b.func.dfg.value_type(v) == types::I8 {
        b.ins().uextend(types::I64, v)
    } else {
        v
    }
}

/// Materialise an operand to an unboxed `f64`. A `Slot` is normally tag-checked `==
/// Float` and its payload bit-cast to `f64`. Two fast paths, applied in order:
///
/// 1. Float-carry slots (0..carry_argc, profiled Int/Float): `use_var` — no tag-check,
///    no memory access, just the phi-propagated SSA value.
/// 2. F64 SSA cache: `store_op(Float(v))` stashes `v` in `slot_f64_cache`; subsequent
///    reads in the same block return it directly. Eliminates the store→load→bitcast
///    round-trip for let-bound floats (e.g. `nx`/`ny` in mandelbrot's `esc` inner loop,
///    where `(* nx nx)` would otherwise reload and tag-check the just-written slot).
///    The cache is valid only for slots written via `store_op` (never via SelfCall/entry),
///    and parameter slots are always None — safe against cross-branch pollution.
/// 3. Unknown: tag-check; `Float` → load + bitcast, `Int` → load + `fcvt_from_sint` (the
///    VM's own promotion), else deopt ([`float_or_promoted_int`]). NOTE: we do NOT skip the
///    tag-check based on `slot_float[k]` alone: that flag is a single-pass approximation
///    that can be contaminated by stores in other branches (e.g. a then-branch `store_op`
///    setting slot_float[k]=true before an else-branch `as_f64` read — the slot is really
///    Int at that point). Skipping the brif deopt there produces wrong results.
/// A value read *as a float*: `Float` → its bits; **`Int` → promoted with `fcvt_from_sint`**,
/// the same `i64 as f64` the VM's `prim_apply_float` performs; anything else → deopt with
/// `reason`. `tag` is the low byte already isolated; `payload` the second word.
///
/// The `Int` arm is the fix for a whole class of arms that could never stay native: any
/// float-context body applied to an int — `->float` is literally `(* 1.0 x)`, and every
/// program that converts calls it with an int. The old guard accepted `Float` alone, so the
/// arm deopted on EVERY activation, sixteen in a row latched it BAILED, and it ran
/// interpreted for the rest of the process (`mandelbrot`: one VM call per pixel, KI-109).
/// Promoting here is not a guess about types; it is the VM's own semantics for a mixed
/// operand, so a stale profile costs nothing and a `BigInt` (a different tag) still deopts
/// to the native that owns it.
fn float_or_promoted_int(
    b: &mut FunctionBuilder,
    tag: cranelift_codegen::ir::Value,
    payload: cranelift_codegen::ir::Value,
    f: Frame,
    reason: i64,
) -> cranelift_codegen::ir::Value {
    let is_f = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_FLOAT as i64);
    let is_i = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_INT as i64);
    let as_float = b.create_block();
    let not_float = b.create_block();
    let as_int = b.create_block();
    let merge = b.create_block();
    b.append_block_param(merge, types::F64);
    b.ins().brif(is_f, as_float, &[], not_float, &[]);
    b.switch_to_block(as_float);
    let bits = b.ins().bitcast(types::F64, MemFlagsData::new(), payload);
    b.ins().jump(merge, &[BlockArg::Value(bits)]);
    b.switch_to_block(not_float);
    let __dr = b.ins().iconst(types::I32, reason);
    b.ins()
        .brif(is_i, as_int, &[], f.deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(as_int);
    let promoted = b.ins().fcvt_from_sint(types::F64, payload);
    b.ins().jump(merge, &[BlockArg::Value(promoted)]);
    b.switch_to_block(merge);
    b.block_params(merge)[0]
}

pub(super) fn as_f64(b: &mut FunctionBuilder, op: Op, f: Frame) -> cranelift_codegen::ir::Value {
    match op {
        Op::Float(v) => v,
        Op::Slot(k) => {
            if let Some((var, true)) = f.carry_vars.get(k).copied().flatten() {
                return b.use_var(var);
            }
            if let Some(v) = f.slot_f64_cache.borrow().get(k).copied().flatten() {
                return v;
            }
            let roots_base = b.use_var(f.rb_var);
            let i = b.ins().iadd_imm_s(f.base, k as i64);
            let o = b.ins().imul_imm_s(i, STRIDE);
            let addr = b.ins().iadd(roots_base, o);
            let tag = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
            let payload = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                addr,
                PAYLOAD_OFFSET as i32,
            );
            float_or_promoted_int(b, tag, payload, f, 24)
        }
        Op::Handle(w0, w1, _) => {
            // A type-erased boxed `Value` (a `nth`/vector read, a call result) used as a
            // float: tag-check `Float` and extract its payload bits, else deopt (the VM
            // then runs the arm with the real type). Mirrors the `Op::Slot` path but on
            // words already in registers. This is what lets `(nth v k)`-fed float
            // arithmetic stay native instead of deopting on the int-path `as_int`.
            let tagb = b.ins().band_imm_s(w0, 0xff);
            float_or_promoted_int(b, tagb, w1, f, 25)
        }
        Op::Int(_) | Op::Bool(_) | Op::HoistedVec { .. } | Op::HoistedTable { .. } => {
            let __dr = b.ins().iconst(types::I32, 26);
            b.ins().jump(f.deopt, &[BlockArg::Value(__dr)]);
            let dead = b.create_block();
            b.switch_to_block(dead);
            b.ins().f64const(0.0)
        }
    }
}

/// Store an operand into frame slot `dst`: an `Int` is boxed; a `Slot` is copied
/// verbatim; a `Handle`/hoisted value stores its three words (so a handle binder /
/// self-call arg / return keeps its type).
///
/// Also tracks the per-slot type flags so a later read of `dst` picks the right
/// representation: a float store marks `slot_float`, an int/handle store clears it,
/// a slot-copy inherits the source's flags; the comparison-`i8` case marks
/// `slot_bool` (block-arg representation). The `slot_f64_cache` is updated in lock
/// step — a float store caches the SSA `f64`, every other store invalidates.
/// Record what kind of value now sits in slot `dst`, so a later read of it picks the right
/// representation. Pure metadata — emits no code. Extracted from [`store_op`] so the return
/// exit ([`store_result`]) can keep the identical bookkeeping without restating the per-`Op`
/// mapping; two copies of this mapping would be a silent-divergence hazard.
pub(super) fn set_slot_flags(b: &FunctionBuilder, dst: i64, op: Op, f: Frame) {
    let (is_float, is_bool, f64v) = match op {
        // A comparison `i8` (`store_int`/`box_scalar` boxes it as `Value::Bool`) marks the
        // slot bool; a real `i64` int does not.
        Op::Int(v) => (false, b.func.dfg.value_type(v) == types::I8, None),
        Op::Float(v) => (true, false, Some(v)),
        Op::Bool(_) => (false, true, None),
        Op::Slot(k) => (
            f.slot_float.borrow().get(k).copied().unwrap_or(false),
            f.slot_bool.borrow().get(k).copied().unwrap_or(false),
            f.slot_f64_cache.borrow().get(k).copied().flatten(),
        ),
        Op::Handle(..) | Op::HoistedVec { .. } | Op::HoistedTable { .. } => (false, false, None),
    };
    set_slot_float(dst, is_float, f);
    set_slot_bool(dst, is_bool, f);
    if let Some(s) = f.slot_f64_cache.borrow_mut().get_mut(dst as usize) {
        *s = f64v;
    }
}

/// The arm's **return** store: write the single result `Value` to `addr` as one 16-byte
/// vector store of `[tag, payload]` plus an 8-byte store of the third word.
///
/// `addr` is the caller-supplied `out` pointer, not a frame slot. Returning through
/// `roots[base]` and having `jit_run_fast_link` load it straight back cost **16.4% of that
/// function** on a single `movups` — measured with `cycles:pp`, so it is the load and not
/// skid from the `callq` before it (docs/compute-frontier.md §2h).
///
/// **The obvious explanation is wrong, and it was tested.** A 16-byte load straddling
/// `store_int`'s 1-byte tag + 8-byte payload cannot store-forward, which predicts exactly
/// this symptom — but widening the store to a single 16-byte vector store while still going
/// through `roots[base]` left the instruction at 16.4% (from 16.4%) and bought 1.3% of the
/// row, against a 1.0% floor. So the cost is the memory round trip itself, not the width
/// mismatch. The wide store is kept because it matches the consumer's width for free.
pub(super) fn store_result(
    b: &mut FunctionBuilder,
    op: Op,
    addr: cranelift_codegen::ir::Value,
    f: Frame,
) {
    let [w0, w1, w2] = read_words(b, op, f);
    // An `I64X2` vector store, NOT `iconcat` + `store.i128`: Cranelift's x64 backend keeps
    // an `i128` in a GPR *pair*, so that form legalizes back into two 8-byte `mov`s and the
    // load still straddles them (measured: no change). A vector store is one `movups`.
    // Lane 0 lands at +0 and lane 1 at PAYLOAD_OFFSET on little-endian, which is the layout
    // `repr(C, u8)` gives every `Value` (see `jit_layout`).
    let v0 = b.ins().scalar_to_vector(types::I64X2, w0);
    let pair = b.ins().insertlane(v0, w1, 1);
    b.ins().store(MemFlagsData::trusted(), pair, addr, 0);
    b.ins()
        .store(MemFlagsData::trusted(), w2, addr, PAYLOAD_OFFSET as i32 + 8);
}

pub(super) fn store_op(b: &mut FunctionBuilder, dst: i64, op: Op, f: Frame) {
    match op {
        Op::Int(v) => store_int(b, dst, v, f),
        Op::Float(v) => {
            let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), v);
            let tag = b.ins().iconst(types::I64, TAG_FLOAT as i64);
            let zero = b.ins().iconst(types::I64, 0);
            store_words(b, dst, [tag, bits, zero], f);
        }
        Op::Bool(v) => {
            let tag = b.ins().iconst(types::I64, TAG_BOOL as i64);
            let zero = b.ins().iconst(types::I64, 0);
            store_words(b, dst, [tag, v, zero], f);
        }
        Op::Slot(k) => copy_value(b, k as i64, dst, f),
        Op::Handle(w0, w1, w2)
        | Op::HoistedVec { w0, w1, w2, .. }
        | Op::HoistedTable { w0, w1, w2, .. } => {
            // A hoisted global vector/table used as a whole `Value` stores its
            // entry-resolved words verbatim, exactly like a `Handle`.
            store_words(b, dst, [w0, w1, w2], f)
        }
    }
    set_slot_flags(b, dst, op, f);
}

/// Call a handle op (`brood_rt_{cons,car,cdr}`) with the out-pointer ABI: pass the
/// scratch slot's address + the operand words, then read the result `Value`'s three
/// words back into a `Handle`. The result rides in registers only until it's consumed
/// (a store / return) — no safepoint in between — so the GC never sees it.
pub(super) fn call_handle(
    b: &mut FunctionBuilder,
    fref: FuncRef,
    operands: &[cranelift_codegen::ir::Value],
    fu: Funcs,
) -> Op {
    let out_addr = b.ins().stack_addr(fu.ptr_ty, fu.out_slot, 0);
    let mut args = Vec::with_capacity(operands.len() + 2);
    args.push(fu.heap);
    args.push(out_addr);
    args.extend_from_slice(operands);
    b.ins().call(fref, &args);
    let w0 = b.ins().stack_load(types::I64, types::I64, fu.out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, types::I64, fu.out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b.ins().stack_load(
        types::I64,
        types::I64,
        fu.out_slot,
        PAYLOAD_OFFSET as i32 + 8,
    );
    Op::Handle(w0, w1, w2)
}

/// Dynamic-index vector read, fully inline for a LOCAL vector (either storage):
/// tag/region/int-index checks → slab slot → inline or spill element read — no
/// FFI on the hot path (this was ~20 ns/element on the json/regex code-vector
/// scans). Anything else — non-vector, non-LOCAL region (RUNTIME/PRELUDE, e.g.
/// matmul's def'd rows), non-int index, out-of-range — falls back to the
/// `brood_rt_vector_ref` FFI, whose nonzero status deopts (the VM owns `nth`'s
/// exact result and errors).
pub(super) fn vector_ref(
    b: &mut FunctionBuilder,
    vec: [cranelift_codegen::ir::Value; 3],
    idx: [cranelift_codegen::ir::Value; 3],
    f: Frame,
    fu: Funcs,
) -> Op {
    let vr_done = b.create_block();
    b.append_block_param(vr_done, types::I64);
    b.append_block_param(vr_done, types::I64);
    b.append_block_param(vr_done, types::I64);
    let ffi_blk = b.create_block();
    // tag byte must be Vector.
    let tagb = b.ins().band_imm_s(vec[0], 0xff);
    let is_vec = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_VECTOR as i64);
    let c1 = b.create_block();
    b.ins().brif(is_vec, c1, &[], ffi_blk, &[]);
    b.switch_to_block(c1);
    // region: high 2 bits of the handle == 0 (LOCAL); RUNTIME/PRELUDE → FFI.
    let high2 = b.ins().ushr_imm_s(vec[1], 62);
    let is_local = b.ins().icmp_imm_s(IntCC::Equal, high2, 0);
    let c2 = b.create_block();
    b.ins().brif(is_local, c2, &[], ffi_blk, &[]);
    b.switch_to_block(c2);
    // index must be an Int.
    let itag = b.ins().band_imm_s(idx[0], 0xff);
    let is_int = b.ins().icmp_imm_s(IntCC::Equal, itag, TAG_INT as i64);
    let c3 = b.create_block();
    b.ins().brif(is_int, c3, &[], ffi_blk, &[]);
    b.switch_to_block(c3);
    let idxv = idx[1];
    // age bit 61 selects the slab base (fetched per read, like the const-index
    // inline — safe across any prior safepoint).
    let age = b.ins().ushr_imm_s(vec[1], 61);
    let is_old = b.ins().icmp_imm_s(IntCC::NotEqual, age, 0);
    let nb2 = b.create_block();
    let ob2 = b.create_block();
    let based = b.create_block();
    b.append_block_param(based, fu.ptr_ty);
    b.ins().brif(is_old, ob2, &[], nb2, &[]);
    b.switch_to_block(nb2);
    let cn2 = b.ins().call(fu.vnbase, &[fu.heap]);
    let bn2 = b.inst_results(cn2)[0];
    b.ins().jump(based, &[BlockArg::Value(bn2)]);
    b.switch_to_block(ob2);
    let co2 = b.ins().call(fu.vobase, &[fu.heap]);
    let bo2 = b.inst_results(co2)[0];
    b.ins().jump(based, &[BlockArg::Value(bo2)]);
    b.switch_to_block(based);
    let sbase = b.block_params(based)[0];
    let vidx = b.ins().band_imm_s(vec[1], 0xFFFF_FFFFi64);
    let soff = b.ins().imul_imm_s(vidx, VS::JIT_STRIDE);
    let slotp = b.ins().iadd(sbase, soff);
    let disc = b
        .ins()
        .load(types::I8, MemFlagsData::trusted(), slotp, VS::JIT_TAG_OFF);
    let is_inline = b.ins().icmp_imm_s(IntCC::Equal, disc, VS::JIT_INLINE_TAG);
    let inl = b.create_block();
    let not_inl = b.create_block();
    b.ins().brif(is_inline, inl, &[], not_inl, &[]);
    // Inline storage: bounds vs the u8 len, elements at JIT_ITEMS_OFF.
    b.switch_to_block(inl);
    let lenb = b
        .ins()
        .load(types::I8, MemFlagsData::trusted(), slotp, VS::JIT_LEN_OFF);
    let lenw = b.ins().uextend(types::I64, lenb);
    let ib = b.ins().icmp(IntCC::UnsignedLessThan, idxv, lenw);
    let iok = b.create_block();
    b.ins().brif(ib, iok, &[], ffi_blk, &[]);
    b.switch_to_block(iok);
    let eo = b.ins().imul_imm_s(idxv, STRIDE);
    let ebase = b.ins().iadd_imm_s(slotp, VS::JIT_ITEMS_OFF as i64);
    let ep = b.ins().iadd(ebase, eo);
    let i0 = b.ins().load(types::I64, MemFlagsData::trusted(), ep, 0);
    let i1 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        ep,
        PAYLOAD_OFFSET as i32,
    );
    let i2 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        ep,
        PAYLOAD_OFFSET as i32 + 8,
    );
    b.ins().jump(
        vr_done,
        &[
            BlockArg::Value(i0),
            BlockArg::Value(i1),
            BlockArg::Value(i2),
        ],
    );
    // Spill storage: bounds vs the cached len, elements via the cached ptr.
    b.switch_to_block(not_inl);
    let is_spill = b.ins().icmp_imm_s(IntCC::Equal, disc, VS::JIT_SPILL_TAG);
    let spl = b.create_block();
    b.ins().brif(is_spill, spl, &[], ffi_blk, &[]);
    b.switch_to_block(spl);
    let sptr = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slotp,
        VS::JIT_SPILL_PTR_OFF,
    );
    let slen = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slotp,
        VS::JIT_SPILL_LEN_OFF,
    );
    let sb2 = b.ins().icmp(IntCC::UnsignedLessThan, idxv, slen);
    let sok2 = b.create_block();
    b.ins().brif(sb2, sok2, &[], ffi_blk, &[]);
    b.switch_to_block(sok2);
    let seo = b.ins().imul_imm_s(idxv, STRIDE);
    let sep = b.ins().iadd(sptr, seo);
    let s0 = b.ins().load(types::I64, MemFlagsData::trusted(), sep, 0);
    let s1 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        sep,
        PAYLOAD_OFFSET as i32,
    );
    let s2 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        sep,
        PAYLOAD_OFFSET as i32 + 8,
    );
    b.ins().jump(
        vr_done,
        &[
            BlockArg::Value(s0),
            BlockArg::Value(s1),
            BlockArg::Value(s2),
        ],
    );
    // FFI fallback: exact semantics for every non-inlined shape; status → deopt.
    b.switch_to_block(ffi_blk);
    let out_addr = b.ins().stack_addr(fu.ptr_ty, fu.out_slot, 0);
    let c = b.ins().call(
        fu.vref,
        &[
            fu.heap, out_addr, vec[0], vec[1], vec[2], idx[0], idx[1], idx[2],
        ],
    );
    let status = b.inst_results(c)[0];
    let cont = b.create_block();
    let __dr = b.ins().iconst(types::I32, 27);
    b.ins()
        .brif(status, f.deopt, &[BlockArg::Value(__dr)], cont, &[]);
    b.switch_to_block(cont);
    let w0 = b.ins().stack_load(types::I64, types::I64, fu.out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, types::I64, fu.out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b.ins().stack_load(
        types::I64,
        types::I64,
        fu.out_slot,
        PAYLOAD_OFFSET as i32 + 8,
    );
    b.ins().jump(
        vr_done,
        &[
            BlockArg::Value(w0),
            BlockArg::Value(w1),
            BlockArg::Value(w2),
        ],
    );
    b.switch_to_block(vr_done);
    let r0 = b.block_params(vr_done)[0];
    let r1 = b.block_params(vr_done)[1];
    let r2 = b.block_params(vr_done)[2];
    Op::Handle(r0, r1, r2)
}

/// `table-has?` / 2-arg `table-get` via their runtime callbacks. Status protocol:
/// 0 = done (`out` holds the result), 1 = deopt (non-Table operand — the VM owns the
/// exact type error), 2 = a real error is parked in `jit_pending_error` (dropped
/// table / bad key) → exit via the arm's error block (outcome 3). The callbacks may
/// allocate (a compound stored value reconstructs) but never collect, so live
/// register handles stay valid across the call.
pub(super) fn table_prim(
    b: &mut FunctionBuilder,
    fref: FuncRef,
    tbl: [cranelift_codegen::ir::Value; 3],
    key: [cranelift_codegen::ir::Value; 3],
    f: Frame,
    fu: Funcs,
) -> Op {
    let out_addr = b.ins().stack_addr(fu.ptr_ty, fu.out_slot, 0);
    let c = b.ins().call(
        fref,
        &[
            fu.heap, out_addr, tbl[0], tbl[1], tbl[2], key[0], key[1], key[2],
        ],
    );
    let status = b.inst_results(c)[0];
    let cont = b.create_block();
    let slow = b.create_block();
    b.ins().brif(status, slow, &[], cont, &[]);
    b.switch_to_block(slow);
    let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
    let __dr = b.ins().iconst(types::I32, 28);
    b.ins()
        .brif(is_err, fu.error, &[], f.deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(cont);
    let w0 = b.ins().stack_load(types::I64, types::I64, fu.out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, types::I64, fu.out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b.ins().stack_load(
        types::I64,
        types::I64,
        fu.out_slot,
        PAYLOAD_OFFSET as i32 + 8,
    );
    Op::Handle(w0, w1, w2)
}

/// Runtime-dispatched `=` on materialised operands — the codegen twin of the VM's
/// keyword/symbol fast path in `prim2_inline_exec`. Cases, by runtime tags:
///   * Int × Int → payload compare (the same two tag-checks the old int path paid);
///   * either side Sym/Keyword → interned identity: equal iff tags equal AND ids
///     equal (a keyword/symbol equals nothing but its same-tag same-id self — never
///     numerically coerced, so `(= :a 1)`/`(= :a 'a)` are correctly false);
///   * anything else (floats, bignums, structural values) → deopt: the VM owns
///     numeric coercion and deep equality.
/// This is what keeps keyword-dispatching arms (`(= (get st :t) :split)` — the regex
/// NFA walkers, any tagged-map code) running native instead of deopting per compare.
pub(super) fn eq_dispatch(
    b: &mut FunctionBuilder,
    wa: [cranelift_codegen::ir::Value; 3],
    wb: [cranelift_codegen::ir::Value; 3],
    f: Frame,
) -> cranelift_codegen::ir::Value {
    let ta = b.ins().band_imm_s(wa[0], 0xff);
    let tb = b.ins().band_imm_s(wb[0], 0xff);
    let done = b.create_block();
    b.append_block_param(done, types::I8);
    // Int × Int?
    let a_int = b.ins().icmp_imm_s(IntCC::Equal, ta, TAG_INT as i64);
    let b_int = b.ins().icmp_imm_s(IntCC::Equal, tb, TAG_INT as i64);
    let both_int = b.ins().band(a_int, b_int);
    let intb = b.create_block();
    let not_int = b.create_block();
    b.ins().brif(both_int, intb, &[], not_int, &[]);
    b.switch_to_block(intb);
    let ieq = b.ins().icmp(IntCC::Equal, wa[1], wb[1]);
    b.ins().jump(done, &[BlockArg::Value(ieq)]);
    // Either side an interned immediate (Sym=5 / Keyword=6)?
    b.switch_to_block(not_int);
    let a_sym = b.ins().icmp_imm_s(IntCC::Equal, ta, TAG_SYM as i64);
    let a_kw = b.ins().icmp_imm_s(IntCC::Equal, ta, TAG_KEYWORD as i64);
    let b_sym = b.ins().icmp_imm_s(IntCC::Equal, tb, TAG_SYM as i64);
    let b_kw = b.ins().icmp_imm_s(IntCC::Equal, tb, TAG_KEYWORD as i64);
    let a_in = b.ins().bor(a_sym, a_kw);
    let b_in = b.ins().bor(b_sym, b_kw);
    let either = b.ins().bor(a_in, b_in);
    let kwb = b.create_block();
    let __dr = b.ins().iconst(types::I32, 29);
    b.ins()
        .brif(either, kwb, &[], f.deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(kwb);
    let tags_eq = b.ins().icmp(IntCC::Equal, ta, tb);
    // A Sym/Keyword payload is a u32 — the HIGH half of the payload word is
    // undefined padding (Rust doesn't zero it, and word-copies carry it along),
    // so compare only the low 32 bits or equal interned ids can compare unequal.
    let ida = b.ins().band_imm_s(wa[1], 0xFFFF_FFFFi64);
    let idb = b.ins().band_imm_s(wb[1], 0xFFFF_FFFFi64);
    let ids_eq = b.ins().icmp(IntCC::Equal, ida, idb);
    let keq = b.ins().band(tags_eq, ids_eq);
    b.ins().jump(done, &[BlockArg::Value(keq)]);
    b.switch_to_block(done);
    b.block_params(done)[0]
}

/// Inline read of `(nth v <const idx>)` for a LOCAL small (inline) vector, the analog of
/// the pair `first`/`rest` inline. Fetches the vector-slab base *per read* (a trivial
/// FFI, not the hoist used for pairs) so it is safe even in arms with GC safepoints (a
/// non-tail `Call` between reads) — `bintree`'s `check` is exactly that. Any slow
/// condition (not a `Vector`, non-LOCAL region, spilled/large vector, or out-of-range
/// index) deopts to the VM, which produces `nth`'s exact result. Element read is `slot +
/// JIT_ITEMS_OFF + idx*STRIDE`; `vec` is the handle word-triple, `idx` a compile-time
/// index.
pub(super) fn inline_vec_ref(
    b: &mut FunctionBuilder,
    vec: [cranelift_codegen::ir::Value; 3],
    idx: i64,
    frame: Frame,
    funcs: Funcs,
) -> Op {
    let deopt = frame.deopt;
    let ptr_ty = funcs.ptr_ty;
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    let w0 = vec[0];
    let w1 = vec[1];
    // Tag byte must be Vector (Range/SeqView share the slab but tag differently).
    let tagb = b.ins().band_imm_s(w0, 0xff);
    let is_vec = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_VECTOR as i64);
    let c1 = b.create_block();
    let __dr = b.ins().iconst(types::I32, 30);
    b.ins()
        .brif(is_vec, c1, &[], deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(c1);
    // Region: high 2 bits of the handle == 0 (LOCAL). Deopt for PRELUDE/RUNTIME.
    let high2 = b.ins().ushr_imm_s(w1, 62);
    let is_local = b.ins().icmp_imm_s(IntCC::Equal, high2, 0);
    let c2 = b.create_block();
    let __dr = b.ins().iconst(types::I32, 31);
    b.ins()
        .brif(is_local, c2, &[], deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(c2);
    // Age bit 61 (0=nursery, 1=old) selects which slab base to fetch. Fetch it per-read
    // so a prior safepoint that moved the slab can't leave it stale.
    let age = b.ins().ushr_imm_s(w1, 61);
    let is_old = b.ins().icmp_imm_s(IntCC::NotEqual, age, 0);
    let nb = b.create_block();
    let ob = b.create_block();
    let merge = b.create_block();
    b.append_block_param(merge, ptr_ty);
    b.ins().brif(is_old, ob, &[], nb, &[]);
    b.switch_to_block(nb);
    let cn = b.ins().call(funcs.vnbase, &[heap]);
    let bn = b.inst_results(cn)[0];
    b.ins().jump(merge, &[BlockArg::Value(bn)]);
    b.switch_to_block(ob);
    let co = b.ins().call(funcs.vobase, &[heap]);
    let bo = b.inst_results(co)[0];
    b.ins().jump(merge, &[BlockArg::Value(bo)]);
    b.switch_to_block(merge);
    let base = b.block_params(merge)[0];
    // Slot pointer: base + slab_index * stride. slab_index = low 32 bits.
    let vidx = b.ins().band_imm_s(w1, 0xFFFF_FFFFi64);
    let slot_off = b.ins().imul_imm_s(vidx, VS::JIT_STRIDE);
    let slot_ptr = b.ins().iadd(base, slot_off);
    // Discriminant byte must be `Inline` (spilled/large vectors deopt).
    let disc = b.ins().load(
        types::I8,
        MemFlagsData::trusted(),
        slot_ptr,
        VS::JIT_TAG_OFF,
    );
    let is_inline = b.ins().icmp_imm_s(IntCC::Equal, disc, VS::JIT_INLINE_TAG);
    let inline_blk = b.create_block();
    let heap_blk = b.create_block();
    // The two storage layouts converge here with the element's 3 words.
    let ivr_done = b.create_block();
    b.append_block_param(ivr_done, types::I64);
    b.append_block_param(ivr_done, types::I64);
    b.append_block_param(ivr_done, types::I64);
    b.ins().brif(is_inline, inline_blk, &[], heap_blk, &[]);
    // Inline (`INLINE_VEC_CAP`-or-fewer elements): read straight from the slab slot.
    b.switch_to_block(inline_blk);
    // Bounds: idx < len (len is the inline element count, a u8).
    let lenb = b.ins().load(
        types::I8,
        MemFlagsData::trusted(),
        slot_ptr,
        VS::JIT_LEN_OFF,
    );
    let lenw = b.ins().uextend(types::I64, lenb);
    let idxc = b.ins().iconst(types::I64, idx);
    let in_bounds = b.ins().icmp(IntCC::UnsignedLessThan, idxc, lenw);
    let c4 = b.create_block();
    let __dr = b.ins().iconst(types::I32, 32);
    b.ins()
        .brif(in_bounds, c4, &[], deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(c4);
    // Element read: slot_ptr + JIT_ITEMS_OFF + idx*size_of::<Value>().
    let elem_off = VS::JIT_ITEMS_OFF as i64 + idx * STRIDE;
    let elem = b.ins().iadd_imm_s(slot_ptr, elem_off);
    let r0 = b.ins().load(types::I64, MemFlagsData::trusted(), elem, 0);
    let r1 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        elem,
        PAYLOAD_OFFSET as i32,
    );
    let r2 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        elem,
        PAYLOAD_OFFSET as i32 + 8,
    );
    b.ins().jump(
        ivr_done,
        &[
            BlockArg::Value(r0),
            BlockArg::Value(r1),
            BlockArg::Value(r2),
        ],
    );
    // Heap-backed (a >`INLINE_VEC_CAP` vector — e.g. nbody's 7-element body vectors):
    // read straight through the spill store's CACHED buffer pointer
    // (`VecStore::Spill{ptr,len,..}` — `#[repr(u8)]`-pinned, layout-tested). This
    // replaces the ~20 ns `brood_rt_vector_ref` FFI per field read with two loads + a
    // bounds check. Out-of-range (or an unexpected disc) deopts — the VM owns `nth`'s
    // exact result.
    b.switch_to_block(heap_blk);
    let is_spill = b.ins().icmp_imm_s(IntCC::Equal, disc, VS::JIT_SPILL_TAG);
    let spill_blk = b.create_block();
    let __dr = b.ins().iconst(types::I32, 33);
    b.ins()
        .brif(is_spill, spill_blk, &[], deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(spill_blk);
    let sptr = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slot_ptr,
        VS::JIT_SPILL_PTR_OFF,
    );
    let slen = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        slot_ptr,
        VS::JIT_SPILL_LEN_OFF,
    );
    let idxc2 = b.ins().iconst(types::I64, idx);
    let in_b = b.ins().icmp(IntCC::UnsignedLessThan, idxc2, slen);
    let sok = b.create_block();
    let __dr = b.ins().iconst(types::I32, 34);
    b.ins()
        .brif(in_b, sok, &[], deopt, &[BlockArg::Value(__dr)]);
    b.switch_to_block(sok);
    let elem2 = b.ins().iadd_imm_s(sptr, idx * STRIDE);
    let s0 = b.ins().load(types::I64, MemFlagsData::trusted(), elem2, 0);
    let s1 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        elem2,
        PAYLOAD_OFFSET as i32,
    );
    let s2 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        elem2,
        PAYLOAD_OFFSET as i32 + 8,
    );
    b.ins().jump(
        ivr_done,
        &[
            BlockArg::Value(s0),
            BlockArg::Value(s1),
            BlockArg::Value(s2),
        ],
    );
    let dead_ffi = b.create_block();
    b.switch_to_block(dead_ffi);
    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
    let it = b.ins().iconst(types::I64, TAG_INT as i64);
    let iv = b.ins().iconst(types::I64, idx);
    let iz = b.ins().iconst(types::I64, 0);
    let hc = b.ins().call(
        funcs.vref,
        &[heap, out_addr, vec[0], vec[1], vec[2], it, iv, iz],
    );
    let hstatus = b.inst_results(hc)[0];
    let hok = b.create_block();
    let __dr = b.ins().iconst(types::I32, 35);
    b.ins()
        .brif(hstatus, deopt, &[BlockArg::Value(__dr)], hok, &[]);
    b.switch_to_block(hok);
    let h0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
    let h1 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
    let h2 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
    b.ins().jump(
        ivr_done,
        &[
            BlockArg::Value(h0),
            BlockArg::Value(h1),
            BlockArg::Value(h2),
        ],
    );
    b.switch_to_block(ivr_done);
    let w0 = b.block_params(ivr_done)[0];
    let w1 = b.block_params(ivr_done)[1];
    let w2 = b.block_params(ivr_done)[2];
    Op::Handle(w0, w1, w2)
}
