//! Control-flow arm bodies for `jit_lower_arm_inner`'s emit loop — the `Jump` and
//! `JumpIfFalse` terminators, and the **deferred edge** machinery that types a join's
//! block parameters only once EVERY predecessor is known (`PendingEdge`,
//! `resolve_edges`). Extracted from the emit loop as part of the `jit_lower_arm_inner`
//! decomposition; jit-only. Each arm still `break`s the caller's inner loop (it's a
//! block terminator) — these fns emit the terminator and return `Some(())`, or `None`
//! to bail the whole arm to the VM.
//!
//! # Why edges are deferred (KI-132, second half)
//!
//! A join block carries its operand stack as one `i64` block parameter per entry, and a
//! single word cannot say whether it holds a raw int, a 0/1 bool, or a placeholder for a
//! value living in a frame slot — that is what [`ParamRepr`] records per entry. Edges
//! used to be emitted in ip order and the FIRST edge to reach a join fixed its typing; a
//! later edge that disagreed was compiled as an unconditional deopt ("type-mixed join").
//! That is sound, but it turns any `(or p 7)` / `(if c x 7)` — a boxed slot on one edge,
//! a scalar on the other — into an arm that deopts on every activation taking the second
//! edge, and sixteen of those latch the arm off the native tier for good: the
//! highlighter's `hl-advance` (`(or p (and params? (= n 2)))`, `p` false on most frames)
//! ran interpreted for every keystroke in bedit.
//!
//! Now a branch targets a fresh **edge block** per successor and records the edge's
//! operand stack (with the reprs it would cross as, snapshotted at emission time — the
//! slot flags are a single-pass approximation and must be read when the edge's stores
//! were emitted, not later). Edges are forward-only and emitted in ip order, so by the
//! time the target leader is translated every predecessor is pending; `resolve_edges`
//! then unifies the typings — entries every edge agrees on keep their repr; a
//! disagreement widens, to `Slot(spill)` when the arm reserved a block-argument spill
//! slot at that index (each edge materialises its value there as a full tagged `Value`)
//! and otherwise to `Words` (the three tagged words ride in extra block params and the
//! leader rebuilds an `Op::Handle`) — fills each edge block with its coercions + the
//! jump, and fixes `bool_param` for the target. A widened entry is read back through
//! the ordinary tag-checked paths, so a scalar consumer of a non-scalar value deopts on
//! THAT use (where the VM raises), never on the join. ADR-353.
#![cfg(feature = "jit")]
use super::emit::{
    as_block_arg_for, param_repr, read_words, store_op, store_result, Frame, ParamRepr,
};
use super::Op;
use super::OrBail;
use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL};
use cranelift_codegen::ir::{condcodes::IntCC, types, Block, BlockArg, InstBuilder, MemFlagsData};
use cranelift_frontend::FunctionBuilder;

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// One not-yet-typed control edge into a leader: the edge block the branch already
/// targets (empty until `resolve_edges` fills it), the operand stack crossing it, and
/// the repr each entry would cross as, decided when the branch was emitted.
pub(super) struct PendingEdge {
    pub from: Block,
    pub stack: Vec<Op>,
    pub reprs: Vec<ParamRepr>,
}

/// Register a forward edge into leader `t` from the block being emitted at ip `j`:
/// creates the edge block, snapshots the stack + its reprs, and returns the block for
/// the branch to target. `None` (bail) on a backward edge — the target was translated
/// before this edge existed, so it could never be resolved; only `SelfCall` loops back,
/// and it targets the param-less leader 0 by its own path.
pub(super) fn defer_edge(
    b: &mut FunctionBuilder,
    stack: &[Op],
    t: usize,
    j: usize,
    pending: &mut [Vec<PendingEdge>],
    frame: Frame,
) -> Option<Block> {
    if t <= j {
        return None;
    }
    let reprs: Vec<ParamRepr> = stack
        .iter()
        .enumerate()
        .map(|(i, &op)| param_repr(b, op, i, frame))
        .collect();
    let from = b.create_block();
    pending[t].push(PendingEdge {
        from,
        stack: stack.to_vec(),
        reprs,
    });
    Some(from)
}

/// Fill every pending edge into leader `ip` and fix the leader's block-param typing.
/// Returns the unified reprs (what the leader rebuilds its operand stack from), or
/// `None` to bail on a prepass/emit depth disagreement (a lowering bug, named under the
/// bail trace — never a shape).
///
/// `done`: `ip == len`, the Done leader — each edge stores its single result through
/// `out_ptr` and jumps to `target`; a stack of any other height marks a dead edge (see
/// `emit_jump`) and routes to `deopt`.
pub(super) fn resolve_edges(
    b: &mut FunctionBuilder,
    ip: usize,
    target: Block,
    edges: Vec<PendingEdge>,
    done: bool,
    frame: Frame,
) -> Option<Vec<ParamRepr>> {
    if done {
        for e in edges {
            b.switch_to_block(e.from);
            if e.stack.len() == 1 {
                store_result(b, e.stack[0], frame.out_ptr, frame);
                b.ins().jump(target, &[]);
            } else {
                let __dr = b.ins().iconst(types::I32, 1);
                b.ins().jump(frame.deopt, &[BlockArg::Value(__dr)]);
            }
        }
        return Some(Vec::new());
    }
    let Some(first) = edges.first() else {
        // No predecessor: an unreachable leader (dead code after a tail `SelfCall`). It is
        // still translated; its params default to `Int` as before.
        return Some(Vec::new());
    };
    let depth = first.stack.len();
    // The prepass sized the leader's params from ITS depth model; the emit loop's stack is
    // the truth. A disagreement is a lowering bug (the two models differ on some
    // instruction's stack effect), and used to surface only as a Cranelift verifier
    // rejection at `define_function` — name it, with the ip and both depths.
    let want = b.block_params(target).len();
    if edges.iter().any(|e| e.stack.len() != depth) || depth != want {
        if crate::diagnostics::debug_flags::jit_bail_trace() {
            let got: Vec<usize> = edges.iter().map(|e| e.stack.len()).collect();
            eprintln!("[jit-bail] join-depth-mismatch at ip={ip}: prepass={want} edges={got:?}");
        }
        return None;
    }
    // Unify: agreement keeps the repr; disagreement widens — into the block-argument
    // spill slot at that index when the arm reserved one (the value lands in the frame,
    // GC-visible, exactly as a KI-49 handle crossing does), else as the three tagged
    // words in extra block params (`ParamRepr::Words`: nothing in the frame, and the
    // target holds an `Op::Handle`, which the spill-at-call machinery already covers).
    let mut unified: Vec<ParamRepr> = Vec::with_capacity(depth);
    let mut widened_slots: Vec<usize> = Vec::new();
    let mut words_extra: usize = 0;
    for i in 0..depth {
        let r0 = first.reprs[i];
        // A `Words(WORDS_WANTED)` is a request, not a repr the edges can agree on: it
        // widens even when every predecessor asks for it.
        let wants_words = matches!(r0, ParamRepr::Words(_));
        if !wants_words && edges.iter().all(|e| e.reprs[i] == r0) {
            unified.push(r0);
        } else if i < frame.blockarg_spill_len {
            let s = frame.blockarg_spill_base + i;
            unified.push(ParamRepr::Slot(s));
            widened_slots.push(s);
        } else {
            unified.push(ParamRepr::Words(words_extra));
            words_extra += 1;
        }
    }
    if !widened_slots.is_empty() || words_extra > 0 {
        // Under the bail trace, name the join that widened: the arm now lowers where it
        // used to deopt-thrash, and a widened entry is read back through tag checks, so
        // this is the line to read when a row moves after a lowering change.
        if crate::diagnostics::debug_flags::jit_bail_trace() {
            eprintln!(
                "[jit-widen] ip={ip} slots={widened_slots:?} words={words_extra} reprs={unified:?}"
            );
        }
    }
    // The extra params for `Words` entries: two per entry, appended after the depth
    // params. Legal here because no edge into `target` has been emitted yet.
    for _ in 0..words_extra {
        b.append_block_param(target, types::I64);
        b.append_block_param(target, types::I64);
    }
    for e in edges {
        b.switch_to_block(e.from);
        let mut args: Vec<BlockArg> = Vec::with_capacity(depth + 2 * words_extra);
        let mut extra: Vec<BlockArg> = Vec::with_capacity(2 * words_extra);
        for (i, &op) in e.stack.iter().enumerate() {
            let own = e.reprs[i];
            let want = unified[i];
            let v = match want {
                _ if own == want => as_block_arg_for(b, op, own, frame),
                // Widened into the frame: materialise this edge's value into the spill
                // slot as a tagged `Value` — `store_op` boxes an `i64` as `Int`, an
                // `i8`/`Bool` as `Bool`, copies a `Slot`, stores a `Handle`'s words — and
                // pass a placeholder word.
                ParamRepr::Slot(s) => {
                    store_op(b, s as i64, op, frame);
                    b.ins().iconst(types::I64, 0)
                }
                // Widened into block params: the tagged words, boxed from whatever this
                // edge holds (`read_words` boxes a scalar, loads a slot, passes a handle).
                ParamRepr::Words(_) => {
                    let w = read_words(b, op, frame);
                    extra.push(BlockArg::Value(w[1]));
                    extra.push(BlockArg::Value(w[2]));
                    w[0]
                }
                ParamRepr::Int | ParamRepr::Bool | ParamRepr::Float => {
                    unreachable!("a widened entry is a slot or words")
                }
            };
            args.push(BlockArg::Value(v));
        }
        args.extend(extra);
        b.ins().jump(target, &args);
    }
    // A widened slot holds a different kind of value on each edge, so no single-pass
    // flag describes it: clear them, and every read of the slot tag-checks.
    for &s in &widened_slots {
        if let Some(x) = frame.slot_float.borrow_mut().get_mut(s) {
            *x = false;
        }
        if let Some(x) = frame.slot_bool.borrow_mut().get_mut(s) {
            *x = false;
        }
        if let Some(x) = frame.slot_f64_cache.borrow_mut().get_mut(s) {
            *x = None;
        }
    }
    Some(unified)
}

/// `Inst::Jump(t)` — an unconditional branch. `t == len` targets Done (return the
/// single result via `roots[base]`); otherwise it registers a deferred edge into leader
/// `t` and jumps to its edge block. A dead jump (wrong stack height at Done) routes to
/// `deopt`. Returns `None` to bail the arm (a backward edge).
pub(super) fn emit_jump(
    b: &mut FunctionBuilder,
    stack: &[Op],
    t: usize,
    j: usize,
    len: usize,
    done_block: Block,
    pending: &mut [Vec<PendingEdge>],
    frame: Frame,
) -> Option<()> {
    let deopt = frame.deopt;
    if t == len {
        // Jump straight to Done: return the single result through the caller's `out`
        // pointer (the second of the arm's two Done exits — see `Frame::out_ptr`).
        if stack.len() == 1 {
            store_result(b, stack[0], frame.out_ptr, frame);
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
        let e = defer_edge(b, stack, t, j, pending, frame).or_bail("backward-jump")?;
        b.ins().jump(e, &[]);
    }
    Some(())
}

/// `Inst::JumpIfFalse(t)` — pop the condition and branch: falsy → leader `t`, truthy →
/// the fall-through leader `j + 1`, each through a deferred edge (`defer_edge`), so the
/// two joins are typed once all their predecessors are known. The condition's shape
/// picks the branch form: an unboxed `i8`/`Bool` branches directly; a boxed
/// slot/handle loads the tag+payload and branches on Brood truthiness (only `nil` and
/// `false` falsy); a raw ambiguous `Op::Int` deopts; a float/vector is truthy.
pub(super) fn emit_jump_if_false(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    t: usize,
    j: usize,
    pending: &mut [Vec<PendingEdge>],
    frame: Frame,
) -> Option<()> {
    let deopt = frame.deopt;
    let cond = stack.pop()?;
    // The ambiguous-int case below deopts without taking either edge, and an edge block
    // nobody branches to would stay unfilled — so register edges only for the forms
    // that branch.
    let (want_t, want_f) = match cond {
        Op::Int(v) if b.func.dfg.value_type(v) == types::I64 => (false, false),
        Op::Int(_) | Op::Bool(_) | Op::Slot(_) | Op::Handle(..) => (true, true),
        _ => (false, true),
    };
    let tgt = if want_t {
        defer_edge(b, stack, t, j, pending, frame).or_bail("backward-jump")?
    } else {
        deopt
    }; // falsy → else
    let fall = if want_f {
        defer_edge(b, stack, j + 1, j, pending, frame).or_bail("backward-jump")?
    } else {
        deopt
    }; // truthy → fall-through
    let targs: Vec<BlockArg> = Vec::new();
    let fargs: Vec<BlockArg> = Vec::new();
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
                    let i = b.ins().iadd_imm_s(frame.base, k as i64);
                    let o = b.ins().imul_imm_s(i, STRIDE);
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
                Op::Handle(w0, w1, _) => (b.ins().band_imm_s(w0, 0xff), w1),
                _ => unreachable!(),
            };
            // falsy = (tag == Nil) || (tag == Bool && payload == 0). Nil's
            // discriminant is 0.
            let is_nil = b.ins().icmp_imm_s(IntCC::Equal, tagv, 0);
            let is_bool = b.ins().icmp_imm_s(IntCC::Equal, tagv, TAG_BOOL as i64);
            // A `Value::Bool`'s payload word is only meaningful in its low byte (the
            // `bool`): Rust leaves the upper 7 bytes of the union slot uninitialised, so
            // comparing the full `i64` to 0 spuriously reads `false` (byte 0, garbage
            // above) as *truthy*. Mask to the bool byte — matching the VM's
            // `Value::Bool(b)` read. (This is the bug that corrupted `nest format` once
            // `not`/bool-const arms tiered: `(if x false true)` read its `false` arg as
            // truthy.)
            let pl_byte = b.ins().band_imm_s(payload, 0xff);
            let pl_false = b.ins().icmp_imm_s(IntCC::Equal, pl_byte, 0);
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
