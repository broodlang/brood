//! CLIF emit helpers for `jit_lower_arm_inner`, extracted from the closures that
//! took `b: &mut FunctionBuilder` and captured the `deopt` block. Part of the
//! `jit_lower_arm_inner` decomposition; jit-only. `Op` is the shared operand model
//! (module scope in `jit_lower.rs`).
#![cfg(feature = "jit")]
use super::*;
use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL, TAG_INT};
use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder, MemFlagsData};
use cranelift_frontend::{FunctionBuilder, Variable};

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// The frame-access context the slot helpers need: the `roots` base variable, the
/// arm's frame base offset, the slot count, the shared `deopt` block, and the
/// register-carried param-slot table. All `Copy`, so it threads freely.
#[derive(Clone, Copy)]
pub(super) struct Frame<'a> {
    pub rb_var: Variable,
    pub base: cranelift_codegen::ir::Value,
    pub nslots: usize,
    pub deopt: cranelift_codegen::ir::Block,
    pub carry_vars: &'a [Option<(Variable, bool)>],
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
            b.ins().brif(ov, deopt, &[], cont, &[]);
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
                b.ins().brif(div0, deopt, &[], c0, &[]);
                b.switch_to_block(c0);
                // (x == i64::MIN) && (y == -1) — the one signed-division overflow.
                let min = b.ins().iconst(types::I64, i64::MIN);
                let neg1 = b.ins().iconst(types::I64, -1);
                let x_min = b.ins().icmp(IntCC::Equal, x, min);
                let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
                let ov = b.ins().band(x_min, y_m1);
                let c1 = b.create_block();
                b.ins().brif(ov, deopt, &[], c1, &[]);
                b.switch_to_block(c1);
                match op {
                    PrimOp::Rem => b.ins().srem(x, y),
                    PrimOp::Quot => b.ins().sdiv(x, y),
                    PrimOp::Div => {
                        // Exact only: a nonzero remainder → Float → deopt to the native.
                        let r = b.ins().srem(x, y);
                        let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                        let c2 = b.create_block();
                        b.ins().brif(inexact, deopt, &[], c2, &[]);
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
                b.ins().brif(is_zero, deopt, &[], cont, &[]);
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
pub(super) fn load_slot_int(b: &mut FunctionBuilder, k: i64, f: Frame) -> cranelift_codegen::ir::Value {
        if let Some((var, false)) = f.carry_vars.get(k as usize).copied().flatten() {
            return b.use_var(var);
        }
        let roots_base = b.use_var(f.rb_var);
        let idx = b.ins().iadd_imm(f.base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, off);
        let tag = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
        let is_int = b.ins().icmp_imm(IntCC::Equal, tag, TAG_INT as i64);
        let cont = b.create_block();
        b.ins().brif(is_int, cont, &[], f.deopt, &[]);
        b.switch_to_block(cont);
        b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            addr,
            PAYLOAD_OFFSET as i32,
        )
}

/// Store an unboxed scalar into frame slot `k`, boxing via [`box_scalar`].
pub(super) fn store_int(b: &mut FunctionBuilder, k: i64, v: cranelift_codegen::ir::Value, f: Frame) {
        debug_assert!(
            (k as usize) < f.nslots,
            "[jit-slot] store_int slot {} >= nslots {}",
            k,
            f.nslots
        );
        let (tag_byte, payload) = box_scalar(b, v);
        let roots_base = b.use_var(f.rb_var);
        let idx = b.ins().iadd_imm(f.base, k);
        let off = b.ins().imul_imm(idx, STRIDE);
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
            let i = b.ins().iadd_imm(f.base, src);
            let o = b.ins().imul_imm(i, STRIDE);
            b.ins().iadd(roots_base, o)
        };
        let daddr = {
            let i = b.ins().iadd_imm(f.base, dst);
            let o = b.ins().imul_imm(i, STRIDE);
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
