//! CLIF emit helpers for `jit_lower_arm_inner`, extracted from the closures that
//! took `b: &mut FunctionBuilder` and captured the `deopt` block. Part of the
//! `jit_lower_arm_inner` decomposition; jit-only. `Op` is the shared operand model
//! (module scope in `jit_lower.rs`).
#![cfg(feature = "jit")]
use super::*;
use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder};
use cranelift_frontend::FunctionBuilder;

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
