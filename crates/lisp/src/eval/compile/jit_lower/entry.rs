//! The entry block of a lowered arm: what is decided once per activation before the
//! first leader runs — the locals nilled, the native stack guard, the loop-invariant
//! hoists (the LICM levers), the register-carried params loaded and tag-checked, and the
//! deopt journals reset. jit-only.
#![cfg(feature = "jit")]
use super::callbacks::CallbackRefs;
use super::emit::{DeoptReason, deopt_code, deopt_code_with_constant, read_out_slot};
use super::*;
use crate::core::value::jit_layout::{PAYLOAD_OFFSET, TAG_FLOAT, TAG_INT};
use cranelift_codegen::ir::{
    BlockArg, InstBuilder, MemFlagsData, StackSlot, StackSlotData, StackSlotKind, Type,
    condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, Variable};
use std::collections::{HashMap, HashSet};

type SsaValue = cranelift_codegen::ir::Value;

const STRIDE: i64 = std::mem::size_of::<Value>() as i64;

/// What the loop-invariant hoists may resolve once at entry instead of per iteration.
pub(super) struct HoistPlan {
    /// Invariant vector params read as a fused `(nth slot index)` operand.
    pub(super) slots: Vec<usize>,
    /// Globals read as a `(nth GLOBAL index)` vector operand.
    pub(super) globals: HashSet<Symbol>,
    /// Every other global read in value position (`n` in `(>= i n)`).
    pub(super) scalar_globals: HashSet<Symbol>,
}

/// Decide the hoists (matmul-style LICM). A vector slot the arm carries unchanged every
/// iteration has an *immutable* element base resolved once at entry and read inline
/// (`ptr + index*STRIDE`) instead of a `brood_rt_vector_ref` per element — sound with no
/// alias analysis because Brood data cannot be mutated (ADR-026). A global read in a no-call
/// arm is loop-invariant too: resolved once, with the back-edge `entry_epoch` guard deopting
/// on a concurrent `def` rebind, so it stays bit-identical to the VM's late binding.
///
/// Gated to arms that neither allocate (`cons`/vector build → LOCAL GC) nor make a
/// Brood→Brood call (could `def` → RUNTIME compaction): under that gate nothing runs mid-arm
/// to relocate the storage, and a preempt/deopt re-enters from the entry block (re-hoist).
/// The LOCAL element-base hoist takes the STRICTER [`vector_base_hoist_safe`] gate, because
/// nothing refreshes the raw pointer it hoists.
pub(super) fn plan_hoists(code: &[Inst], lower_body: &Node, nrequired: usize) -> HoistPlan {
    let invariant = invariant_param_slots(lower_body, nrequired);
    let hoist_safe = !code.iter().any(|i| {
        matches!(
            i,
            Inst::Call { .. }
                | Inst::MakeVector(_)
                | Inst::MakeMap(_)
                | Inst::Prim2 {
                    op: PrimOp::Cons,
                    ..
                }
                | Inst::Prim2SlotSlot {
                    op: PrimOp::Cons,
                    ..
                }
                | Inst::Prim2SlotInt {
                    op: PrimOp::Cons,
                    ..
                }
        )
    });
    let mut slots: Vec<usize> = Vec::new();
    if vector_base_hoist_safe(code) {
        for i in code.iter() {
            if let Inst::Prim2SlotSlot {
                op: PrimOp::VectorRef,
                slot_a,
                ..
            } = i
                && invariant.get(*slot_a).copied().unwrap_or(false)
                && !slots.contains(slot_a)
            {
                slots.push(*slot_a);
            }
        }
    }
    let mut globals: HashSet<Symbol> = HashSet::new();
    if hoist_safe {
        invariant_global_vecs(lower_body, &mut globals);
    }
    let mut scalar_globals: HashSet<Symbol> = HashSet::new();
    if hoist_safe {
        for i in code.iter() {
            if let Inst::Global(s) | Inst::GlobalIc { sym: s, .. } = i
                && !globals.contains(s)
            {
                scalar_globals.insert(*s);
            }
        }
    }
    HoistPlan {
        slots,
        globals,
        scalar_globals,
    }
}

/// **The callee nils its own locals.** At entry only the parameters (and, for a closure arm
/// entered from the VM, its optionals/rest/captures, which `push_frame` filled) are live;
/// every slot from the first `let` binder up to `nslots` is dead and may hold whatever the
/// roots buffer's spare capacity held from earlier frames — a stale handle the collector
/// would trace at the first safepoint (KI-49's class). The callee knows its frame
/// statically, so this is unrolled: three stores per slot. Emitted before the stack guard,
/// deliberately: a guard trip deopts, and the VM then owns a frame that must already be nil
/// past its args.
pub(super) fn emit_nil_locals(
    b: &mut FunctionBuilder,
    arm: &CompiledArm,
    nslots: usize,
    base: SsaValue,
    roots_base: SsaValue,
) {
    let first_local =
        arm.nrequired + arm.noptional + arm.rest_slot.is_some() as usize + arm.capture_names.len();
    if first_local >= nslots {
        return;
    }
    let zero = b.ins().iconst(types::I64, 0);
    let base_bytes = b.ins().imul_imm_s(base, STRIDE);
    let frame_pointer = b.ins().iadd(roots_base, base_bytes);
    for slot in first_local..nslots {
        // A missing `&optional` default's flag is live at entry too: `push_frame` set it,
        // and the arm's prologue reads it to decide whether to run the default. Nilling it
        // here skipped every default, natively and again after a deopt, which re-runs this
        // frame from the top. Only `push_frame` enters an arm with optionals (a direct
        // native call refuses one, `vm_direct_eligible`), and it zero-fills the frame, so a
        // flag it did not set is already nil.
        if arm.optional_flags.contains(&(slot as u32)) {
            continue;
        }
        let offset = (slot as i64 * STRIDE) as i32;
        b.ins()
            .store(MemFlagsData::trusted(), zero, frame_pointer, offset);
        b.ins().store(
            MemFlagsData::trusted(),
            zero,
            frame_pointer,
            offset + PAYLOAD_OFFSET as i32,
        );
        b.ins().store(
            MemFlagsData::trusted(),
            zero,
            frame_pointer,
            offset + PAYLOAD_OFFSET as i32 + 8,
        );
    }
}

/// **Stack guard (KI-14).** Every native frame passes through here, whatever route created
/// it — the dispatch-path guards (`jit_native_depth`, the `stacker` headroom probe) only
/// bound recursion that goes through a fast link, and a green process's worker dying on its
/// guard page is an abort no `try` or supervisor can observe.
///
/// Cost: a load of the limit the entry point stamped (`Heap::jit_stack_limit`), one compare
/// of this frame's address against it, and a predicted branch. A trip first asks the runtime
/// (`brood_rt_stack_recheck`): the stamp is an address on the stack the native nest began on,
/// and a frame on another stack (a `stacker` segment, a stamp from another thread) trips it
/// with room to spare — every arm under the ASan build deopted sixteen times and latched off
/// native that way. The re-check re-stamps for this stack; only a real shortage deopts (the
/// VM owns deep recursion: heap frames and the clean `MAX_BC_FRAMES` error). A `0` limit (the
/// probe could not read the stack) never trips, because the compare is unsigned.
pub(super) fn emit_stack_guard(
    b: &mut FunctionBuilder,
    pointer_type: Type,
    heap: SsaValue,
    deopt: cranelift_codegen::ir::Block,
    stack_recheck: cranelift_codegen::ir::FuncRef,
) {
    let limit = b.ins().load(
        pointer_type,
        MemFlagsData::trusted(),
        heap,
        std::mem::offset_of!(crate::core::heap::Heap, jit_stack_limit) as i32,
    );
    // The address of a scratch slot in *this* frame stands in for the stack pointer; the
    // stack grows down, so `here < limit` means we have run past the budget.
    let probe = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let here = b.ins().stack_addr(pointer_type, probe, 0);
    let trip = b.ins().icmp(IntCC::UnsignedLessThan, here, limit);
    let cont = b.create_block();
    let recheck = b.create_block();
    let bail = b.create_block();
    b.ins().brif(trip, recheck, &[], cont, &[]);
    b.switch_to_block(recheck);
    let call = b.ins().call(stack_recheck, &[heap]);
    let room = b.inst_results(call)[0];
    b.ins().brif(room, cont, &[], bail, &[]);
    b.seal_block(recheck);
    // On a trip, set `Heap::jit_force_vm` before deopting. Deopt alone livelocks: the VM
    // re-runs the arm, the recursive callee tiers again, its prologue trips again. The flag
    // makes `jit_tier` decline native for the rest of this subtree (the native entry points
    // save/restore it), so the recursion drains through the bounded heap-frame loop.
    b.switch_to_block(bail);
    let one = b.ins().iconst(types::I8, 1);
    b.ins().store(
        MemFlagsData::trusted(),
        one,
        heap,
        std::mem::offset_of!(crate::core::heap::Heap, jit_force_vm) as i32,
    );
    let deopt_reason = deopt_code(b, DeoptReason::NativeDepthExceeded);
    b.ins().jump(deopt, &[BlockArg::Value(deopt_reason)]);
    b.seal_block(bail);
    b.switch_to_block(cont);
}

/// The global-epoch counter's address, fetched once in the entry block (which dominates
/// every loop/call block) when the arm reads the epoch on a hot path — a self-loop's
/// back-edge guard (which every `SelfCall` arm has; see `jit_lower`'s `entry_epoch`), or a
/// per-call epoch check. Those sites then do a raw load.
pub(super) fn emit_epoch_pointer(
    b: &mut FunctionBuilder,
    code: &[Inst],
    plan: &HoistPlan,
    callbacks: &CallbackRefs,
    heap: SsaValue,
) -> Option<SsaValue> {
    let needs = !plan.globals.is_empty()
        || !plan.scalar_globals.is_empty()
        || code.iter().any(|i| matches!(i, Inst::SelfCall { .. }))
        || (crate::diagnostics::debug_flags::jit_inline_call()
            && code.iter().any(|i| {
                matches!(
                    i,
                    Inst::Call {
                        tail: false,
                        head: Some(_),
                        ..
                    }
                )
            }));
    if needs {
        let c = b.ins().call(callbacks.global_epoch_pointer, &[heap]);
        Some(b.inst_results(c)[0])
    } else {
        None
    }
}

/// Inline `first`/`rest` pair reads: when the arm uses them but has no allocation
/// safepoint and no non-tail call (either can run `minor_collect`, which replaces
/// `self.local` and frees the old slab), fetch the LOCAL nursery and old-gen pair-slab
/// bases once here. Held in `Variable`s, not bare SSA values: the non-pair fallback of
/// `rest` can grow the pair slab and move its base, so that path re-fetches both
/// (`prim::emit_prim1`) and every inline read `use_var`s.
pub(super) fn emit_pair_bases(
    b: &mut FunctionBuilder,
    code: &[Inst],
    callbacks: &CallbackRefs,
    heap: SsaValue,
    pointer_type: Type,
) -> Option<(Variable, Variable)> {
    let has_first_or_rest = code.iter().any(|i| {
        matches!(
            i,
            Inst::Prim1 {
                op: PrimOp1::First | PrimOp1::Rest,
                ..
            }
        )
    });
    let has_allocation_safepoint = code.iter().any(inst_may_allocate);
    let has_call_safepoint = code
        .iter()
        .any(|i| matches!(i, Inst::Call { tail: false, .. }));
    if !has_first_or_rest || has_allocation_safepoint || has_call_safepoint {
        return None;
    }
    let nursery_call = b.ins().call(callbacks.pair_nursery_base, &[heap]);
    let nursery = b.inst_results(nursery_call)[0];
    let old_call = b.ins().call(callbacks.pair_old_base, &[heap]);
    let old = b.inst_results(old_call)[0];
    let nursery_variable = b.declare_var(pointer_type);
    b.def_var(nursery_variable, nursery);
    let old_variable = b.declare_var(pointer_type);
    b.def_var(old_variable, old);
    Some((nursery_variable, old_variable))
}

/// A hoisted global vector: `(element pointer, length, w0, w1, w2)` — the words are the
/// global's entry-resolved `Value`, for any non-`VectorRef` use.
pub(super) type HoistedGlobal = (SsaValue, SsaValue, SsaValue, SsaValue, SsaValue);

/// What the entry hoists resolved — read by the emit loop.
#[derive(Default)]
pub(super) struct Hoisted {
    /// Invariant vector slot → `(element pointer, length)`.
    pub(super) slots: HashMap<usize, (SsaValue, SsaValue)>,
    pub(super) globals: HashMap<Symbol, HoistedGlobal>,
    /// Scalar global → its entry-resolved `Value` words.
    pub(super) scalars: HashMap<Symbol, (SsaValue, SsaValue, SsaValue)>,
    /// Dense-table global → `(slot region — possibly null at run time, checked per op —,
    /// dense-flag address, w0, w1, w2)`.
    pub(super) tables: HashMap<Symbol, (SsaValue, SsaValue, SsaValue, SsaValue, SsaValue)>,
    /// The `global_epoch` at entry, re-checked on the back edge, when any global is hoisted.
    pub(super) entry_epoch: Option<SsaValue>,
}

/// The values the entry hoists need from the block being built.
pub(super) struct EntryContext<'a> {
    pub(super) callbacks: &'a CallbackRefs,
    pub(super) heap: SsaValue,
    pub(super) base: SsaValue,
    pub(super) roots_base_variable: Variable,
    pub(super) context_pointer: SsaValue,
    pub(super) out_slot: StackSlot,
    pub(super) pointer_type: Type,
    pub(super) deopt: cranelift_codegen::ir::Block,
}

/// Resolve every planned hoist once, here in the entry block (which dominates every loop
/// block). A slot or global that is not a vector deopts (the VM then owns the exact
/// result). An UNBOUND global also deopts rather than raising: the hoist runs for every
/// global the arm mentions, including ones only a cold branch reads, and the VM evaluates
/// only the branch actually taken (`brood_rt_global_probe` parks no error).
pub(super) fn emit_entry_hoists(
    b: &mut FunctionBuilder,
    code: &[Inst],
    plan: &HoistPlan,
    entry: &EntryContext,
    epoch_pointer: Option<SsaValue>,
) -> Hoisted {
    let mut hoisted = Hoisted::default();
    if plan.slots.is_empty() && plan.globals.is_empty() && plan.scalar_globals.is_empty() {
        return hoisted;
    }
    let EntryContext {
        callbacks,
        heap,
        base,
        roots_base_variable,
        context_pointer,
        out_slot,
        pointer_type,
        deopt,
    } = *entry;
    // Only pay the entry-time dense-base resolution when the body has table ops a hoisted
    // table could serve.
    let chunk_has_table_ops = code.iter().any(|i| {
        matches!(
            i,
            Inst::Prim3 {
                op: PrimOp3::TablePut,
                ..
            } | Inst::Prim2 {
                op: PrimOp::TableHas,
                ..
            }
        )
    });
    let length_slot =
        b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let length_address = b.ins().stack_addr(pointer_type, length_slot, 0);
    for &slot in &plan.slots {
        let roots_base = b.use_var(roots_base_variable);
        let index = b.ins().iadd_imm_s(base, slot as i64);
        let offset = b.ins().imul_imm_s(index, STRIDE);
        let address = b.ins().iadd(roots_base, offset);
        let w0 = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), address, 0);
        let w1 = b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            address,
            PAYLOAD_OFFSET as i32,
        );
        let w2 = b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            address,
            PAYLOAD_OFFSET as i32 + 8,
        );
        let c = b
            .ins()
            .call(callbacks.vector_base, &[heap, w0, w1, w2, length_address]);
        let pointer = b.inst_results(c)[0];
        let cont = b.create_block();
        let deopt_reason = deopt_code(b, DeoptReason::HoistedSlotVectorBaseNull);
        b.ins()
            .brif(pointer, cont, &[], deopt, &[BlockArg::Value(deopt_reason)]);
        b.switch_to_block(cont);
        let length = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), length_address, 0);
        hoisted.slots.insert(slot, (pointer, length));
    }
    // Sorted for deterministic codegen.
    let mut global_symbols: Vec<Symbol> = plan.globals.iter().copied().collect();
    global_symbols.sort_unstable();
    for symbol in global_symbols {
        let out_address = b.ins().stack_addr(pointer_type, out_slot, 0);
        let symbol_value = b.ins().iconst(types::I32, symbol as i64);
        let c = b.ins().call(
            callbacks.global_probe,
            &[heap, out_address, symbol_value, context_pointer],
        );
        let status = b.inst_results(c)[0];
        let bound = b.create_block();
        let deopt_reason = deopt_code(b, DeoptReason::HoistedGlobalUnbound);
        b.ins()
            .brif(status, deopt, &[BlockArg::Value(deopt_reason)], bound, &[]);
        b.switch_to_block(bound);
        let (w0, w1, w2) = read_out_slot(b, out_slot);
        let c = b
            .ins()
            .call(callbacks.vector_base, &[heap, w0, w1, w2, length_address]);
        let pointer = b.inst_results(c)[0];
        let cont = b.create_block();
        let deopt_reason = deopt_code(b, DeoptReason::HoistedGlobalVectorBaseNull);
        b.ins()
            .brif(pointer, cont, &[], deopt, &[BlockArg::Value(deopt_reason)]);
        b.switch_to_block(cont);
        let length = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), length_address, 0);
        hoisted
            .globals
            .insert(symbol, (pointer, length, w0, w1, w2));
    }
    let mut scalar_symbols: Vec<Symbol> = plan.scalar_globals.iter().copied().collect();
    scalar_symbols.sort_unstable();
    for symbol in scalar_symbols {
        let out_address = b.ins().stack_addr(pointer_type, out_slot, 0);
        let symbol_value = b.ins().iconst(types::I32, symbol as i64);
        let c = b.ins().call(
            callbacks.global_probe,
            &[heap, out_address, symbol_value, context_pointer],
        );
        let status = b.inst_results(c)[0];
        let bound = b.create_block();
        let deopt_reason = deopt_code(b, DeoptReason::HoistedScalarGlobalUnbound);
        b.ins()
            .brif(status, deopt, &[BlockArg::Value(deopt_reason)], bound, &[]);
        b.switch_to_block(bound);
        let (w0, w1, w2) = read_out_slot(b, out_slot);
        hoisted.scalars.insert(symbol, (w0, w1, w2));
        // Dense-table hoist (the sieve lever): resolve this global's dense slot region once.
        // NO branch — a null base (non-table / hashed / dropped) is carried and checked per
        // op, so such an arm never deopts, it just uses the per-op callback.
        if chunk_has_table_ops {
            let c = b.ins().call(
                callbacks.table_dense_base,
                &[heap, w0, w1, w2, length_address],
            );
            let slots = b.inst_results(c)[0];
            let flag = b
                .ins()
                .load(types::I64, MemFlagsData::trusted(), length_address, 0);
            hoisted.tables.insert(symbol, (slots, flag, w0, w1, w2));
        }
    }
    if !hoisted.globals.is_empty() || !hoisted.scalars.is_empty() {
        let pointer = epoch_pointer.expect("epoch_pointer fetched when globals are hoisted");
        hoisted.entry_epoch = Some(
            b.ins()
                .load(types::I64, MemFlagsData::trusted(), pointer, 0),
        );
    }
    hoisted
}

/// Load each register-carried param from its frame slot (first iteration), tag-checked
/// once here; later iterations `use_var` the carried value. A tag mismatch deopts with the
/// SLOT as payload, so the feedback can mark exactly the polymorphic param
/// (`jit_any_deopt_feedback`).
pub(super) fn emit_carry_init(
    b: &mut FunctionBuilder,
    carry_vars: &[Option<(Variable, bool)>],
    roots_base_variable: Variable,
    base: SsaValue,
    deopt: cranelift_codegen::ir::Block,
) {
    for (k, carried) in carry_vars.iter().enumerate() {
        let Some((variable, is_float)) = *carried else {
            continue; // a handle/vector slot stays on the frame
        };
        let roots_base = b.use_var(roots_base_variable);
        let index = b.ins().iadd_imm_s(base, k as i64);
        let offset = b.ins().imul_imm_s(index, STRIDE);
        let address = b.ins().iadd(roots_base, offset);
        let tag = b.ins().load(types::I8, MemFlagsData::trusted(), address, 0);
        let expected_tag = if is_float {
            TAG_FLOAT as i64
        } else {
            TAG_INT as i64
        };
        let ok = b.ins().icmp_imm_s(IntCC::Equal, tag, expected_tag);
        let cont = b.create_block();
        let deopt_reason = deopt_code_with_constant(b, DeoptReason::EntryParamTag, k as i64);
        b.ins()
            .brif(ok, cont, &[], deopt, &[BlockArg::Value(deopt_reason)]);
        b.switch_to_block(cont);
        let bits = b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            address,
            PAYLOAD_OFFSET as i32,
        );
        if is_float {
            let float = b.ins().bitcast(types::F64, MemFlagsData::new(), bits);
            b.def_var(variable, float);
        } else {
            b.def_var(variable, bits);
        }
    }
}

/// Reset BOTH deopt journals in this frame, not just this lowering's: the small and the
/// leaf-spliced layouts occupy disjoint slots and take turns running the same frame, so a
/// journal left by the *other* engine's earlier run would otherwise be read as live by a
/// later resume. Packed 0 = "resume at ip 0 with an empty operand stack" — no journal.
pub(super) fn emit_checkpoint_reset(
    b: &mut FunctionBuilder,
    checkpoint_slots: [u32; 2],
    roots_base_variable: Variable,
    base: SsaValue,
) {
    let mut reset: Option<u32> = None;
    for slot in checkpoint_slots {
        if slot == u32::MAX || reset == Some(slot) {
            continue; // absent, or already reset (the small native, where the two agree)
        }
        reset = Some(slot);
        let index = b.ins().iadd_imm_s(base, slot as i64);
        let offset = b.ins().imul_imm_s(index, STRIDE);
        let roots_base = b.use_var(roots_base_variable);
        let address = b.ins().iadd(roots_base, offset);
        let tag = b.ins().iconst(types::I8, TAG_INT as i64);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().store(MemFlagsData::trusted(), tag, address, 0);
        b.ins().store(
            MemFlagsData::trusted(),
            zero,
            address,
            PAYLOAD_OFFSET as i32,
        );
    }
}

/// The register-carry plan: for a self-tail loop, each param slot `k` the tier-time
/// profile typed `Int` or `Float` is carried in a Cranelift `Variable` (SSA, phi-inserted
/// at the loop header), so its reads skip the per-access tag check and memory traffic;
/// `None` leaves a slot on the frame (a handle/vector/nil slot is GC-relocatable and must
/// stay rooted). The `roots` stores at each `SelfCall` are kept for deopt correctness —
/// only reads change — and the per-read tag checks still guard a mistyped profile.
pub(super) fn plan_carry_vars(
    b: &mut FunctionBuilder,
    code: &[Inst],
    slot_tags: &[u8],
) -> Vec<Option<(Variable, bool)>> {
    // The profile snapshots the *lattice* `Tag` enum, not the in-memory layout byte.
    let profile_tag_int = crate::core::value::Tag::Int as u8;
    let profile_tag_float = crate::core::value::Tag::Float as u8;
    let has_self_call = code.iter().any(|i| matches!(i, Inst::SelfCall { .. }));
    let max_argc = code
        .iter()
        .filter_map(|i| match i {
            Inst::SelfCall { argc } => Some(*argc),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    if !has_self_call || max_argc == 0 {
        return vec![];
    }
    (0..max_argc)
        .map(|k| match slot_tags.get(k).copied() {
            Some(t) if t == profile_tag_int => Some((b.declare_var(types::I64), false)),
            Some(t) if t == profile_tag_float => Some((b.declare_var(types::F64), true)),
            _ => None,
        })
        .collect()
}
