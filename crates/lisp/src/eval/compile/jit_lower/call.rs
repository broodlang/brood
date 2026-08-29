//! Call arm bodies for `jit_lower_arm_inner`'s emit loop — the general `Call`
//! (tail / non-tail, with the in-IR epoch-guarded fast link) and the direct
//! `SelfCall` (self-tail loop back-edge). Extracted from the emit loop as part of the
//! `jit_lower_arm_inner` decomposition; jit-only. `Call` returns a [`Flow`] (tail →
//! `Break` the caller's inner loop, non-tail → `Fall` through); `SelfCall` is always a
//! terminator, so the caller `break`s after it. `None` bails the whole arm to the VM.
#![cfg(feature = "jit")]
use super::emit::{box_scalar, read_words, store_int, store_op, Frame, Funcs, TICK_BATCH};
use super::Op;
use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_BOOL, TAG_FLOAT};
use crate::core::value::Symbol;
use crate::eval::compile::inline::icall_enabled;
use cranelift_codegen::ir::BlockArg;
use cranelift_codegen::ir::{
    condcodes::IntCC, types, Block, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Value,
};
use cranelift_frontend::{FunctionBuilder, Variable};

/// The size of a `Value` in bytes — the frame-slot stride in `roots`.
const STRIDE: i64 = std::mem::size_of::<crate::core::value::Value>() as i64;

/// How the `Call` arm ends: a tail call is a block terminator (the caller must `break`
/// its inner loop); a non-tail call falls through to the next instruction.
pub(super) enum Flow {
    Fall,
    Break,
}

/// `Inst::MakeClosure` — a `(fn …)` literal. The callback runs `exec_chunk`'s arm verbatim,
/// so this only has to put the world in the shape that arm expects: the `ncap` capture
/// values staged on top of `roots` (where the VM's operand stack leaves them), then one
/// `brood_rt_make_closure(heap, out, inst)` call, then the fresh closure read back from
/// `out` as a `Handle`.
///
/// The callback is a **safepoint** (it allocates, and `make_closure_cached` can promote),
/// so it takes the same two disciplines a non-tail `Call` does: every live `Handle` deeper
/// on the operand stack is spilled to a reserved frame slot first (GC-visible; `jit_spill_reserve`
/// counts `MakeClosure` as a producer, so the reserve exists), and the roots base is
/// re-fetched after the call (the staging `push_room` may have reallocated `roots`).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_make_closure(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    spill_next: &mut usize,
    ncap: usize,
    inst: *const crate::eval::compile::Inst,
    spill_base: usize,
    reserve: usize,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let ptr_ty = funcs.ptr_ty;
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    // Spill deeper live Handles across the safepoint — same loop, same reasoning as
    // `emit_call` above.
    let below = stack.len().checked_sub(ncap)?;
    for d in 0..below {
        if matches!(stack[d], Op::Handle(..)) {
            if *spill_next >= reserve {
                return None;
            }
            let slot = spill_base + *spill_next;
            *spill_next += 1;
            store_op(b, slot as i64, stack[d], frame);
            stack[d] = Op::Slot(slot);
        }
    }
    // Pop the captures (top of stack = last in source order), read all words BEFORE the
    // staging push (push_room may realloc `roots`, so no slot read after it).
    let mut ops: Vec<Op> = Vec::with_capacity(ncap);
    for _ in 0..ncap {
        ops.push(stack.pop()?);
    }
    ops.reverse(); // back to `names` order — the order exec_chunk reads them in
    let mut worded: Vec<[Value; 3]> = Vec::with_capacity(ncap);
    for &op in &ops {
        worded.push(read_words(b, op, frame));
    }
    if ncap > 0 {
        let stage_n = b.ins().iconst(types::I64, ncap as i64);
        let prc = b.ins().call(funcs.pushroom, &[heap, stage_n]);
        let stage_ptr = b.inst_results(prc)[0];
        for (i, w) in worded.iter().enumerate() {
            let off = (i * STRIDE as usize) as i32;
            b.ins().store(MemFlagsData::trusted(), w[0], stage_ptr, off);
            b.ins().store(
                MemFlagsData::trusted(),
                w[1],
                stage_ptr,
                off + PAYLOAD_OFFSET as i32,
            );
            b.ins().store(
                MemFlagsData::trusted(),
                w[2],
                stage_ptr,
                off + PAYLOAD_OFFSET as i32 + 8,
            );
        }
    }
    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
    let inst_v = b.ins().iconst(ptr_ty, inst as i64);
    let c = b.ins().call(funcs.mkclo, &[heap, out_addr, inst_v]);
    let status = b.inst_results(c)[0];
    // The callback (and the staging push before it) may have reallocated `roots`.
    let rbc = b.ins().call(funcs.rb, &[heap]);
    b.def_var(frame.rb_var, b.inst_results(rbc)[0]);
    let cont = b.create_block();
    b.ins().brif(status, funcs.error, &[], cont, &[]);
    b.seal_block(cont);
    b.switch_to_block(cont);
    let w0 = b.ins().stack_load(types::I64, out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b
        .ins()
        .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
    stack.push(Op::Handle(w0, w1, w2));
    Some(())
}

/// `Inst::Call` — a general combination. Spills any live `Handle` below the call's
/// operands into reserved frame slots (GC-visible across the callee's safepoint), stages
/// the operands into a per-site stack slot + `roots`, then either returns via the
/// `tailcall` exit (tail) or dispatches inline through the epoch-guarded fast link with a
/// slow-dispatch fallback (non-tail). Returns `None` to bail the arm.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_call(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    spill_next: &mut usize,
    argc: usize,
    tail: bool,
    site: u32,
    head: Option<Symbol>,
    spill_base: usize,
    reserve: usize,
    epoch_ptr: Option<Value>,
    tailcall: Block,
    frame: Frame,
    funcs: Funcs,
) -> Option<Flow> {
    let heap = funcs.heap;
    let out_slot = funcs.out_slot;
    let ptr_ty = funcs.ptr_ty;
    let error = funcs.error;
    let rb_var = frame.rb_var;
    let call_site = site;
    // The call-head symbol, for the call-site inline cache in `jit_dispatch_call` (only
    // meaningful when `site != NO_SITE`, i.e. a free-global head). `u32::MAX` stands in
    // for a computed/local head.
    let call_head = head.unwrap_or(u32::MAX);
    // Operands consumed by the call. A **free-global** head (`head = Some`) isn't staged
    // — the compiler emits no head `Global`, so the operand stack holds only the `argc`
    // args; `jit_dispatch_call` resolves the callee via the call IC. A **computed** head
    // leaves the callee staged below the args (`argc + 1` operands).
    let n_ops = if head.is_some() { argc } else { argc + 1 };
    #[cfg(debug_assertions)]
    {
        let sv = b.ins().iconst(types::I32, call_site as i64);
        b.ins().call(funcs.dbg_staging, &[heap, sv]);
    }
    // The call is a safepoint (the callee runs arbitrary Brood and may GC). A live
    // `Handle` left on the operand stack BELOW the call's own operands would be a heap
    // pointer in a register across the collection → stale. `Slot`/`Int` are safe (a slot
    // lives in `roots`, GC-visible; an int is not a handle). So **spill** each deeper
    // `Handle` into a reserved frame slot (GC-visible, relocated correctly by the
    // callee's safepoint) and replace it with that `Slot` — this is what lets two-call
    // recursion (`(+ (fib …) (fib …))`, bintree `check`) lower instead of bailing. The
    // store writes the handle's three words into the frame *before* any `brood_rt_push`
    // (which may realloc `roots`), so the read-all-then-stage discipline below is
    // preserved. Out of reserved slots → bail to the VM.
    let below = stack.len().checked_sub(n_ops)?;
    for d in 0..below {
        if matches!(stack[d], Op::Handle(..)) {
            if *spill_next >= reserve {
                return None;
            }
            let slot = spill_base + *spill_next;
            *spill_next += 1;
            store_op(b, slot as i64, stack[d], frame);
            stack[d] = Op::Slot(slot);
        }
    }
    // Pop the operands (computed callee deepest, then args), then read each into registers
    // BEFORE staging — a `brood_rt_push` may reallocate `roots`, so no slot read may run
    // after a push (the read-all-then-store discipline, same as `SelfCall`).
    let mut ops: Vec<Op> = Vec::with_capacity(n_ops);
    for _ in 0..n_ops {
        ops.push(stack.pop()?);
    }
    ops.reverse(); // computed callee (if any) first, then args in source order
    let mut worded: Vec<[Value; 3]> = Vec::with_capacity(ops.len());
    for &op in &ops {
        worded.push(read_words(b, op, frame));
    }
    // ---- Batch staging (BEAM X-register style) ----
    // Operands are written **straight into `roots`**: one `brood_rt_push_room` reserves the
    // block and hands back its address, and the same stores that used to fill a per-site
    // stack slot now land in place. Layout: [callee?][arg0..arg_{argc-1}], 24 bytes each,
    // all three words written (a whole-`Value` copy must carry the third).
    //
    // It used to go via a stack slot and then one `brood_rt_push_n` block copy. LBR put that
    // copy at 4.4% of `bintree` on its own — 24-72 bytes per call, so almost entirely libc
    // `memmove`'s size-class dispatch, ~8M times — with the stack stores on top. Inlining
    // the copy for small arities was tried first and moved ~1% (inside the floor): the bytes
    // still had to move. This deletes the copy instead. See docs/compute-frontier.md §2j.
    //
    // The reserved slots are live roots holding uninitialised memory until the stores below
    // complete, so **nothing between here and them may allocate or collect** — they are pure
    // stores, and `Heap::push_roots_room` documents the invariant.
    // For a free-global tail call, jit_dispatch_tail reads [callee, args…] from roots —
    // but the elided head is never staged. Resolve it via the global IC and put it at slot
    // 0, args after.
    // The elided-head resolution below is a CALL, so it must happen before the room is
    // reserved — nothing may run between the reservation and the stores. Its words are held
    // back and stored with the rest.
    let mut staged_callee: Option<[cranelift_codegen::ir::Value; 3]> = None;
    let arg_base: i32 = if tail && head.is_some() {
        let sym_v2 = b.ins().iconst(types::I32, call_head as i64);
        let site_v2 = b.ins().iconst(types::I32, call_site as i64);
        let out_a = b.ins().stack_addr(ptr_ty, out_slot, 0);
        let cv = b.ins().call(funcs.globic, &[heap, out_a, sym_v2, site_v2]);
        let cstatus = b.inst_results(cv)[0];
        let callee_ok = b.create_block();
        b.ins().brif(cstatus, error, &[], callee_ok, &[]);
        b.switch_to_block(callee_ok);
        let cw0 = b.ins().stack_load(types::I64, out_slot, 0);
        let cw1 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let cw2 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        Some([cw0, cw1, cw2])
    } else {
        None
    }
    .map_or(0i32, |callee_words| {
        staged_callee = Some(callee_words);
        1
    });
    // Reserve the block, then store every operand into it — `[callee?, args…]`, the VM's
    // `Inst::Call` layout that `brood_rt_call_slow` / `jit_dispatch_tail` / the fast frame
    // all read back.
    let stage_n_i = (arg_base as i64) + n_ops as i64;
    let stage_n = b.ins().iconst(types::I64, stage_n_i);
    let prc = b.ins().call(funcs.pushroom, &[heap, stage_n]);
    let stage_ptr = b.inst_results(prc)[0];
    let store_words = |b: &mut FunctionBuilder, slot: i32, w: [cranelift_codegen::ir::Value; 3]| {
        let off = slot * STRIDE as i32;
        b.ins().store(MemFlagsData::trusted(), w[0], stage_ptr, off);
        b.ins().store(
            MemFlagsData::trusted(),
            w[1],
            stage_ptr,
            off + PAYLOAD_OFFSET as i32,
        );
        b.ins().store(
            MemFlagsData::trusted(),
            w[2],
            stage_ptr,
            off + PAYLOAD_OFFSET as i32 + 8,
        );
    };
    if let Some(cw) = staged_callee {
        store_words(b, 0, cw);
    }
    for (i, w) in worded.iter().enumerate() {
        store_words(b, arg_base + i as i32, *w);
    }
    if tail {
        // Tail position: the staged call *is* this arm's result (TCO). It ends the block
        // — nothing may remain on the operand stack below it (a real tail call's stack is
        // exactly `[callee, args]`). Return outcome 4; `vm_run_bc` dispatches the staged
        // call with `tail = true` and reuses this frame, so the native stack never grows.
        if !stack.is_empty() {
            return None;
        }
        b.ins().jump(tailcall, &[]);
        return Some(Flow::Break);
    }
    // Non-tail: dispatch through the interpreter inline (a safepoint): result →
    // `out_slot`, status in a register.
    let out_addr = b.ins().stack_addr(ptr_ty, out_slot, 0);
    let argc_v = b.ins().iconst(types::I32, argc as i64);
    let site_v = b.ins().iconst(types::I32, call_site as i64);
    let head_v = b.ins().iconst(types::I32, call_head as i64);
    // Read the result `Value` (3 words) back out of `out_slot` and push it.
    let read_out = |b: &mut FunctionBuilder| {
        let w0 = b.ins().stack_load(types::I64, out_slot, 0);
        let w1 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32);
        let w2 = b
            .ins()
            .stack_load(types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
        (w0, w1, w2)
    };
    // The shared slow-dispatch tail: call `brood_rt_call_slow`, re-fetch the roots base
    // (the callee may have relocated `roots`), and branch to `error` on a nonzero status
    // or `cont` on success. Used as the only path (icall off / computed head) and as the
    // miss path of the fast-link.
    let emit_call_slow = |b: &mut FunctionBuilder, cont: Block| {
        let c = b
            .ins()
            .call(funcs.callslow, &[heap, out_addr, argc_v, site_v, head_v]);
        let status = b.inst_results(c)[0];
        let rbc = b.ins().call(funcs.rb, &[heap]);
        b.def_var(rb_var, b.inst_results(rbc)[0]);
        b.ins().brif(status, error, &[], cont, &[]);
    };

    if icall_enabled() && head.is_some() {
        // ---- Track B / Technique A: in-IR epoch-guarded fast link ----
        // Read the flat-table base + length (re-fetched here, like the roots base, since a
        // cold nested call may have grown + reallocated it).
        use crate::core::heap::FastLink;
        const FL_SIZE: i64 = std::mem::size_of::<FastLink>() as i64;
        let fl_epoch_off = std::mem::offset_of!(FastLink, epoch) as i32;
        let fl_code_off = std::mem::offset_of!(FastLink, code) as i32;
        let fl_nslots_off = std::mem::offset_of!(FastLink, nslots) as i32;
        let fl_sym_off = std::mem::offset_of!(FastLink, sym) as i32;
        let fl_argc_off = std::mem::offset_of!(FastLink, argc) as i32;
        let len_slot =
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let len_addr = b.ins().stack_addr(ptr_ty, len_slot, 0);
        let fbc = b.ins().call(funcs.flbase, &[heap, len_addr]);
        let fl_base = b.inst_results(fbc)[0];
        let fl_len = b.ins().stack_load(types::I64, len_slot, 0);
        let site_idx = b.ins().iconst(types::I64, call_site as i64);
        // Bounds: `site < len` (a live arm whose site ids outran a post-collect re-grow
        // misses here and goes slow — the table read would be OOB).
        let in_bounds = b.ins().icmp(IntCC::UnsignedLessThan, site_idx, fl_len);
        let chk_epoch = b.create_block();
        let chk_ident = b.create_block();
        let hit = b.create_block();
        let miss = b.create_block();
        let cont = b.create_block();
        b.ins().brif(in_bounds, chk_epoch, &[], miss, &[]);

        // chk_epoch: this slot's epoch must equal the current global epoch.
        b.switch_to_block(chk_epoch);
        let stride = b.ins().iconst(types::I64, FL_SIZE);
        let off = b.ins().imul(site_idx, stride);
        let slot_ptr = b.ins().iadd(fl_base, off);
        let ep = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), slot_ptr, fl_epoch_off);
        let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when icall is on");
        let gep = b.ins().load(types::I64, MemFlagsData::trusted(), ep_ptr, 0);
        let ep_ok = b.ins().icmp(IntCC::Equal, ep, gep);
        b.ins().brif(ep_ok, chk_ident, &[], miss, &[]);

        // chk_ident: the slot must link the *same* callee this site calls. A call-site id
        // reused across a `runtime_collect` table clear (ADR-096) can leave a slot
        // populated by a different arm for a different callee; the epoch guard alone
        // wouldn't catch it (same epoch). Match the slot's resolved `sym`/`argc` against
        // this site's baked `head`/`argc` — exactly the validation the IC probe paths do —
        // or fall to the slow path, which re-resolves correctly. Without this the fast
        // path would jump into the wrong native code with the wrong arity (a SIGSEGV in
        // release).
        b.switch_to_block(chk_ident);
        let slot_sym = b
            .ins()
            .load(types::I32, MemFlagsData::trusted(), slot_ptr, fl_sym_off);
        let sym_ok = b.ins().icmp(IntCC::Equal, slot_sym, head_v);
        let slot_argc = b
            .ins()
            .load(types::I32, MemFlagsData::trusted(), slot_ptr, fl_argc_off);
        let argc_ok = b.ins().icmp(IntCC::Equal, slot_argc, argc_v);
        let ident_ok = b.ins().band(sym_ok, argc_ok);
        b.ins().brif(ident_ok, hit, &[], miss, &[]);

        // hit: read (code, nslots, env). `nslots == u32::MAX` marks a NATIVE flat cell (a
        // builtin callee, arity pre-validated at publish): call the fn pointer directly on
        // the staging slot — no frame, no env_get, no dispatch. Otherwise run the Brood
        // fast frame exactly as before.
        b.switch_to_block(hit);
        let code_v = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), slot_ptr, fl_code_off);
        let nslots_v = b
            .ins()
            .load(types::I32, MemFlagsData::trusted(), slot_ptr, fl_nslots_off);
        let is_native = b.ins().icmp_imm(IntCC::Equal, nslots_v, u32::MAX as i64);
        let nat_blk = b.create_block();
        let brood_blk = b.create_block();
        b.ins().brif(is_native, nat_blk, &[], brood_blk, &[]);

        // Native flat cell: one trampoline call; the staged roots copies anchor the args
        // for any GC inside (the trampoline drops them).
        b.switch_to_block(nat_blk);
        let nfc = b
            .ins()
            .call(funcs.natfl, &[heap, out_addr, code_v, stage_ptr, argc_v]);
        let nst = b.inst_results(nfc)[0];
        let rbc_n = b.ins().call(funcs.rb, &[heap]);
        b.def_var(rb_var, b.inst_results(rbc_n)[0]);
        b.ins().brif(nst, error, &[], cont, &[]);

        b.switch_to_block(brood_blk);
        // Pass the **slot pointer**, not its unpacked fields. `brood_rt_fast_frame` used to
        // take ten arguments — head/argc/nslots/code/env and the two callee IC bases, all
        // read here and immediately re-pushed on the other side, because SysV only passes
        // six in registers. Four arguments fit in registers, and the callee's reads are free:
        // the guard blocks above have just touched that cache line to check the slot's epoch,
        // sym and argc. See `brood_rt_fast_frame`'s doc.
        let ffc = b
            .ins()
            .call(funcs.fastframe, &[heap, out_addr, site_v, slot_ptr]);
        let fst = b.inst_results(ffc)[0];
        // The callee may have relocated `roots`; re-fetch the base.
        let rbc = b.ins().call(funcs.rb, &[heap]);
        b.def_var(rb_var, b.inst_results(rbc)[0]);
        // status: 1 = error → `error`; 2 = could-not-link → `miss`; 0 = `cont`.
        let is_err = b.ins().icmp_imm(IntCC::Equal, fst, 1);
        let not_err = b.create_block();
        b.ins().brif(is_err, error, &[], not_err, &[]);
        b.switch_to_block(not_err);
        let is_slow = b.ins().icmp_imm(IntCC::Equal, fst, 2);
        b.ins().brif(is_slow, miss, &[], cont, &[]);

        // miss: cold / redefined / over-cap / IC-moved → the slow dispatch.
        b.switch_to_block(miss);
        emit_call_slow(b, cont);

        b.switch_to_block(cont);
        let (w0, w1, w2) = read_out(b);
        stack.push(Op::Handle(w0, w1, w2));
    } else {
        let cont = b.create_block();
        emit_call_slow(b, cont);
        b.switch_to_block(cont);
        let (w0, w1, w2) = read_out(b);
        stack.push(Op::Handle(w0, w1, w2));
    }
    Some(Flow::Fall)
}

/// `Inst::SelfCall { argc }` — a direct `letrec` self-tail call (the loop back-edge).
/// Reads the `argc` new args into registers, writes them into frame slots `0..argc`,
/// keeps `carry_vars` in sync, runs the (cons-only) GC safepoint + checkpoint reset, and
/// closes the batched back-edge (reduction poll + hoisted-global epoch guard). Always a
/// terminator — the caller `break`s. Returns `None` to bail the arm.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_self_call(
    b: &mut FunctionBuilder,
    stack: &mut Vec<Op>,
    argc: usize,
    leader_block: &[Option<Block>],
    preempt: Block,
    tick_budget: Variable,
    entry_epoch: Option<Value>,
    epoch_ptr: Option<Value>,
    has_cons: bool,
    ckpt_active: bool,
    ckpt_slot: u32,
    frame: Frame,
    funcs: Funcs,
) -> Option<()> {
    let heap = funcs.heap;
    let deopt = frame.deopt;
    let rb_var = frame.rb_var;
    let base = frame.base;
    let carry_vars = frame.carry_vars;
    // Tail self-call (loop back-edge): pop the argc new args and write them into frame
    // slots `0..argc`. Read every arg's `Value` into registers FIRST, then store — an arg
    // may reference a slot being overwritten (e.g. `(f b a)`), so a read-as-you-store
    // would alias. The reads are safepoint-free, so even a handle's bits are safe in a
    // register here.
    let mut ops = Vec::with_capacity(argc);
    for _ in 0..argc {
        ops.push(stack.pop()?);
    }
    ops.reverse(); // ops[i] = the i-th positional arg → frame slot i
    if !stack.is_empty() {
        return None;
    }
    // Each arg becomes a list of (byte-offset, word) stores. An `Int` is boxed (tag at 0,
    // payload at PAYLOAD_OFFSET — the third word is left alone, irrelevant to an Int). A
    // `Slot` copies **every** word of the `Value` (tag/payload/…) so a handle — including
    // a `Pid` whose `id` is the third word at offset 16 — moves intact.
    let mut vals: Vec<Vec<(i32, Value)>> = Vec::with_capacity(argc);
    for &op in &ops {
        match op {
            Op::Int(v) => {
                // Box as `Int`, or (a comparison `i8`) `Bool` — a loop can carry a
                // boolean arg.
                let (tag_byte, payload) = box_scalar(b, v);
                let tag = b.ins().iconst(types::I8, tag_byte as i64);
                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, payload)]);
            }
            Op::Slot(k) => {
                let roots_base = b.use_var(rb_var);
                let i = b.ins().iadd_imm(base, k as i64);
                let o = b.ins().imul_imm(i, STRIDE);
                let addr = b.ins().iadd(roots_base, o);
                let mut words = Vec::new();
                let mut off = 0i32;
                while (off as i64) < STRIDE {
                    words.push((
                        off,
                        b.ins().load(types::I64, MemFlagsData::trusted(), addr, off),
                    ));
                    off += 8;
                }
                vals.push(words);
            }
            // A freshly-produced handle (cons/car/cdr result): its three words are already
            // in registers — store all three.
            Op::Handle(w0, w1, w2) => {
                vals.push(vec![
                    (0, w0),
                    (PAYLOAD_OFFSET as i32, w1),
                    (PAYLOAD_OFFSET as i32 + 8, w2),
                ]);
            }
            // A hoisted global vector/table passed as a self-call arg — moves its three
            // entry-resolved words verbatim, like a `Handle`.
            Op::HoistedVec { w0, w1, w2, .. } | Op::HoistedTable { w0, w1, w2, .. } => {
                vals.push(vec![
                    (0, w0),
                    (PAYLOAD_OFFSET as i32, w1),
                    (PAYLOAD_OFFSET as i32 + 8, w2),
                ]);
            }
            // A float arg — box as Value::Float (TAG_FLOAT + bits). The next iteration
            // reads it back via `as_f64` (tag-checked).
            Op::Float(v) => {
                let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), v);
                let tag = b.ins().iconst(types::I8, TAG_FLOAT as i64);
                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, bits)]);
            }
            // A crossed-boundary boolean (already `i64` 0/1) → Value::Bool.
            Op::Bool(v) => {
                let tag = b.ins().iconst(types::I8, TAG_BOOL as i64);
                vals.push(vec![(0, tag), (PAYLOAD_OFFSET as i32, v)]);
            }
        }
    }
    let roots_base = b.use_var(rb_var);
    for (i, words) in vals.iter().enumerate() {
        let idx = b.ins().iadd_imm(base, i as i64);
        let o = b.ins().imul_imm(idx, STRIDE);
        let addr = b.ins().iadd(roots_base, o);
        for &(off, w) in words {
            b.ins().store(MemFlagsData::trusted(), w, addr, off);
        }
    }
    // Register-carry update: keep carry_vars in sync with the new slot values. The `roots`
    // stores above are kept for deopt; this additionally def_var's the unboxed i64/f64 so
    // subsequent load_slot_int/as_f64 skip the tag-check. For Op::Int/Float, use the raw
    // value directly. For any other op (slot passthrough), load from the just-stored roots
    // payload — always correct and avoids parallel-assignment issues with cross-slot
    // references.
    if !carry_vars.is_empty() {
        let rb2 = b.use_var(rb_var);
        for (k, (&op, entry)) in ops.iter().zip(carry_vars.iter()).enumerate() {
            let (var, is_float) = match *entry {
                Some(x) => x,
                None => continue, // handle slot: only the frame store above applies
            };
            if is_float {
                let f = match op {
                    Op::Float(v) => v,
                    _ => {
                        let idx = b.ins().iadd_imm(base, k as i64);
                        let o = b.ins().imul_imm(idx, STRIDE);
                        let addr = b.ins().iadd(rb2, o);
                        let bits = b.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            addr,
                            PAYLOAD_OFFSET as i32,
                        );
                        b.ins().bitcast(types::F64, MemFlagsData::new(), bits)
                    }
                };
                b.def_var(var, f);
            } else {
                let raw = match op {
                    Op::Int(v) => {
                        if b.func.dfg.value_type(v) == types::I64 {
                            v
                        } else {
                            b.ins().uextend(types::I64, v)
                        }
                    }
                    _ => {
                        let idx = b.ins().iadd_imm(base, k as i64);
                        let o = b.ins().imul_imm(idx, STRIDE);
                        let addr = b.ins().iadd(rb2, o);
                        b.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            addr,
                            PAYLOAD_OFFSET as i32,
                        )
                    }
                };
                b.def_var(var, raw);
            }
        }
    }
    // GC safepoint (cons-allocating arms only): bound the nursery over loop iterations.
    // Placed here — args already stored to slots, operand stack empty — so no handle is
    // live in a register across the collection; the collector relocates the frame slots in
    // place, leaving `roots_base` valid. (`car`/`rest` don't allocate, so non-cons arms
    // skip it.)
    if has_cons {
        b.ins().call(funcs.sp, &[heap]);
    }
    // Back-edge checkpoint reset (see `CompiledArm::ckpt_slot`): the frame was just reset
    // to the next iteration's args — a deopt from here on resumes at ip 0 with an empty
    // stack, which re-executes only this fresh iteration's (so-far-nonexistent) work.
    if ckpt_active {
        let zero = b.ins().iconst(types::I64, 0);
        store_int(b, ckpt_slot as i64, zero, frame);
    }
    // Back-edge bookkeeping, BATCHED (BEAM-style): decrement the in-register countdown;
    // while nonzero the loop resumes with ONE sub + branch — no FFI, no TLS, no guard
    // load. Every `TICK_BATCH` iterations the poll block settles the reduction account
    // (`brood_rt_tick_n`, preempting exactly like the old per-iteration tick, at the same
    // reduction rate) and runs the hoisted-global epoch guard (a rebind is observed within
    // one batch — the guard's "eventually" contract; the frame slots hold the current
    // iteration's args every iteration, so both deopt and preempt resume exactly).
    let loop_top = leader_block[0]?;
    let bv = b.use_var(tick_budget);
    let nv = b.ins().iadd_imm(bv, -1);
    b.def_var(tick_budget, nv);
    let poll = b.create_block();
    b.ins().brif(nv, loop_top, &[], poll, &[]);
    b.switch_to_block(poll);
    {
        let refill = b.ins().iconst(types::I64, TICK_BATCH);
        b.def_var(tick_budget, refill);
    }
    if let Some(entry_ep) = entry_epoch {
        let ep_ptr = epoch_ptr.expect("epoch_ptr fetched when a global is hoisted");
        let now_ep = b.ins().load(types::I64, MemFlagsData::trusted(), ep_ptr, 0);
        let changed = b.ins().icmp(IntCC::NotEqual, now_ep, entry_ep);
        let ck = b.create_block();
        let __dr = b.ins().iconst(types::I32, 36);
        b.ins()
            .brif(changed, deopt, &[BlockArg::Value(__dr)], ck, &[]);
        b.switch_to_block(ck);
    }
    let batch = b.ins().iconst(types::I64, TICK_BATCH);
    let tc = b.ins().call(funcs.tickn, &[heap, batch]);
    let yld = b.inst_results(tc)[0];
    b.ins().brif(yld, preempt, &[], loop_top, &[]);
    Some(())
}
