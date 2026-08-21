//! Control-flow arm bodies for `jit_lower_arm_inner`'s emit loop — the `Jump` and
//! `JumpIfFalse` terminators, plus the shared block-param edge-typing helper
//! (`record_block_flags`). Extracted from the emit loop as part of the
//! `jit_lower_arm_inner` decomposition; jit-only. Each arm still `break`s the
//! caller's inner loop (it's a block terminator) — these fns emit the terminator
//! and return `Some(())`, or `None` to bail the whole arm to the VM.
#![cfg(feature = "jit")]
use super::emit::{as_block_arg, param_repr, store_op, Frame, ParamRepr};
use super::Op;
use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL};
use cranelift_codegen::ir::{condcodes::IntCC, types, Block, BlockArg, InstBuilder, MemFlagsData};
use cranelift_frontend::FunctionBuilder;

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// Record an edge's per-entry bool-ness flags for its target block, returning whether
/// this edge AGREES with the typing the block already has. The first edge to reach a
/// join fixes the typing; a later edge whose flags differ must NOT jump there — a
/// single-i64 block param can't distinguish `Int 1` from `true`, so a type-mixed join
/// (e.g. `(if c 7 (< a b))` flowing into a call argument) would either box the int
/// edge's raw value as a `Value::Bool` (the `Bool(7)` staging miscompile) or strip the
/// bool edge to a raw truthy int, depending on which edge lowered last. The caller
/// routes a disagreeing edge to `deopt` instead — the VM runs that iteration with the
/// real tagged value, bit-identical.
pub(super) fn record_block_flags(slot: &mut Option<Vec<ParamRepr>>, flags: Vec<ParamRepr>) -> bool {
    match slot {
        None => {
            *slot = Some(flags);
            true
        }
        Some(prev) => *prev == flags,
    }
}

/// `Inst::Jump(t)` — an unconditional branch. `t == len` targets Done (return the
/// single result via `roots[base]`); otherwise it jumps to leader block `t`, typing
/// its operand-stack args. A dead jump (wrong stack height at Done) or a type-mixed
/// join routes to `deopt`. Returns `None` to bail the arm (`leader_block[t]` absent).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_jump(
    b: &mut FunctionBuilder,
    stack: &[Op],
    t: usize,
    len: usize,
    done_block: Block,
    leader_block: &[Option<Block>],
    bool_param: &mut [Option<Vec<ParamRepr>>],
    frame: Frame,
) -> Option<()> {
    let deopt = frame.deopt;
    if t == len {
        // Jump straight to Done: return the single result via roots[base].
        if stack.len() == 1 {
            store_op(b, 0, stack[0], frame);
            b.ins().jump(done_block, &[]);
        } else {
            // A reachable Done always leaves exactly one value, so a different stack
            // height here means this block is **dead** — the bytecode compiler emits a
            // jump-past-the-`else` after a branch that ended in a tail `SelfCall` (which
            // never falls through), so it can't run. Terminate it by routing to `deopt`:
            // never executes, and if the unreachability assumption were ever wrong it
            // safely falls back to the VM rather than mis-returning. (This dead jump is
            // why e.g. `collatz`'s `steps` arm wouldn't lower.)
            let __dr = b.ins().iconst(types::I32, 1);
            b.ins().jump(deopt, &[BlockArg::Value(__dr)]);
        }
    } else {
        let flags: Vec<ParamRepr> = stack
            .iter()
            .enumerate()
            .map(|(i, &op)| param_repr(b, op, i, frame))
            .collect();
        if record_block_flags(&mut bool_param[t], flags) {
            let args: Vec<BlockArg> = stack
                .iter()
                .enumerate()
                .map(|(i, &op)| BlockArg::Value(as_block_arg(b, op, i, frame)))
                .collect();
            b.ins().jump(leader_block[t]?, &args);
        } else {
            // Type-mixed join (see `record_block_flags`): this edge's scalar typing
            // disagrees with the block's — deopt to the VM.
            let __dr = b.ins().iconst(types::I32, 2);
            b.ins().jump(deopt, &[BlockArg::Value(__dr)]);
        }
    }
    Some(())
}

/// `Inst::JumpIfFalse(t)` — pop the condition and branch: falsy → leader `t`, truthy →
/// the fall-through leader `j + 1`. Types both edges' operand-stack args; a side whose
/// typing disagrees with its join routes to `deopt` (no args). The condition's shape
/// picks the branch form: an unboxed `i8`/`Bool` branches directly; a boxed
/// slot/handle loads the tag+payload and branches on Brood truthiness (only `nil` and
/// `false` falsy); a raw ambiguous `Op::Int` deopts; a float/vector is truthy.
pub(super) fn emit_jump_if_false(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    t: usize,
    j: usize,
    leader_block: &[Option<Block>],
    bool_param: &mut [Option<Vec<ParamRepr>>],
    frame: Frame,
) -> Option<()> {
    let deopt = frame.deopt;
    let cond = stack.pop()?;
    let flags: Vec<ParamRepr> = stack
        .iter()
        .enumerate()
        .map(|(i, &op)| param_repr(b, op, i, frame))
        .collect();
    // A side whose typing disagrees with its join's recorded flags routes to `deopt`
    // (no args) instead — see `record_block_flags`.
    let t_ok = record_block_flags(&mut bool_param[t], flags.clone());
    let f_ok = record_block_flags(&mut bool_param[j + 1], flags);
    let tgt = if t_ok { leader_block[t]? } else { deopt }; // falsy → else
    let fall = if f_ok { leader_block[j + 1]? } else { deopt }; // truthy → fall-through
    let args: Vec<BlockArg> = stack
        .iter()
        .enumerate()
        .map(|(i, &op)| BlockArg::Value(as_block_arg(b, op, i, frame)))
        .collect();
    let targs: Vec<BlockArg> = if t_ok { args.clone() } else { Vec::new() };
    let fargs: Vec<BlockArg> = if f_ok { args } else { Vec::new() };
    match cond {
        // A comparison result (`i8`) or a boolean that crossed a block boundary
        // (`Op::Bool`, already `i64`): branch directly — nonzero (true) → truthy →
        // fall-through, zero → else.
        Op::Int(v) if b.func.dfg.value_type(v) != types::I64 => {
            b.ins().brif(v, fall, &fargs, tgt, &targs);
        }
        Op::Bool(v) => {
            b.ins().brif(v, fall, &fargs, tgt, &targs);
        }
        // A boxed condition in a slot/handle — e.g. `(and a b)` boxes its result to a
        // temp slot (`box_scalar` tags it `Bool`), then reads it back. Load the tag
        // (and payload) and branch on Brood truthiness: only `nil` and `false` are
        // falsy, everything else truthy.
        Op::Slot(_) | Op::Handle(..) => {
            let (tagv, payload) = match cond {
                Op::Slot(k) => {
                    let roots_base = b.use_var(frame.rb_var);
                    let i = b.ins().iadd_imm(frame.base, k as i64);
                    let o = b.ins().imul_imm(i, STRIDE);
                    let addr = b.ins().iadd(roots_base, o);
                    let t8 = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
                    let tagv = b.ins().uextend(types::I64, t8);
                    let pl = b.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        addr,
                        PAYLOAD_OFFSET as i32,
                    );
                    (tagv, pl)
                }
                Op::Handle(w0, w1, _) => (b.ins().band_imm(w0, 0xff), w1),
                _ => unreachable!(),
            };
            // falsy = (tag == Nil) || (tag == Bool && payload == 0). Nil's
            // discriminant is 0.
            let is_nil = b.ins().icmp_imm(IntCC::Equal, tagv, 0);
            let is_bool = b.ins().icmp_imm(IntCC::Equal, tagv, TAG_BOOL as i64);
            // A `Value::Bool`'s payload word is only meaningful in its low byte (the
            // `bool`): Rust leaves the upper 7 bytes of the union slot uninitialised, so
            // comparing the full `i64` to 0 spuriously reads `false` (byte 0, garbage
            // above) as *truthy*. Mask to the bool byte — matching the VM's
            // `Value::Bool(b)` read. (This is the bug that corrupted `nest format` once
            // `not`/bool-const arms tiered: `(if x false true)` read its `false` arg as
            // truthy.)
            let pl_byte = b.ins().band_imm(payload, 0xff);
            let pl_false = b.ins().icmp_imm(IntCC::Equal, pl_byte, 0);
            let false_bool = b.ins().band(is_bool, pl_false);
            let falsy = b.ins().bor(is_nil, false_bool);
            b.ins().brif(falsy, tgt, &targs, fall, &fargs);
        }
        // A raw `Op::Int(i64)` here is AMBIGUOUS: it is either a genuine unboxed int
        // (always truthy in Brood) OR a boolean/comparison result that crossed a block
        // boundary and lost its `bool_param` typing at a type-mixed merge (e.g. `(and
        // one (<= …))`, where `and`'s short-circuit can yield the non-bool `one` on one
        // edge — downgrading the slot's tracked bool-ness, so the comparison's 0/1 on
        // the other edge is rebuilt as a raw i64). With no tag we can't tell a falsy
        // bool-0 from a truthy int-0, so branching as "always truthy" silently mis-takes
        // the truthy edge (the bug that made `nest format` non-idempotent — a >width form
        // collapsed because its width-check `<=` 0 read as truthy). Deopt to the VM,
        // which has the real tagged value and branches correctly.
        Op::Int(_) => {
            let __dr = b.ins().iconst(types::I32, 3);
            b.ins().jump(deopt, &[BlockArg::Value(__dr)]);
        }
        // `Op::Float`/`Op::HoistedVec`: unambiguously truthy (a float / a vector is
        // never a boolean), so branch to the truthy edge directly.
        _ => {
            b.ins().jump(fall, &fargs);
        }
    }
    Some(())
}
