//! The value-producing arm bodies of the emit loop that are not primitives or calls:
//! `Const`, `Global`/`GlobalIc`, `SetLocal`, and the deopt-resume journal written after an
//! effect. jit-only.
#![cfg(feature = "jit")]
use super::emit::{self, Frame, handle_from_out_slot};
use super::entry::Hoisted;
use super::*;
use cranelift_codegen::ir::{FuncRef, InstBuilder, MemFlagsData, StackSlot, Type, types};
use cranelift_frontend::FunctionBuilder;

type SsaValue = cranelift_codegen::ir::Value;

/// `Inst::Const`: an int or float literal becomes an unboxed SSA value; `nil` a constant
/// 3-word handle (a consumer that wants an int tag-checks and deopts); a bool an `Op::Bool`.
/// A GC-movable heap constant (string, bignum, pair, closure, …) is loaded at the point of
/// use through `brood_rt_const_load(cv, out)`, which reads its CURRENT bits —
/// `runtime_collect` rewrites them in place (`ConstVal::rewrite`). The `ConstVal` lives in a
/// chunk the installed code keeps alive, so its address is stable for the code's lifetime.
pub(super) fn emit_const(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    constant: &ConstVal,
    pointer_type: Type,
    out_slot: StackSlot,
    const_load: FuncRef,
) {
    match constant.load().unpack() {
        ValueRef::Int(n) => stack.push(Op::Int(b.ins().iconst(types::I64, n))),
        ValueRef::Float(f) => stack.push(Op::Float(b.ins().f64const(f))),
        ValueRef::Nil => {
            let zero = b.ins().iconst(types::I64, 0);
            stack.push(Op::Handle(zero, zero, zero));
        }
        ValueRef::Bool(value) => {
            let bits = b.ins().iconst(types::I64, if value { 1 } else { 0 });
            stack.push(Op::Bool(bits));
        }
        // A keyword or a symbol is an interned id, never moved: its words are known now, as
        // `type-of`'s answer is built (`prim.rs`). Through `brood_rt_const_load` every
        // `(get m :key)` in native code paid a call for its key — 3% of a 3D game's tick,
        // most of it the keys of pose maps.
        ValueRef::Keyword(id) | ValueRef::Sym(id) => {
            let tag = if matches!(constant.load().unpack(), ValueRef::Keyword(_)) {
                crate::core::value::jit_layout::TAG_KEYWORD
            } else {
                crate::core::value::jit_layout::TAG_SYM
            };
            let w0 = b.ins().iconst(types::I64, tag as i64);
            let w1 = b.ins().iconst(types::I64, id as i64);
            let w2 = b.ins().iconst(types::I64, 0);
            stack.push(Op::Handle(w0, w1, w2));
        }
        _ => {
            let constant_pointer = b
                .ins()
                .iconst(pointer_type, constant as *const ConstVal as i64);
            let out_address = b.ins().stack_addr(pointer_type, out_slot, 0);
            b.ins().call(const_load, &[constant_pointer, out_address]);
            stack.push(handle_from_out_slot(b, out_slot));
        }
    }
}

/// Unbox a free-global read that was observed holding a `Value::Float` at tier time
/// ([`CompiledArm::float_globals`]) into an `Op::Float`, so arithmetic over it takes the
/// float path instead of the integer default — otherwise an arm whose floats come from a
/// `def`'d constant (nbody's `dt`) deopts on every activation until it latches `BAILED`.
///
/// Soundness is [`emit::as_f64`]'s tag guard, not the tier-time observation: a global that
/// is no longer a float fails the guard and deopts to the VM. A stale guess costs a deopt;
/// it can never miscompile.
fn unbox_float_global(
    b: &mut FunctionBuilder,
    symbol: Symbol,
    op: Op,
    frame: Frame,
    float_globals: &[Symbol],
) -> Op {
    if float_globals.contains(&symbol) {
        Op::Float(emit::as_f64(b, op, frame))
    } else {
        op
    }
}

/// The context a free-global read needs.
pub(super) struct GlobalReadContext<'a> {
    pub(super) hoisted: &'a Hoisted,
    pub(super) float_globals: &'a [Symbol],
    pub(super) global: FuncRef,
    pub(super) global_ic: FuncRef,
    pub(super) heap: SsaValue,
    pub(super) context_pointer: SsaValue,
    pub(super) out_slot: StackSlot,
    pub(super) pointer_type: Type,
    pub(super) error: cranelift_codegen::ir::Block,
}

/// `Inst::Global` / `Inst::GlobalIc`: a free-global read (a call's callee, or a value).
///
/// A global hoisted at entry pushes its entry-resolved words (the back-edge epoch guard
/// deopts on a rebind, so this stays bit-identical to the VM's late binding). Otherwise a
/// `GlobalIc` resolves through the per-`site` inline cache (`brood_rt_global_ic`) and a bare
/// `Global` through `brood_rt_global`; an unbound symbol parks an error and exits via
/// `error` (outcome 3). The resolved value is an arbitrary `Value`, so it is a `Handle`.
pub(super) fn emit_global_read(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    symbol: Symbol,
    site: Option<u32>,
    reads: &GlobalReadContext,
    frame: Frame,
) {
    if let Some(&(slots, flag, w0, w1, w2)) = reads.hoisted.tables.get(&symbol) {
        // A hoisted dense table: words + slot region for the inline table ops.
        stack.push(Op::HoistedTable {
            slots,
            flag,
            w0,
            w1,
            w2,
        });
    } else if let Some(&(w0, w1, w2)) = reads.hoisted.scalars.get(&symbol) {
        stack.push(unbox_float_global(
            b,
            symbol,
            Op::Handle(w0, w1, w2),
            frame,
            reads.float_globals,
        ));
    } else if let Some(&(ptr, len, w0, w1, w2)) = reads.hoisted.globals.get(&symbol) {
        stack.push(Op::HoistedVec {
            ptr,
            len,
            w0,
            w1,
            w2,
        });
    } else {
        let symbol_value = b.ins().iconst(types::I32, symbol as i64);
        let out_address = b.ins().stack_addr(reads.pointer_type, reads.out_slot, 0);
        let c = if let Some(site) = site {
            let site_value = b.ins().iconst(types::I32, site as i64);
            b.ins().call(
                reads.global_ic,
                &[
                    reads.heap,
                    out_address,
                    symbol_value,
                    site_value,
                    reads.context_pointer,
                ],
            )
        } else {
            b.ins().call(
                reads.global,
                &[reads.heap, out_address, symbol_value, reads.context_pointer],
            )
        };
        let status = b.inst_results(c)[0];
        let cont = b.create_block();
        emit::branch_on_global_status(b, status, reads.error, frame.deopt, cont);
        b.switch_to_block(cont);
        let handle = handle_from_out_slot(b, reads.out_slot);
        stack.push(unbox_float_global(
            b,
            symbol,
            handle,
            frame,
            reads.float_globals,
        ));
    }
}

/// `Inst::SetLocal`: a `let`/`letrec` binder → frame slot `slot`. A `Slot` operand
/// (possibly a handle) is copied verbatim; an `Int` is boxed as `Int`, a comparison `i8`
/// as `Bool` (`store_op`). Let-slots are scratch, dominated by this store, so a deopt's VM
/// re-run recomputes the binding before any read sees a stale slot.
///
/// BUT `Inst::Local` pushes a *lazy* `Op::Slot(i)` that re-reads the slot at its consumer,
/// and the bytecode reuses one slot index across sibling `let` scopes (sound for the VM,
/// whose operand stack holds *values*). So a still-pending `Op::Slot(slot)` from an earlier
/// binding would read THIS binding after the store. Materialise every such pending
/// reference to the slot's current (pre-store) value first, preserving its exact type.
/// (`(- (let (a A) a) (let (b B) b))` reused slot 1 and computed 0, not A-B.)
pub(super) fn emit_set_local(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    slot: usize,
    frame: Frame,
) -> Option<()> {
    for op in stack.iter_mut() {
        if !matches!(*op, Op::Slot(k) if k == slot) {
            continue;
        }
        let words = emit::read_words(b, Op::Slot(slot), frame);
        *op = if frame
            .slot_float
            .borrow()
            .get(slot)
            .copied()
            .unwrap_or(false)
        {
            Op::Float(b.ins().bitcast(types::F64, MemFlagsData::new(), words[1]))
        } else if frame.slot_bool.borrow().get(slot).copied().unwrap_or(false) {
            Op::Bool(words[1])
        } else {
            Op::Handle(words[0], words[1], words[2])
        };
    }
    let op = stack.pop().or_bail("operand-stack-underflow")?;
    emit::store_op(b, slot as i64, op, frame);
    Some(())
}

/// Deopt-resume checkpoint (see `CompiledArm::ckpt_slot`): an effect at `ip` (a non-tail
/// call, a `table-put`) just completed — journal the abstract operand stack (only GC-safe
/// shapes here: unboxed scalars, frame slots, and the fresh result) into the reserved slots
/// above `checkpoint_slot`, plus the packed `(resume_ip << 16) | depth`, so a LATER deopt in
/// this activation resumes right here instead of re-running (and re-effecting) from ip 0.
pub(super) fn emit_checkpoint_journal(
    b: &mut FunctionBuilder,
    stack: &[Op],
    ip: usize,
    checkpoint_slot: u32,
    frame: Frame,
) {
    let journal_base = checkpoint_slot as i64 + 1;
    for (k, &op) in stack.iter().enumerate() {
        emit::store_op(b, journal_base + k as i64, op, frame);
    }
    let packed = (((ip as i64) + 1) << 16) | stack.len() as i64;
    let packed_value = b.ins().iconst(types::I64, packed);
    emit::store_int(b, checkpoint_slot as i64, packed_value, frame);
}
