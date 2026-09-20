//! Primitive-op arm bodies for `jit_lower_arm_inner`'s emit loop — `Prim1`,
//! `MakeVector`, `Prim3` (table-put), and the fused `Prim2`/`Prim2SlotSlot`/
//! `Prim2SlotInt` arithmetic/list/table/vector primitives. Extracted from the emit
//! loop as part of the `jit_lower_arm_inner` decomposition; jit-only. None of these
//! arms is a terminator, so each returns `Some(())` (fall through) or `None` to bail
//! the whole arm to the VM.
#![cfg(feature = "jit")]
use super::emit::{
    as_f64_guarded, as_f64_pair, as_int, call_handle, cmp_dispatch, emit_arith, emit_float_arith,
    eq_dispatch, inline_vec_ref, load_slot_int, op_is_float, op_maybe_float, read_words,
    slot_untyped, table_prim, vector_ref, Frame, Funcs,
};
use super::Op;
use super::OrBail;
use crate::core::value::jit_layout::{
    PAYLOAD_OFFSET, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_KEYWORD, TAG_PAIR,
};
use crate::eval::compile::ir::{PrimOp, PrimOp1};
use cranelift_codegen::ir::{
    condcodes::{FloatCC, IntCC},
    types, AtomicRmwOp, BlockArg, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Value,
};
use cranelift_frontend::{FunctionBuilder, Variable};
use std::collections::HashMap;

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// `map` reorders the two operands into the primitive's `(x, y)` argument order —
/// e.g. `>` is `%lt` with `map = [1, 0]` (operands swapped). `m == 0` picks the first
/// source, else the second. (`emit_node` only ever produces `[0,1]` or `[1,0]`.)
fn pick(s0: Value, s1: Value, m: u8) -> Value {
    if m == 0 {
        s0
    } else {
        s1
    }
}

/// `Inst::Prim1` — a unary primitive: `first`/`rest` (Pair read, inline or FFI),
/// `nil?`/`pair?`/`empty?` (tag checks), `sqrt` (IEEE fsqrt), `type-of` (total). A
/// non-conforming operand shape deopts; `type-of` never deopts.
pub(super) fn emit_prim1(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    op: &PrimOp1,
    pair_bases: Option<(Variable, Variable)>,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let deopt = frame.deopt;
    let ptr_ty = funcs.ptr_ty;
    let operand = stack.pop().or_bail("operand-stack-underflow")?;
    match op {
        PrimOp1::First | PrimOp1::Rest => {
            // Tag-check it's a Pair; anything else takes the `brood_rt_first`/`_rest`
            // FALLBACK, which answers nil / vector / range / bytes / set and the type
            // error itself and deopts only for a record or a seq-view (they need the
            // evaluator). This used to deopt for every non-pair — an arm whose `first`
            // saw a vector on each activation (`second`, `(first (rest x))`, on `http`)
            // deopted per activation and was latched off the native tier (ADR-353's
            // class). The result is an arbitrary Value, so it's a Handle; the two paths
            // meet at `merge`.
            let [w0, w1, w2] = read_words(b, operand, frame);
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_pair = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_PAIR as i64);
            let cont = b.create_block();
            let slow = b.create_block();
            let merge = b.create_block();
            for _ in 0..3 {
                b.append_block_param(merge, types::I64);
            }
            b.ins().brif(is_pair, cont, &[], slow, &[]);
            b.switch_to_block(slow);
            {
                let fref = match op {
                    PrimOp1::First => funcs.first,
                    PrimOp1::Rest => funcs.rest,
                    _ => unreachable!(),
                };
                let out_addr = b.ins().stack_addr(funcs.ptr_ty, funcs.out_slot, 0);
                let c = b.ins().call(fref, &[funcs.heap, out_addr, w0, w1, w2]);
                let status = b.inst_results(c)[0];
                // `rest` of a non-pair ALLOCATES (a fresh list / range), which can grow the
                // pair slab and move its base: re-fetch the hoisted bases here so the
                // arm's later inline reads go through the new pointers.
                if let (PrimOp1::Rest, Some((nursery_var, old_var))) = (op, pair_bases) {
                    let cn = b.ins().call(funcs.pnbase, &[funcs.heap]);
                    let nb = b.inst_results(cn)[0];
                    b.def_var(nursery_var, nb);
                    let co = b.ins().call(funcs.pobase, &[funcs.heap]);
                    let ob = b.inst_results(co)[0];
                    b.def_var(old_var, ob);
                }
                let ok = b.create_block();
                let not_ok = b.create_block();
                b.ins().brif(status, not_ok, &[], ok, &[]);
                b.switch_to_block(not_ok);
                let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
                let __dr = b.ins().iconst(types::I32, 4);
                b.ins()
                    .brif(is_err, funcs.error, &[], deopt, &[BlockArg::Value(__dr)]);
                b.switch_to_block(ok);
                let o0 = b
                    .ins()
                    .stack_load(types::I64, types::I64, funcs.out_slot, 0);
                let o1 = b.ins().stack_load(
                    types::I64,
                    types::I64,
                    funcs.out_slot,
                    PAYLOAD_OFFSET as i32,
                );
                let o2 = b.ins().stack_load(
                    types::I64,
                    types::I64,
                    funcs.out_slot,
                    PAYLOAD_OFFSET as i32 + 8,
                );
                b.ins().jump(merge, &[o0.into(), o1.into(), o2.into()]);
            }
            b.switch_to_block(cont);
            let h = if let Some((nursery_var, old_var)) = pair_bases {
                let nursery_base = b.use_var(nursery_var);
                let old_base = b.use_var(old_var);
                // Inline LOCAL pair read. PairId layout (w1):
                //   bits 0..31  = index into the slab
                //   bits 32..60 = gen epoch (ignored here)
                //   bit  61     = age  (0=nursery, 1=old)
                //   bits 62..63 = region (0=LOCAL, 1=PRELUDE, 2=RUNTIME)
                // Non-LOCAL (PRELUDE/RUNTIME) reads take the `car`/`cdr` callback rather
                // than deopting. Deopting here was a **70x cliff on `def`'d data**: the
                // deopt fires per element, the arm's consecutive-deopt counter bails it,
                // and the whole loop reverts to the interpreter — measured 2026-07-28 at
                // 77 ns/element walking a `def`'d list against 1 ns for the identical loop
                // over a LOCAL one, i.e. exactly the `BROOD_NO_JIT=1` time. A global
                // holding a data structure is ordinary Brood (`sort` walks a `def`'d list;
                // `matmul` derefs a `def`'d matrix ~16 M times), so this is not the rare
                // path the old comment assumed.
                //
                // The shared regions cannot use the inline `base + idx*48` form at all:
                // their slabs are `boxcar::Vec`, chunked rather than contiguous, so there
                // is no single base pointer to hoist. One call per read is the cheap
                // option, and it keeps the arm native instead of surrendering the loop.
                let high2 = b.ins().ushr_imm_s(w1, 62);
                let is_local = b.ins().icmp_imm_s(IntCC::Equal, high2, 0i64);
                let local_cont = b.create_block();
                let shared_cont = b.create_block();
                let join = b.create_block();
                for _ in 0..3 {
                    b.append_block_param(join, types::I64);
                }
                b.ins().brif(is_local, local_cont, &[], shared_cont, &[]);

                // PRELUDE/RUNTIME: one callback, result joined with the inline path.
                b.switch_to_block(shared_cont);
                b.seal_block(shared_cont);
                let sref = match op {
                    PrimOp1::First => funcs.car,
                    PrimOp1::Rest => funcs.cdr,
                    _ => unreachable!(),
                };
                let shared = call_handle(b, sref, &[w0, w1, w2], funcs);
                let (s0, s1, s2) = match shared {
                    Op::Handle(a, c, d) => (a, c, d),
                    _ => unreachable!("call_handle yields a Handle"),
                };
                b.ins().jump(join, &[s0.into(), s1.into(), s2.into()]);

                b.switch_to_block(local_cont);
                b.seal_block(local_cont);
                // Age bit 61: 0=nursery, 1=old. After the LOCAL check, bits 62-63 are
                // 0, so ushr by 61 gives exactly 0 or 1.
                let age_shifted = b.ins().ushr_imm_s(w1, 61);
                let is_old = b.ins().icmp_imm_s(IntCC::NotEqual, age_shifted, 0i64);
                let base = b.ins().select(is_old, old_base, nursery_base);
                // Index: lower 32 bits. stride = 48 (two 24-byte Values).
                let idx = b.ins().band_imm_s(w1, 0xFFFF_FFFFi64);
                let byte_off = b.ins().imul_imm_s(idx, 48i64);
                let pair_ptr = b.ins().iadd(base, byte_off);
                // Car at offset 0, cdr at offset 24 (one Value = 24 bytes).
                let field_off: i64 = if matches!(op, PrimOp1::Rest) { 24 } else { 0 };
                let field_ptr = if field_off == 0 {
                    pair_ptr
                } else {
                    b.ins().iadd_imm_s(pair_ptr, field_off)
                };
                let rw0 = b
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), field_ptr, 0);
                let rw1 = b.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    field_ptr,
                    PAYLOAD_OFFSET as i32,
                );
                let rw2 = b.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    field_ptr,
                    PAYLOAD_OFFSET as i32 + 8,
                );
                b.ins().jump(join, &[rw0.into(), rw1.into(), rw2.into()]);

                b.switch_to_block(join);
                b.seal_block(join);
                let jp = b.block_params(join);
                Op::Handle(jp[0], jp[1], jp[2])
            } else {
                let fref = match op {
                    PrimOp1::First => funcs.car,
                    PrimOp1::Rest => funcs.cdr,
                    _ => unreachable!(),
                };
                call_handle(b, fref, &[w0, w1, w2], funcs)
            };
            let (p0, p1, p2) = match h {
                Op::Handle(a, c, d) => (a, c, d),
                _ => unreachable!("the pair path yields a Handle"),
            };
            b.ins().jump(merge, &[p0.into(), p1.into(), p2.into()]);
            b.switch_to_block(merge);
            let mp = b.block_params(merge);
            stack.push(Op::Handle(mp[0], mp[1], mp[2]));
        }
        PrimOp1::IsNil => {
            // Tag-only nil check: compare the tag byte to 0 (Tag::Nil). Result is an
            // i8 comparison value (truthy in JumpIfFalse).
            let [w0, _, _] = read_words(b, operand, frame);
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_nil = b.ins().icmp_imm_s(IntCC::Equal, tagb, 0);
            stack.push(Op::Int(is_nil));
        }
        PrimOp1::IsPair => {
            // Tag-only pair check: compare the tag byte to TAG_PAIR. Ranges and
            // SeqViews also carry TAG_PAIR — matching nil?/pair? semantics from
            // builtins.rs.
            let [w0, _, _] = read_words(b, operand, frame);
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_pair = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_PAIR as i64);
            stack.push(Op::Int(is_pair));
        }
        PrimOp1::IsEmpty => {
            // nil → true, pair → false, inline; anything else takes the
            // `brood_rt_is_empty` FALLBACK, which sizes a vector / string / set / bytes /
            // range / rope / table itself and deopts only for a map (a record's `Seqable`
            // view decides) or a seq-view (it realises). This used to deopt for every
            // non-nil/non-pair, and a `(cond (empty? coll) … (first coll) … (rest coll))`
            // loop — `any?`, `every?`, `seq/index-where` — handed a vector deopted AT ENTRY
            // on every activation and ran its whole scan on the VM (`json`'s
            // `needs-escape?` is `(any? (string/->codepoints s) …)`: 7 910 entry deopts per
            // run, never latched because a `SelfCall` arm is not deopt-watched). The two
            // paths meet at `merge` with the boolean as an `i8`.
            let [w0, w1, w2] = read_words(b, operand, frame);
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_nil = b.ins().icmp_imm_s(IntCC::Equal, tagb, 0);
            let is_pair = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_PAIR as i64);
            let is_nil_or_pair = b.ins().bor(is_nil, is_pair);
            let cont = b.create_block();
            let slow = b.create_block();
            let merge = b.create_block();
            b.append_block_param(merge, types::I8);
            b.ins().brif(is_nil_or_pair, cont, &[], slow, &[]);
            b.switch_to_block(slow);
            {
                let out_addr = b.ins().stack_addr(funcs.ptr_ty, funcs.out_slot, 0);
                let c = b
                    .ins()
                    .call(funcs.is_empty, &[funcs.heap, out_addr, w0, w1, w2]);
                let status = b.inst_results(c)[0];
                let ok = b.create_block();
                let not_ok = b.create_block();
                b.ins().brif(status, not_ok, &[], ok, &[]);
                b.switch_to_block(not_ok);
                let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
                let __dr = b.ins().iconst(types::I32, 5);
                b.ins()
                    .brif(is_err, funcs.error, &[], deopt, &[BlockArg::Value(__dr)]);
                b.switch_to_block(ok);
                // `*out` is a `Value::Bool`: only the payload's low byte is the `bool`
                // (the upper bytes are uninitialised padding — see `emit_jump_if_false`).
                let pl = b.ins().stack_load(
                    types::I64,
                    types::I64,
                    funcs.out_slot,
                    PAYLOAD_OFFSET as i32,
                );
                let pl_byte = b.ins().band_imm_s(pl, 0xff);
                let is_true = b.ins().icmp_imm_s(IntCC::NotEqual, pl_byte, 0);
                b.ins().jump(merge, &[is_true.into()]);
            }
            b.switch_to_block(cont);
            // Past the tag test: is_nil is 1 for nil, 0 for pair — exactly the boolean
            // result we want.
            b.ins().jump(merge, &[is_nil.into()]);
            b.switch_to_block(merge);
            let r = b.block_params(merge)[0];
            stack.push(Op::Int(r));
        }
        PrimOp1::Sqrt => {
            // Prelude `sqrt`, x > 0 only: one IEEE `fsqrt` (correctly rounded —
            // identical to the wrapper's `f64::sqrt`). Zero, negatives (the wrapper's
            // error), NaN, and non-float shapes deopt so the VM dispatches the real
            // wrapper.
            match operand {
                Op::Float(v) => {
                    let zero = b.ins().f64const(0.0);
                    let pos = b.ins().fcmp(FloatCC::GreaterThan, v, zero);
                    let cont = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 6);
                    b.ins()
                        .brif(pos, cont, &[], deopt, &[BlockArg::Value(__dr)]);
                    b.switch_to_block(cont);
                    stack.push(Op::Float(b.ins().sqrt(v)));
                }
                Op::Int(v) if b.func.dfg.value_type(v) == types::I64 => {
                    let pos = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, v, 0);
                    let cont = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 7);
                    b.ins()
                        .brif(pos, cont, &[], deopt, &[BlockArg::Value(__dr)]);
                    b.switch_to_block(cont);
                    let f = b.ins().fcvt_from_sint(types::F64, v);
                    stack.push(Op::Float(b.ins().sqrt(f)));
                }
                _ => {
                    // Type-erased (slot / call result): runtime tag dispatch — Float >
                    // 0 → fsqrt; Int > 0 → convert + fsqrt; anything else → deopt.
                    let [w0, w1, _] = read_words(b, operand, frame);
                    let tagb = b.ins().band_imm_s(w0, 0xff);
                    let done = b.create_block();
                    b.append_block_param(done, types::F64);
                    let is_f = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_FLOAT as i64);
                    let fblk = b.create_block();
                    let not_f = b.create_block();
                    b.ins().brif(is_f, fblk, &[], not_f, &[]);
                    b.switch_to_block(fblk);
                    let fv = b.ins().bitcast(types::F64, MemFlagsData::new(), w1);
                    let zero = b.ins().f64const(0.0);
                    let posf = b.ins().fcmp(FloatCC::GreaterThan, fv, zero);
                    let fok = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 8);
                    b.ins()
                        .brif(posf, fok, &[], deopt, &[BlockArg::Value(__dr)]);
                    b.switch_to_block(fok);
                    let fr = b.ins().sqrt(fv);
                    b.ins().jump(done, &[BlockArg::Value(fr)]);
                    b.switch_to_block(not_f);
                    let is_i = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_INT as i64);
                    let iblk = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 9);
                    b.ins()
                        .brif(is_i, iblk, &[], deopt, &[BlockArg::Value(__dr)]);
                    b.switch_to_block(iblk);
                    let posi = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, w1, 0);
                    let iok = b.create_block();
                    let __dr = b.ins().iconst(types::I32, 10);
                    b.ins()
                        .brif(posi, iok, &[], deopt, &[BlockArg::Value(__dr)]);
                    b.switch_to_block(iok);
                    let fi = b.ins().fcvt_from_sint(types::F64, w1);
                    let ir = b.ins().sqrt(fi);
                    b.ins().jump(done, &[BlockArg::Value(ir)]);
                    b.switch_to_block(done);
                    stack.push(Op::Float(b.block_params(done)[0]));
                }
            }
        }
        PrimOp1::TypeIs(kw) => {
            let kw = *kw;
            // `(vector? x)` and kin: `(%eq (type-of x) :kw)` as one compare. Total over
            // every operand — no deopt. An unboxed operand's tag is known at compile time,
            // so the answer is a constant; a boxed one goes through the same
            // `type_of_kw_table` load `TypeOf` uses (so the collapsing rules hold) and
            // compares the keyword id. Result is an i8 compare (truthy in JumpIfFalse).
            let known = match operand {
                Op::Int(v) if b.func.dfg.value_type(v) == types::I64 => {
                    Some(crate::core::value::Tag::Int)
                }
                Op::Float(_) => Some(crate::core::value::Tag::Float),
                Op::Bool(_) => Some(crate::core::value::Tag::Bool),
                _ => None,
            };
            match known {
                Some(t) => {
                    let r = b.ins().iconst(types::I8, i64::from(t.keyword() == kw));
                    stack.push(Op::Int(r));
                }
                None => {
                    let [w0, _, _] = read_words(b, operand, frame);
                    let tagb = b.ins().band_imm_s(w0, 0xff);
                    let table = crate::core::value::jit_layout::type_of_kw_table();
                    let base = b.ins().iconst(ptr_ty, table.as_ptr() as i64);
                    let off = b.ins().imul_imm_s(tagb, 4);
                    let addr = b.ins().iadd(base, off);
                    let sym = b.ins().load(types::I32, MemFlagsData::new(), addr, 0);
                    let is = b.ins().icmp_imm_s(IntCC::Equal, sym, kw as i64);
                    stack.push(Op::Int(is));
                }
            }
        }
        PrimOp1::VectorLen => {
            // `(%vector-length v)`: one callback that reads the slab, returning the
            // length unboxed — or -1 for a non-vector, which deopts so the VM raises the
            // native's exact type error. Not a safepoint (no allocation), so live
            // handles stay valid across it.
            let [w0, w1, w2] = read_words(b, operand, frame);
            let c = b.ins().call(funcs.vlen, &[funcs.heap, w0, w1, w2]);
            let len = b.inst_results(c)[0];
            let ok = b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, len, 0);
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 40);
            b.ins().brif(ok, cont, &[], deopt, &[BlockArg::Value(__dr)]);
            b.switch_to_block(cont);
            stack.push(Op::Int(len));
        }
        PrimOp1::TypeOf => {
            // Total over every operand — no deopt. An unboxed operand's tag is known at
            // compile time (constant keyword); a boxed one loads its keyword id from the
            // 256-entry discriminant-byte table (`type_of_kw_table`, 'static — the
            // address is stable for the process lifetime) and boxes TAG_KEYWORD + the id.
            let kw_const = |b: &mut FunctionBuilder, t: crate::core::value::Tag| {
                let w0 = b.ins().iconst(types::I64, TAG_KEYWORD as i64);
                let w1 = b.ins().iconst(types::I64, t.keyword() as i64);
                let w2 = b.ins().iconst(types::I64, 0);
                Op::Handle(w0, w1, w2)
            };
            match operand {
                Op::Int(v) if b.func.dfg.value_type(v) == types::I64 => {
                    let op = kw_const(b, crate::core::value::Tag::Int);
                    stack.push(op);
                }
                Op::Float(_) => {
                    let op = kw_const(b, crate::core::value::Tag::Float);
                    stack.push(op);
                }
                Op::Bool(_) => {
                    let op = kw_const(b, crate::core::value::Tag::Bool);
                    stack.push(op);
                }
                _ => {
                    // Type-erased (slot / call result / i8 compare): tag byte → table
                    // load → boxed keyword.
                    let [w0, _, _] = read_words(b, operand, frame);
                    let tagb = b.ins().band_imm_s(w0, 0xff);
                    let table = crate::core::value::jit_layout::type_of_kw_table();
                    let base = b.ins().iconst(ptr_ty, table.as_ptr() as i64);
                    let off = b.ins().imul_imm_s(tagb, 4);
                    let addr = b.ins().iadd(base, off);
                    let sym = b.ins().load(types::I32, MemFlagsData::new(), addr, 0);
                    let w1 = b.ins().uextend(types::I64, sym);
                    let w0k = b.ins().iconst(types::I64, TAG_KEYWORD as i64);
                    let w2 = b.ins().iconst(types::I64, 0);
                    stack.push(Op::Handle(w0k, w1, w2));
                }
            }
        }
    }
    Some(())
}

/// `Inst::MakeVector(n)` — build an `n`-element vector literal. `n == 2` bump-allocates
/// in place via `vec2_room` (no temp `Vec`); otherwise stages the `n` operands into a per-site
/// stack slot and calls `make_vector_n`.
pub(super) fn emit_make_vector(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    n: usize,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let ptr_ty = funcs.ptr_ty;
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    if n == 2 {
        // Arity-2 fast path: bump-allocate the slot, then write both elements **into it**.
        //
        // The elements used to travel as six `i64` arguments to `brood_rt_make_vector2`.
        // SysV passes six in registers and `heap`/`out` take two of them, so four words
        // spilled to this arm's outgoing-args area and the callee loaded them straight
        // back — two stack loads that were **66% of that function**, itself 7.9% of
        // `bintree`. Same shape as the return value in §2h, same fix: hand back the
        // destination. `brood_rt_vec2_room` takes two register arguments, returns the
        // slot's element storage, and the stores below land there.
        //
        // The slot is reachable from the handle the moment it is allocated, so **nothing
        // between the call and these stores may allocate or collect** — they are pure
        // stores. `alloc_vector2_room` leaves the elements `Nil` rather than uninitialised
        // precisely because this window exists: a missed store degrades to a wrong value,
        // never to a word the GC would trace as a handle.
        let (b_op, a_op) = (
            stack.pop().or_bail("operand-stack-underflow")?,
            stack.pop().or_bail("operand-stack-underflow")?,
        );
        let aw = read_words(b, a_op, frame);
        let bw = read_words(b, b_op, frame);
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let c = b.ins().call(funcs.vec2room, &[heap, out_addr]);
        let items = b.inst_results(c)[0];
        for (i, w) in [aw, bw].iter().enumerate() {
            let off = (i * std::mem::size_of::<crate::core::value::Value>()) as i32;
            b.ins().store(MemFlagsData::trusted(), w[0], items, off);
            b.ins().store(
                MemFlagsData::trusted(),
                w[1],
                items,
                off + PAYLOAD_OFFSET as i32,
            );
            b.ins().store(
                MemFlagsData::trusted(),
                w[2],
                items,
                off + PAYLOAD_OFFSET as i32 + 8,
            );
        }
        let w0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        stack.push(Op::Handle(w0, w1, w2));
    } else {
        // Variadic `[e0 … e{n-1}]` (nbody's `[vx vy vz]` / 7-body rebuild). Pop the `n`
        // operands (pushed in source order: e0 deepest, e{n-1} on top), box each to a
        // `Value` word-triple, and store it into a per-site Cranelift stack slot (`n ×
        // STRIDE` bytes) the JIT owns. Then call `brood_rt_make_vector_n(heap, out,
        // stage, n)`, which `alloc_vector`s (never collects) — so the staged bytes stay
        // live across the call. Read the fresh handle back out of `out_slot`.
        let mut ops = Vec::with_capacity(n);
        for _ in 0..n {
            ops.push(stack.pop().or_bail("operand-stack-underflow")?);
        }
        ops.reverse(); // ops[i] = element i, in source order
        let stage = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            STRIDE as u32 * n as u32,
            3,
        ));
        for (i, op) in ops.into_iter().enumerate() {
            let w = read_words(b, op, frame);
            let off = i as i32 * STRIDE as i32;
            b.ins().stack_store(types::I64, w[0], stage, off);
            b.ins()
                .stack_store(types::I64, w[1], stage, off + PAYLOAD_OFFSET as i32);
            b.ins()
                .stack_store(types::I64, w[2], stage, off + PAYLOAD_OFFSET as i32 + 8);
        }
        let stage_addr = b.ins().stack_addr(ptr_ty, stage, 0);
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let n_val = b.ins().iconst(types::I64, n as i64);
        b.ins()
            .call(funcs.makevecn, &[heap, out_addr, stage_addr, n_val]);
        let w0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        stack.push(Op::Handle(w0, w1, w2));
    }
    Some(())
}

/// `Inst::MakeMap(npairs)` — a `{k v …}` literal. The variadic `MakeVector` shape verbatim:
/// pop the `2·npairs` operands (pushed in source order, key before value, last value on
/// top), stage each as a `Value` word-triple in a per-site stack slot, and call
/// `brood_rt_make_map_n(heap, out, stage, npairs)`, whose `map_from_pairs` allocates (a
/// GC-quiet in-place CHAMP build) and never collects, so the staged bytes stay live across
/// the call. Read the fresh handle back out of `out_slot`.
pub(super) fn emit_make_map(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    npairs: usize,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let ptr_ty = funcs.ptr_ty;
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    let n = 2 * npairs;
    let mut ops = Vec::with_capacity(n);
    for _ in 0..n {
        ops.push(stack.pop().or_bail("operand-stack-underflow")?);
    }
    ops.reverse(); // ops[2i] = key i, ops[2i+1] = value i, in source order
    let stage = b.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        STRIDE as u32 * n.max(1) as u32,
        3,
    ));
    for (i, op) in ops.into_iter().enumerate() {
        let w = read_words(b, op, frame);
        let off = i as i32 * STRIDE as i32;
        b.ins().stack_store(types::I64, w[0], stage, off);
        b.ins()
            .stack_store(types::I64, w[1], stage, off + PAYLOAD_OFFSET as i32);
        b.ins()
            .stack_store(types::I64, w[2], stage, off + PAYLOAD_OFFSET as i32 + 8);
    }
    let stage_addr = b.ins().stack_addr(ptr_ty, stage, 0);
    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
    let n_val = b.ins().iconst(types::I64, npairs as i64);
    b.ins()
        .call(funcs.makemapn, &[heap, out_addr, stage_addr, n_val]);
    let w0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
    stack.push(Op::Handle(w0, w1, w2));
    Some(())
}

/// A 3-operand primitive lowered as ONE runtime callback of `table_put`'s shape —
/// `(heap, out, recv 3w, a 3w, b 3w) -> status` — for the map ops (ADR-368). Operands are
/// on the stack in source order (`b` on top). Status 0: the answer rides back in `out`; 1:
/// deopt (the receiver, or the case, is one the VM's real wrapper owns); 2: an error is
/// parked in `jit_pending_error` → the arm's error block. The callbacks may allocate
/// (`assoc` path-copies) but never collect, so live register handles stay valid.
pub(super) fn emit_prim3_callback(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    frame: Frame,
    funcs: Funcs,
    fref: cranelift_codegen::ir::FuncRef,
    deopt_code: i64,
) -> Option<()> {
    let bv = stack.pop().or_bail("operand-stack-underflow")?;
    let av = stack.pop().or_bail("operand-stack-underflow")?;
    let recv = stack.pop().or_bail("operand-stack-underflow")?;
    let r = read_words(b, recv, frame);
    let a = read_words(b, av, frame);
    let v = read_words(b, bv, frame);
    let out_addr = b.ins().stack_addr(funcs.ptr_ty, funcs.out_slot, 0);
    let c = b.ins().call(
        fref,
        &[
            funcs.heap, out_addr, r[0], r[1], r[2], a[0], a[1], a[2], v[0], v[1], v[2],
        ],
    );
    let status = b.inst_results(c)[0];
    let cont = b.create_block();
    let slow = b.create_block();
    b.ins().brif(status, slow, &[], cont, &[]);
    b.switch_to_block(slow);
    let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
    let __dr = b.ins().iconst(types::I32, deopt_code);
    b.ins().brif(
        is_err,
        funcs.error,
        &[],
        frame.deopt,
        &[BlockArg::Value(__dr)],
    );
    b.switch_to_block(cont);
    let w0 = b
        .ins()
        .stack_load(types::I64, types::I64, funcs.out_slot, 0);
    let w1 = b.ins().stack_load(
        types::I64,
        types::I64,
        funcs.out_slot,
        PAYLOAD_OFFSET as i32,
    );
    let w2 = b.ins().stack_load(
        types::I64,
        types::I64,
        funcs.out_slot,
        PAYLOAD_OFFSET as i32 + 8,
    );
    stack.push(Op::Handle(w0, w1, w2));
    Some(())
}

/// `Inst::Prim3 { op: TablePut, .. }` — `(table-put t k v)`. A hoisted dense table does
/// one atomic xchg on the key's slot (every guard failure routes to the FFI, never a
/// deopt); otherwise the FFI callback runs. Status 0 → the table handle rides back, 1 →
/// deopt (non-Table operand), 2 → parked error → the error block.
pub(super) fn emit_prim3_table_put(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let deopt = frame.deopt;
    let ptr_ty = funcs.ptr_ty;
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    let error = funcs.error;
    // `(table-put t k v)`: operands pushed in source order — value on top.
    let val = stack.pop().or_bail("operand-stack-underflow")?;
    let key = stack.pop().or_bail("operand-stack-underflow")?;
    let tbl = stack.pop().or_bail("operand-stack-underflow")?;
    if let Op::HoistedTable {
        slots,
        flag,
        w0,
        w1,
        w2,
    } = tbl
    {
        // Inline dense put (the sieve lever): ONE atomic xchg on the key's slot. Every
        // guard failure — null base (hashed table), non-int / out-of-range key,
        // unencodable value, MOVED sentinel, dense flag dropped — routes to the FFI
        // block, which runs the exact full semantics (never a deopt, so an odd shape
        // can't thrash the arm). The result is the table handle either way — the hoisted
        // words.
        let kw = read_words(b, key, frame);
        let vw = read_words(b, val, frame);
        let ffi = b.create_block();
        let merge = b.create_block();
        let g_key = b.create_block();
        let g_bounds = b.create_block();
        let g_enc = b.create_block();
        let enc_done = b.create_block();
        b.append_block_param(enc_done, types::I64);
        let nul = b.ins().icmp_imm_s(IntCC::Equal, slots, 0);
        b.ins().brif(nul, ffi, &[], g_key, &[]);
        b.switch_to_block(g_key);
        let ktag = b.ins().band_imm_s(kw[0], 0xff);
        let k_int = b.ins().icmp_imm_s(IntCC::Equal, ktag, TAG_INT as i64);
        b.ins().brif(k_int, g_bounds, &[], ffi, &[]);
        b.switch_to_block(g_bounds);
        let oob = b.ins().icmp_imm_s(
            IntCC::UnsignedGreaterThanOrEqual,
            kw[1],
            crate::core::table::DENSE_KEY_MAX,
        );
        b.ins().brif(oob, ffi, &[], g_enc, &[]);
        // Encode the value into a tagged slot word (mirrors `table::slot_enc`): Int
        // (61-bit) / Bool / Nil; else FFI.
        b.switch_to_block(g_enc);
        let vtag = b.ins().band_imm_s(vw[0], 0xff);
        let enc_int_range = b.create_block();
        let t_bool = b.create_block();
        let v_int = b.ins().icmp_imm_s(IntCC::Equal, vtag, TAG_INT as i64);
        b.ins().brif(v_int, enc_int_range, &[], t_bool, &[]);
        b.switch_to_block(enc_int_range);
        let sh = b.ins().ishl_imm_s(vw[1], 3);
        let back = b.ins().sshr_imm_s(sh, 3);
        let fits = b.ins().icmp(IntCC::Equal, back, vw[1]);
        let enc_int_ok = b.create_block();
        b.ins().brif(fits, enc_int_ok, &[], ffi, &[]);
        b.switch_to_block(enc_int_ok);
        let wi = b.ins().bor_imm_s(sh, crate::core::table::INT_TAG as i64);
        b.ins().jump(enc_done, &[BlockArg::Value(wi)]);
        b.switch_to_block(t_bool);
        let v_bool = b.ins().icmp_imm_s(IntCC::Equal, vtag, TAG_BOOL as i64);
        let t_nil = b.create_block();
        let enc_bool = b.create_block();
        b.ins().brif(v_bool, enc_bool, &[], t_nil, &[]);
        b.switch_to_block(enc_bool);
        // Bool payload byte may carry padding above bit 0 — mask, then 3 - bit → TRUE
        // (1→2) / FALSE (0→3).
        let bit = b.ins().band_imm_s(vw[1], 1);
        let three = b.ins().iconst(types::I64, 3);
        let wb = b.ins().isub(three, bit);
        b.ins().jump(enc_done, &[BlockArg::Value(wb)]);
        b.switch_to_block(t_nil);
        // `Value::Nil`'s discriminant is 0 (declaration order).
        let v_nil = b.ins().icmp_imm_s(IntCC::Equal, vtag, 0);
        let enc_nil = b.create_block();
        b.ins().brif(v_nil, enc_nil, &[], ffi, &[]);
        b.switch_to_block(enc_nil);
        let wn = b
            .ins()
            .iconst(types::I64, crate::core::table::SLOT_NIL as i64);
        b.ins().jump(enc_done, &[BlockArg::Value(wn)]);
        b.switch_to_block(enc_done);
        let word = b.block_params(enc_done)[0];
        // The key's chunk pointer (`DenseSlots` is a directory of lazily-mapped chunks):
        // null means no write ever reached this key's range — the FFI maps it. A plain
        // load: the pointer is installed once, and the slot access depends on it.
        let cidx = b
            .ins()
            .ushr_imm_s(kw[1], crate::core::table::CHUNK_SHIFT as i64);
        let coff = b.ins().imul_imm_s(cidx, 8);
        let caddr = b.ins().iadd(slots, coff);
        let chunk = b.ins().load(types::I64, MemFlagsData::trusted(), caddr, 0);
        let cnull = b.ins().icmp_imm_s(IntCC::Equal, chunk, 0);
        let g_slot = b.create_block();
        b.ins().brif(cnull, ffi, &[], g_slot, &[]);
        b.switch_to_block(g_slot);
        let within = b
            .ins()
            .band_imm_s(kw[1], (crate::core::table::CHUNK_SLOTS - 1) as i64);
        let off = b.ins().imul_imm_s(within, 8);
        let addr = b.ins().iadd(chunk, off);
        let old = b.ins().atomic_rmw(
            types::I64,
            MemFlagsData::trusted(),
            AtomicRmwOp::Xchg,
            addr,
            word,
        );
        let moved = b
            .ins()
            .icmp_imm_s(IntCC::Equal, old, crate::core::table::SLOT_MOVED as i64);
        let g_flag = b.create_block();
        b.ins().brif(moved, ffi, &[], g_flag, &[]);
        // Post-op dense-flag re-check (the migration protocol on `table::Store`): still
        // dense → done; flipped → re-apply via the FFI (an idempotent overwrite on the
        // hashed map).
        b.switch_to_block(g_flag);
        let f = b
            .ins()
            .atomic_load(types::I8, MemFlagsData::trusted(), flag);
        b.ins().brif(f, merge, &[], ffi, &[]);
        b.switch_to_block(ffi);
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let c = b.ins().call(
            funcs.tput,
            &[
                heap, out_addr, w0, w1, w2, kw[0], kw[1], kw[2], vw[0], vw[1], vw[2],
            ],
        );
        let status = b.inst_results(c)[0];
        let slow = b.create_block();
        b.ins().brif(status, slow, &[], merge, &[]);
        b.switch_to_block(slow);
        let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
        let __dr = b.ins().iconst(types::I32, 11);
        b.ins()
            .brif(is_err, error, &[], deopt, &[BlockArg::Value(__dr)]);
        b.switch_to_block(merge);
        stack.push(Op::Handle(w0, w1, w2));
    } else {
        let t = read_words(b, tbl, frame);
        let k = read_words(b, key, frame);
        let v = read_words(b, val, frame);
        let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let c = b.ins().call(
            funcs.tput,
            &[
                heap, out_addr, t[0], t[1], t[2], k[0], k[1], k[2], v[0], v[1], v[2],
            ],
        );
        let status = b.inst_results(c)[0];
        let cont = b.create_block();
        let slow = b.create_block();
        b.ins().brif(status, slow, &[], cont, &[]);
        b.switch_to_block(slow);
        let is_err = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
        let __dr = b.ins().iconst(types::I32, 12);
        b.ins()
            .brif(is_err, error, &[], deopt, &[BlockArg::Value(__dr)]);
        b.switch_to_block(cont);
        let w0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        stack.push(Op::Handle(w0, w1, w2));
    }
    Some(())
}

/// `Inst::Prim2` — a binary primitive over two operand-stack values. Dispatches by op:
/// `cons`/`table-has?`/`table-get`/`vector-ref` (with hoisted-table / hoisted-vec inline
/// fast paths), runtime-dispatched `=`, float arith/compare, and integer arith/compare.
/// `map` reorders operands into `(x, y)`. Returns `None` to bail (unlowerable op).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_prim2(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    op: &PrimOp,
    map: [u8; 2],
    has_float_slot: bool,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let deopt = frame.deopt;
    // Operands were pushed in source order: `aa` (deeper) is source 0, `bb` (top) is
    // source 1.
    let (bb_op, aa_op) = (
        stack.pop().or_bail("operand-stack-underflow")?,
        stack.pop().or_bail("operand-stack-underflow")?,
    );
    if matches!(op, PrimOp::Cons) {
        // `cons` takes any operands and allocates: car = source 0, cdr = source 1
        // (cons's `map` is `[0,1]`). Read each as words, alloc.
        let car = read_words(b, aa_op, frame);
        let cdr = read_words(b, bb_op, frame);
        let h = call_handle(
            b,
            funcs.cons,
            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
            funcs,
        );
        stack.push(h);
    } else if matches!(op, PrimOp::TableHas | PrimOp::TableGet) {
        // `(table-has? t k)` / 2-arg `(table-get t k)`. `map[0]` picks which SOURCE is
        // the table (a swapped wrapper reorders), exactly like the VM's `[sa, sb][map[0]]`.
        let (tbl_op, key_op) = if map[0] == 0 {
            (aa_op, bb_op)
        } else {
            (bb_op, aa_op)
        };
        if let (
            PrimOp::TableHas,
            Op::HoistedTable {
                slots,
                flag,
                w0,
                w1,
                w2,
            },
        ) = (*op, tbl_op)
        {
            // Inline dense has? (the sieve lever): one atomic load of the key's slot.
            // Guard failures route to the FFI (exact semantics, no deopt); an in-range
            // EMPTY/set slot answers inline, and an out-of-range int key is simply absent.
            let kw = read_words(b, key_op, frame);
            let ffi = b.create_block();
            let merge = b.create_block();
            b.append_block_param(merge, types::I8);
            let g_key = b.create_block();
            let g_bounds = b.create_block();
            let g_load = b.create_block();
            let g_flag = b.create_block();
            let absent = b.create_block();
            let nul = b.ins().icmp_imm_s(IntCC::Equal, slots, 0);
            b.ins().brif(nul, ffi, &[], g_key, &[]);
            b.switch_to_block(g_key);
            let ktag = b.ins().band_imm_s(kw[0], 0xff);
            let k_int = b.ins().icmp_imm_s(IntCC::Equal, ktag, TAG_INT as i64);
            b.ins().brif(k_int, g_bounds, &[], ffi, &[]);
            b.switch_to_block(g_bounds);
            let oob = b.ins().icmp_imm_s(
                IntCC::UnsignedGreaterThanOrEqual,
                kw[1],
                crate::core::table::DENSE_KEY_MAX,
            );
            b.ins().brif(oob, absent, &[], g_load, &[]);
            b.switch_to_block(absent);
            let no = b.ins().iconst(types::I8, 0);
            b.ins().jump(merge, &[BlockArg::Value(no)]);
            b.switch_to_block(g_load);
            // The key's chunk pointer — see the put lowering above; an unmapped chunk
            // answers through the FFI (absent, under the same flag protocol).
            let cidx = b
                .ins()
                .ushr_imm_s(kw[1], crate::core::table::CHUNK_SHIFT as i64);
            let coff = b.ins().imul_imm_s(cidx, 8);
            let caddr = b.ins().iadd(slots, coff);
            let chunk = b.ins().load(types::I64, MemFlagsData::trusted(), caddr, 0);
            let cnull = b.ins().icmp_imm_s(IntCC::Equal, chunk, 0);
            let g_slot = b.create_block();
            b.ins().brif(cnull, ffi, &[], g_slot, &[]);
            b.switch_to_block(g_slot);
            let within = b
                .ins()
                .band_imm_s(kw[1], (crate::core::table::CHUNK_SLOTS - 1) as i64);
            let off = b.ins().imul_imm_s(within, 8);
            let addr = b.ins().iadd(chunk, off);
            let sv = b
                .ins()
                .atomic_load(types::I64, MemFlagsData::trusted(), addr);
            let moved = b
                .ins()
                .icmp_imm_s(IntCC::Equal, sv, crate::core::table::SLOT_MOVED as i64);
            b.ins().brif(moved, ffi, &[], g_flag, &[]);
            b.switch_to_block(g_flag);
            let f = b
                .ins()
                .atomic_load(types::I8, MemFlagsData::trusted(), flag);
            let done = b.create_block();
            b.ins().brif(f, done, &[], ffi, &[]);
            b.switch_to_block(done);
            let present =
                b.ins()
                    .icmp_imm_s(IntCC::NotEqual, sv, crate::core::table::SLOT_EMPTY as i64);
            b.ins().jump(merge, &[BlockArg::Value(present)]);
            // FFI fallback: the exact `table-has?`; its `Value::Bool` result reduces to
            // the same i8.
            b.switch_to_block(ffi);
            let h = table_prim(b, funcs.thas, [w0, w1, w2], kw, frame, funcs);
            let hb = match h {
                Op::Handle(_, hw1, _) => {
                    let bit = b.ins().band_imm_s(hw1, 1);
                    b.ins().icmp_imm_s(IntCC::NotEqual, bit, 0)
                }
                _ => unreachable!("table_prim returns a Handle"),
            };
            b.ins().jump(merge, &[BlockArg::Value(hb)]);
            b.switch_to_block(merge);
            let out = b.block_params(merge)[0];
            stack.push(Op::Int(out));
        } else {
            let tbl = read_words(b, tbl_op, frame);
            let key = read_words(b, key_op, frame);
            let fref = if matches!(op, PrimOp::TableHas) {
                funcs.thas
            } else {
                funcs.tget
            };
            let h = table_prim(b, fref, tbl, key, frame, funcs);
            stack.push(h);
        }
    } else if matches!(op, PrimOp::MapGet) {
        // `(get m k)` on a map: `map` is always `[0,1]` here (`resolve_prim` returns only
        // that for `get`), so source 0 is the map and source 1 the key. Status 1 — a
        // non-map receiver, an absent key, or a stored `nil` — deopts, and the VM re-runs
        // the real `get`, which owns those branches and `%lookup-miss`.
        let m = read_words(b, aa_op, frame);
        let k = read_words(b, bb_op, frame);
        let h = table_prim(b, funcs.mget, m, k, frame, funcs);
        stack.push(h);
    } else if matches!(op, PrimOp::VectorRef) {
        // `(vector-ref v i)` / inlined `(nth v i)`: map is `[0,1]`, so source 0 (`aa`)
        // is the vector, source 1 (`bb`) the index.
        if let Op::HoistedVec { ptr, len, .. } = aa_op {
            // Hoisted invariant global vector: inline `ptr + idx*STRIDE` (no slab-lookup
            // call). Index tag-checks to int (deopt else); out-of-range deopts so the VM
            // gives `nth`'s exact result.
            let idx = as_int(b, bb_op, frame);
            let oob = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, len);
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 13);
            b.ins()
                .brif(oob, deopt, &[BlockArg::Value(__dr)], cont, &[]);
            b.switch_to_block(cont);
            let off = b.ins().imul_imm_s(idx, STRIDE);
            let elem = b.ins().iadd(ptr, off);
            let w0 = b.ins().load(types::I64, MemFlagsData::trusted(), elem, 0);
            let w1 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                elem,
                PAYLOAD_OFFSET as i32,
            );
            let w2 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                elem,
                PAYLOAD_OFFSET as i32 + 8,
            );
            stack.push(Op::Handle(w0, w1, w2));
        } else {
            let vec = read_words(b, aa_op, frame);
            let idx = read_words(b, bb_op, frame);
            let h = vector_ref(b, vec, idx, frame, funcs);
            stack.push(h);
        }
    } else if matches!(op, PrimOp::Eq)
        && !op_is_float(aa_op, frame)
        && !op_is_float(bb_op, frame)
        && (matches!(aa_op, Op::Handle(..) | Op::Slot(_))
            || matches!(bb_op, Op::Handle(..) | Op::Slot(_)))
    {
        // `=` with a type-erased operand: runtime-dispatched equality (int×int payload
        // compare / interned-immediate identity / deopt).
        let wa = read_words(b, aa_op, frame);
        let wb = read_words(b, bb_op, frame);
        stack.push(Op::Int(eq_dispatch(b, wa, wb, frame, funcs)));
    } else if matches!(op, PrimOp::Lt | PrimOp::Le)
        && !op_is_float(aa_op, frame)
        && !op_is_float(bb_op, frame)
        && (matches!(aa_op, Op::Handle(..)) || matches!(bb_op, Op::Handle(..)))
        && op_maybe_float(aa_op, frame)
        && op_maybe_float(bb_op, frame)
    {
        // `<` / `<=` with a type-erased operand and nothing proven float: dispatched by
        // tag at runtime (int×int as ints, else as floats) — a bool either way, so no
        // guess is needed and none is made. See `cmp_dispatch`.
        let wa = read_words(b, aa_op, frame);
        let wb = read_words(b, bb_op, frame);
        stack.push(Op::Int(cmp_dispatch(b, *op, wa, wb, map, frame)?));
    } else if op_is_float(aa_op, frame)
        || op_is_float(bb_op, frame)
        || (has_float_slot
            && matches!(op, PrimOp::Add | PrimOp::Sub | PrimOp::Mul | PrimOp::Div)
            && (matches!(aa_op, Op::Handle(..)) || matches!(bb_op, Op::Handle(..)))
            // … unless the other operand can never be a float (an int literal: `(+ (count
            // xs) 4)`), which the float path would deopt on unconditionally
            && op_maybe_float(aa_op, frame)
            && op_maybe_float(bb_op, frame))
    {
        // Float arith/compare (an operand is a float, or — in a float-context arm — a
        // type-erased `Op::Handle` optimistically treated as float, e.g. `(- (nth bi 0)
        // (nth bj 0))`). `as_f64_guarded` tag-checks each `Handle` is `Float` and deopts
        // otherwise, so a wrong guess is safe (a deopt, not a miscompile); a right guess
        // yields `Op::Float`, which `store_op` marks float so the rest of the chain stays
        // unboxed. `pick` selects f64 values the same as i64.
        //
        // An `Int` operand may only be PROMOTED to f64 once the OTHER operand is proven
        // float, which is the VM's own rule for when an op is float arithmetic at all —
        // hence one paired read rather than two independent ones. See KI-114.
        let (aa, bb) = as_f64_pair(b, aa_op, bb_op, frame);
        let x = pick(aa, bb, map[0]);
        let y = pick(aa, bb, map[1]);
        stack.push(emit_float_arith(b, *op, x, y, deopt)?);
    } else {
        // Arithmetic/comparison: materialise to int, apply `map`.
        let aa = as_int(b, aa_op, frame);
        let bb = as_int(b, bb_op, frame);
        let x = pick(aa, bb, map[0]);
        let y = pick(aa, bb, map[1]);
        stack.push(Op::Int(emit_arith(b, *op, x, y, deopt)?));
    }
    Some(())
}

/// `Inst::Prim2SlotSlot` — a fused binary primitive reading two frame slots directly.
/// Same op dispatch as `Prim2`, plus the LICM `hoisted` invariant-vector inline for
/// `(nth slot slot)`. Returns `None` to bail.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_prim2_slot_slot(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    op: &PrimOp,
    map: [u8; 2],
    slot_a: usize,
    slot_b: usize,
    hoisted: &HashMap<usize, (Value, Value)>,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let deopt = frame.deopt;
    if matches!(op, PrimOp::Cons) {
        // `(cons slot_a slot_b)`: car = slot_a, cdr = slot_b (map `[0,1]`).
        let car = read_words(b, Op::Slot(slot_a), frame);
        let cdr = read_words(b, Op::Slot(slot_b), frame);
        let h = call_handle(
            b,
            funcs.cons,
            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
            funcs,
        );
        stack.push(h);
    } else if matches!(op, PrimOp::TableHas | PrimOp::TableGet) {
        // `(table-has?/table-get slot_a slot_b)`. `map[0]` picks which slot is the table
        // (mirrors the VM's `[sa, sb][map[0]]`).
        let s0 = read_words(b, Op::Slot(slot_a), frame);
        let s1 = read_words(b, Op::Slot(slot_b), frame);
        let (tbl, key) = if map[0] == 0 { (s0, s1) } else { (s1, s0) };
        let fref = if matches!(op, PrimOp::TableHas) {
            funcs.thas
        } else {
            funcs.tget
        };
        let h = table_prim(b, fref, tbl, key, frame, funcs);
        stack.push(h);
    } else if matches!(op, PrimOp::MapGet) {
        // `(get slot_a slot_b)` on a map — the slot/slot twin of the operand form above.
        let m = read_words(b, Op::Slot(slot_a), frame);
        let k = read_words(b, Op::Slot(slot_b), frame);
        let h = table_prim(b, funcs.mget, m, k, frame, funcs);
        stack.push(h);
    } else if matches!(op, PrimOp::VectorRef) {
        // `(nth slot_a slot_b)`: source 0 = vector slot, source 1 = index slot (map `[0,1]`).
        if let Some(&(ptr, vlen)) = hoisted.get(&slot_a) {
            // Hoisted invariant base: inline `ptr + idx*STRIDE` element read (no
            // per-element call / slab lookup). The index slot tag-checks to int (deopt
            // otherwise); an out-of-range index deopts so the VM produces `nth`'s exact
            // out-of-range result.
            let idx = load_slot_int(b, slot_b as i64, frame);
            let oob = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, idx, vlen);
            let cont = b.create_block();
            let __dr = b.ins().iconst(types::I32, 14);
            b.ins()
                .brif(oob, deopt, &[BlockArg::Value(__dr)], cont, &[]);
            b.switch_to_block(cont);
            let off = b.ins().imul_imm_s(idx, STRIDE);
            let elem = b.ins().iadd(ptr, off);
            let w0 = b.ins().load(types::I64, MemFlagsData::trusted(), elem, 0);
            let w1 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                elem,
                PAYLOAD_OFFSET as i32,
            );
            let w2 = b.ins().load(
                types::I64,
                MemFlagsData::trusted(),
                elem,
                PAYLOAD_OFFSET as i32 + 8,
            );
            stack.push(Op::Handle(w0, w1, w2));
        } else {
            // Read each operand as a full `Value`, then slab-read.
            let vec = read_words(b, Op::Slot(slot_a), frame);
            let idx = read_words(b, Op::Slot(slot_b), frame);
            let h = vector_ref(b, vec, idx, frame, funcs);
            stack.push(h);
        }
    } else if matches!(op, PrimOp::Eq)
        && !op_is_float(Op::Slot(slot_a), frame)
        && !op_is_float(Op::Slot(slot_b), frame)
    {
        // `(= slot slot)` — runtime-dispatched equality (see eq_dispatch): int×int costs
        // the same two tag-checks as the old int-only path, and keyword/symbol operands
        // now compare inline instead of deopting the whole arm.
        let wa = read_words(b, Op::Slot(slot_a), frame);
        let wb = read_words(b, Op::Slot(slot_b), frame);
        stack.push(Op::Int(eq_dispatch(b, wa, wb, frame, funcs)));
    } else if matches!(op, PrimOp::Lt | PrimOp::Le)
        && slot_untyped(slot_a, frame)
        && slot_untyped(slot_b, frame)
    {
        // `(< a b)` on two slots nothing typed — let-bound `nth`/`count` results: by tag at
        // runtime, ints as ints and anything else as floats (`cmp_dispatch`), a bool either
        // way. Neither the integer guess nor the float one: each deopted on every call for
        // the other kind.
        let wa = read_words(b, Op::Slot(slot_a), frame);
        let wb = read_words(b, Op::Slot(slot_b), frame);
        stack.push(Op::Int(cmp_dispatch(b, *op, wa, wb, map, frame)?));
    } else if op_is_float(Op::Slot(slot_a), frame)
        || op_is_float(Op::Slot(slot_b), frame)
        || (frame.float_context
            && matches!(op, PrimOp::Add | PrimOp::Sub | PrimOp::Mul | PrimOp::Div)
            && slot_untyped(slot_a, frame)
            && slot_untyped(slot_b, frame))
    {
        // Float arith/compare on two slots (e.g. `(+ xx yy)`, `(* x y)`). Both operands
        // are slots, so the float context rests entirely on `slot_float`'s guess — or, in a
        // float-context arm, on two slots nothing typed at all (`[ax0 ay0 ax1 ay1]`
        // destructured from a vector of floats, then `(<= ax0 bx1)`): read them
        // as a pair, so one being a float is what licenses promoting the other and two ints
        // deopt to the integer operation they are (KI-114).
        let (sa, sb) = as_f64_pair(b, Op::Slot(slot_a), Op::Slot(slot_b), frame);
        let x = pick(sa, sb, map[0]);
        let y = pick(sa, sb, map[1]);
        stack.push(emit_float_arith(b, *op, x, y, deopt)?);
    } else {
        // Source 0 = slot_a, source 1 = slot_b (the VM's `[sa, sb]` order).
        let sa = load_slot_int(b, slot_a as i64, frame);
        let sb = load_slot_int(b, slot_b as i64, frame);
        let x = pick(sa, sb, map[0]);
        let y = pick(sa, sb, map[1]);
        stack.push(Op::Int(emit_arith(b, *op, x, y, deopt)?));
    }
    Some(())
}

/// `Inst::Prim2SlotInt` — a fused binary primitive over a frame slot and a literal int.
/// Handles `(nth slot k)` (inline vector read), table ops with a const key, `cons`, and
/// float / integer arithmetic (the literal promoted to f64 for a float slot). Returns
/// `None` to bail.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_prim2_slot_int(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    op: &PrimOp,
    map: [u8; 2],
    slot_a: usize,
    int_b: i64,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let deopt = frame.deopt;
    if matches!(op, PrimOp::VectorRef) {
        // `(nth v 0)` / `(nth v 1)` — constant index fused into the slot. slot_a is
        // always the vector (source 0 after map normalisation). Inline the read for a
        // LOCAL small vector (deopting otherwise), the analog of the pair car/cdr inline
        // — this is `bintree`'s `(nth node 0/1)` hot path.
        let vec = read_words(b, Op::Slot(slot_a), frame);
        let h = inline_vec_ref(b, vec, int_b, frame, funcs);
        stack.push(h);
    } else if matches!(op, PrimOp::TableHas | PrimOp::TableGet) {
        // `(table-has?/table-get slot <int-const>)` — a constant int fused into the
        // instruction. `map[0]` says which side is the table: 0 → the slot (`(table-has?
        // t 5)`), 1 → the const (a swapped `(table-has? 5 x)` fusion — nonsense at
        // runtime; the callback returns status 1 and the VM raises the exact type error).
        let slot_w = read_words(b, Op::Slot(slot_a), frame);
        let kt = b.ins().iconst(types::I64, TAG_INT as i64);
        let kv = b.ins().iconst(types::I64, int_b);
        let kz = b.ins().iconst(types::I64, 0);
        let int_w = [kt, kv, kz];
        let (tbl, key) = if map[0] == 0 {
            (slot_w, int_w)
        } else {
            (int_w, slot_w)
        };
        let fref = if matches!(op, PrimOp::TableHas) {
            funcs.thas
        } else {
            funcs.tget
        };
        let h = table_prim(b, fref, tbl, key, frame, funcs);
        stack.push(h);
    } else
    // `(cons slot int_literal)` or `(cons int_literal slot)` (after map inversion for
    // the swapped form). After fusion, slot_a is always source 0; map[0]=0 → slot is
    // car, int is cdr; map[0]=1 → int is car, slot is cdr (original was `(cons Const
    // Local)`). Both map to brood_rt_cons.
    if matches!(op, PrimOp::Cons) {
        let slot_words = read_words(b, Op::Slot(slot_a), frame);
        let int_tag = b.ins().iconst(types::I64, TAG_INT as i64);
        let int_val = b.ins().iconst(types::I64, int_b);
        let z = b.ins().iconst(types::I64, 0);
        let int_words = [int_tag, int_val, z];
        let (car, cdr) = if map[0] == 0 {
            (slot_words, int_words)
        } else {
            (int_words, slot_words)
        };
        let h = call_handle(
            b,
            funcs.cons,
            &[car[0], car[1], car[2], cdr[0], cdr[1], cdr[2]],
            funcs,
        );
        stack.push(h);
    } else if op_is_float(Op::Slot(slot_a), frame) {
        // `(op floatslot int-literal)` — Brood coerces the int to f64 (`(+ 1.5 1)` =
        // 2.5). Promote the literal and do float arith.
        //
        // Only the SLOT decides that, and only at runtime: the literal is an int, so if
        // the slot holds an int too this is an integer operation and the VM answers an
        // int. `slot_float` is a guess, so pass `promote = false` and let an int slot
        // deopt. This is KI-114 — unary `-` lowers here as `(%sub 0 x)`, and a float
        // profile made `(- 33)` publish `-33.0`.
        let sa = as_f64_guarded(b, Op::Slot(slot_a), frame, false);
        let sb = b.ins().f64const(int_b as f64);
        let x = pick(sa, sb, map[0]);
        let y = pick(sa, sb, map[1]);
        stack.push(emit_float_arith(b, *op, x, y, deopt)?);
    } else {
        // Source 0 = slot_a, source 1 = the literal `int_b` (the fusion of `(Const,
        // Local)` already inverted `map` so the slot is source 0).
        let sa = load_slot_int(b, slot_a as i64, frame);
        let sb = b.ins().iconst(types::I64, int_b);
        let x = pick(sa, sb, map[0]);
        let y = pick(sa, sb, map[1]);
        stack.push(Op::Int(emit_arith(b, *op, x, y, deopt)?));
    }
    Some(())
}
