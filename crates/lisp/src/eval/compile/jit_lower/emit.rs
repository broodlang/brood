//! CLIF emit helpers for `jit_lower_arm_inner`. jit-only. `Op` is the shared operand
//! model (module scope in `jit_lower.rs`).
#![cfg(feature = "jit")]
use super::*;
use crate::core::heap::VecStore as VS;
use crate::core::value::jit_layout::{
    PAYLOAD_OFFSET, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_KEYWORD, TAG_SYM, TAG_VECTOR,
};
pub(super) use crate::eval::compile::DeoptReason;
use cranelift_codegen::ir::{
    Block, BlockArg, FuncRef, InstBuilder, MemFlagsData, StackSlot, Type, condcodes::IntCC, types,
};
use cranelift_frontend::{FunctionBuilder, Variable};

/// Branch on a `brood_rt_global{,_ic}` status: on to `cont` with the value, to the arm's
/// `deopt` block for a name a module load would bind (the VM's lazy path loads it), or to
/// `error` with the unbound error parked.
pub(super) fn branch_on_global_status(
    b: &mut FunctionBuilder,
    status: cranelift_codegen::ir::Value,
    error: Block,
    deopt: Block,
    cont: Block,
) {
    let miss = b.create_block();
    b.ins().brif(status, miss, &[], cont, &[]);
    b.switch_to_block(miss);
    b.set_cold_block(miss);
    let is_error = b
        .ins()
        .icmp_imm_s(IntCC::Equal, status, crate::jit::rt::status::ERROR);
    let reason = deopt_code(b, DeoptReason::GlobalMissLoads);
    b.ins()
        .brif(is_error, error, &[], deopt, &[BlockArg::Value(reason)]);
}

/// The value a guard passes to its deopt block: `reason`'s code, which the shared deopt
/// path parks on the heap (`brood_rt_note_deopt`). Every guard goes through here, so a
/// reason is always a [`DeoptReason`] — never a bare number another guard might reuse.
pub(super) fn deopt_code(
    b: &mut FunctionBuilder,
    reason: DeoptReason,
) -> cranelift_codegen::ir::Value {
    b.ins().iconst(types::I32, reason.code())
}

/// [`deopt_code`] with a payload known at compile time, above the reason byte.
pub(super) fn deopt_code_with_constant(
    b: &mut FunctionBuilder,
    reason: DeoptReason,
    payload: i64,
) -> cranelift_codegen::ir::Value {
    b.ins().iconst(types::I32, reason.code_with(payload))
}

/// [`deopt_code`] with a payload computed at run time (`payload` is an `i32`), above the
/// reason byte.
pub(super) fn deopt_code_with(
    b: &mut FunctionBuilder,
    reason: DeoptReason,
    payload: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let base = deopt_code(b, reason);
    let shifted = b.ins().ishl_imm_u(payload, 8);
    b.ins().bor(base, shifted)
}

/// The current `roots` data pointer, read straight off the heap: `Heap.roots` is a
/// `RootsBuf` whose header is (ptr, len, cap) at +0/+8/+16 (pinned by a `const` assertion
/// in `roots_buf.rs`), the same load the §7.5 inline call blob makes — one
/// pointer load rather than an FFI round trip at every arm entry and after every call.
pub(super) fn load_roots_base(
    b: &mut FunctionBuilder,
    heap: cranelift_codegen::ir::Value,
) -> cranelift_codegen::ir::Value {
    let off = crate::core::heap::jit_ceremony_offsets().roots as i32;
    b.ins().load(types::I64, MemFlagsData::trusted(), heap, off)
}

/// Read back the three words a callback wrote into `out_slot` (the out-pointer ABI).
pub(super) fn read_out_slot(
    b: &mut FunctionBuilder,
    out_slot: StackSlot,
) -> (
    cranelift_codegen::ir::Value,
    cranelift_codegen::ir::Value,
    cranelift_codegen::ir::Value,
) {
    let w0 = b.ins().stack_load(types::I64, types::I64, out_slot, 0);
    let w1 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32);
    let w2 = b
        .ins()
        .stack_load(types::I64, types::I64, out_slot, PAYLOAD_OFFSET as i32 + 8);
    (w0, w1, w2)
}

/// [`read_out_slot`] as an `Op::Handle`.
pub(super) fn handle_from_out_slot(b: &mut FunctionBuilder, out_slot: StackSlot) -> Op {
    let (w0, w1, w2) = read_out_slot(b, out_slot);
    Op::Handle(w0, w1, w2)
}

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
    /// `brood_rt_vector_base(heap, 3w, out_len) -> *const Value` — a vector's element
    /// storage, decoded once for a run of reads ([`vector_storage`]).
    pub vbase: FuncRef,
    /// `brood_rt_car` / `brood_rt_cdr` — the `first`/`rest` handle ops (FFI fallback).
    pub car: FuncRef,
    /// `brood_rt_first` / `brood_rt_rest` — `first`/`rest` of a NON-pair (the inline
    /// path's tag-check miss): status 0 answers, 1 deopts, 2 parks an error.
    pub first: FuncRef,
    pub rest: FuncRef,
    /// `brood_rt_is_empty` — `empty?` of a non-nil, non-pair (the inline tag check's
    /// miss), same status protocol.
    pub is_empty: FuncRef,
    /// `brood_rt_pair_nursery_base` / `_old_base` — re-fetched after a `rest` fallback
    /// that may have grown the pair slab (see `pair_bases` in `jit_lower_arm_inner`).
    pub pnbase: FuncRef,
    pub pobase: FuncRef,
    pub cdr: FuncRef,
    /// `brood_rt_cons` — pair allocation.
    pub cons: FuncRef,
    /// `brood_rt_vec2_room(heap, out) -> *mut Value` — allocate a 2-element vector and
    /// return its element storage, so the arm writes the elements in place.
    pub vec2room: FuncRef,
    pub makevecn: FuncRef,
    /// `brood_rt_make_map_n(heap, out, elems, npairs)` — a `{k v …}` literal from `2·npairs`
    /// staged `Value`s (`makevecn`'s shape; the map build allocates and never collects).
    pub makemapn: FuncRef,
    /// `brood_rt_make_closure(heap, out, inst) -> status` — a `(fn …)` literal, built by the
    /// VM's own `MakeClosure` arm from captures staged on `roots` ([`super::call::emit_make_closure`]).
    pub mkclo: FuncRef,
    /// `brood_rt_table_has` / `_get2` / `_put` — the table primitives (FFI fallback).
    pub thas: FuncRef,
    pub tget: FuncRef,
    pub tput: FuncRef,
    /// `brood_rt_map_get3` / `brood_rt_map_assoc` — `(get m k default)` and `(assoc m k v)`
    /// on a CHAMP map (ADR-368), `table_put`'s `(heap, out, 3w, 3w, 3w) -> status` shape.
    pub mget3: FuncRef,
    pub massoc: FuncRef,
    /// `brood_rt_vector_ref3` — `(nth v i default)` on a vector, the same shape.
    pub vref3: FuncRef,
    /// `brood_rt_map_get` — the CHAMP probe behind [`PrimOp::MapGet`]. Same
    /// `(heap, out, 3 words, 3 words) -> status` shape as the table reads, and the same
    /// `table_prim` helper drives it: status 0 hands back the value, status 1 deopts to the
    /// VM, which owns every branch of `get` this declines.
    pub mget: FuncRef,
    /// `brood_rt_set_has` — the membership probe behind [`PrimOp::SetHas`], `mget`'s shape:
    /// status 0 answers a bool, 1 (not a set) deopts to the VM's native and its error.
    pub shas: FuncRef,
    /// `brood_rt_tail_native(heap, out, argc, ctx) -> status` — a tail call to a builtin,
    /// called in place so the arm returns its value (ADR-811, `call::emit_call`).
    pub tailnat: FuncRef,
    /// `brood_rt_vector_len` — the length read behind [`PrimOp1::VectorLen`]: `(heap,
    /// vec 3w) -> len`, `-1` for a non-vector (deopt). No out slot; the answer is the return.
    pub vlen: FuncRef,
    /// `brood_rt_count` — [`Heap::count_inline`] behind [`PrimOp1::Count`]: `(heap, 3w) ->
    /// count`, `-1` for a shape the prelude `count` decides (deopt).
    ///
    /// [`Heap::count_inline`]: crate::core::heap::Heap::count_inline
    pub count: FuncRef,
    /// `brood_rt_equal` — `Heap::equal` for the operand pairs [`eq_dispatch`] cannot
    /// decide inline (strings, floats, structural values); status 2 (a seq-view) deopts.
    pub equal: FuncRef,
    /// `brood_rt_global_ic` — resolve a free global through the per-site inline cache.
    pub globic: FuncRef,
    /// `brood_rt_push_room(heap, n) -> *mut Value` — reserve n argument slots on `roots`
    /// and return them, so operands are stored in place rather than copied across.
    pub pushroom: FuncRef,
    /// `brood_rt_in_call_journal(heap, packed) -> *mut Value` — an error path's in-call
    /// journal (ADR-401): room for the operands beneath the call in flight, or null.
    pub incall: FuncRef,
    /// `brood_rt_call_slow` — the general Brood→Brood dispatch (the fast-link miss path).
    pub callslow: FuncRef,
    /// `brood_rt_call_native_fl` — direct builtin call for a native flat-cell hit.
    pub natfl: FuncRef,
    /// `brood_rt_fastlink_base` / `brood_rt_fast_frame` — the in-IR epoch-guarded fast link.
    pub flbase: FuncRef,
    pub fastframe: FuncRef,
    /// `brood_rt_xcall_latch` / `brood_rt_xcall_cold` — the inline fast-frame path's
    /// cold callbacks (§7.5).
    pub xlatch: FuncRef,
    pub xcold: FuncRef,
    /// The [`crate::jit::JitArmFn`] signature `(heap, base, out) -> outcome`, for the
    /// inline path's `call_indirect` straight into a callee's native code.
    pub armfn_sig: cranelift_codegen::ir::SigRef,
    /// This activation's `*const JitCallCtx` (the arm's fourth argument) — passed to every
    /// callback that consults activation state (`docs/call-convention.md`, rung A1).
    pub ctx: cranelift_codegen::ir::Value,
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

/// Batched reduction count for the self-tail loop: the in-register countdown that
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
    /// The arm's Done block: a value stored through `out_ptr`, then a jump here, is the arm's
    /// return (a native tail call to a builtin returns this way, ADR-811).
    pub done: Block,
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
    /// The arm is float-context (`has_float_slot`): a profiled `Float` param, or deopt
    /// feedback that found it computing on floats read out of vectors. Licenses the
    /// optimistic float path for arithmetic on operands nothing else types.
    pub float_context: bool,
    /// The arm's deopt feedback found its arithmetic polymorphic (`jit_arith_dispatch`):
    /// arithmetic that would rest on a type GUESS — a float-guessed slot, a type-erased
    /// operand in a float-context arm — dispatches by tag at runtime instead
    /// (`arith_dispatch`/`cmp_dispatch`), so neither kind of number deopts.
    pub arith_dispatch: bool,
    /// The inline `count` declined often enough that this lowering calls the real `count`
    /// (`CompiledArm::jit_count_calls`, ADR-811).
    pub count_calls: bool,
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
    /// The integer twin of `slot_f64_cache`: the unboxed `i64` last stored to a slot via
    /// `store_op(Op::Int)`, so [`load_slot_int`] can return it instead of reloading the tag,
    /// comparing it to `Int`, branching to `deopt` and reloading the payload — which is what
    /// every read of a `let`-bound int cost (Cranelift does not forward the store). Kept in
    /// lock step with the f64 cache by [`set_slot_flags`] and the widened-join clear, so it
    /// rests on exactly the same soundness argument: a slot is only ever cached by a
    /// `store_op`, a `let` slot is written before it is read in its scope, and parameter
    /// slots (written by `SelfCall`/entry with raw stores) are never cached. Empty (all
    /// `None`) under `BROOD_NO_INT_SLOT_CACHE=1`.
    pub slot_i64_cache: &'a std::cell::RefCell<Vec<Option<cranelift_codegen::ir::Value>>>,
    /// A slot READ as a number, kept for the rest of the straight-line run: the `f64` it
    /// reads as (a float's bits, or an int promoted with `fcvt_from_sint`) and an `i8` that
    /// is 1 when it really was a float. Unlike the two caches above it is filled by a READ —
    /// the first float-path read tag-checks the slot (`number_of`), and every later read in
    /// the run reuses both values with no load and no tag test. The `i8` keeps the VM's mixed
    /// rule exact (KI-114): an op is float arithmetic only when some operand really was a
    /// float, so two cached ints still deopt (`FloatPairBothInt`) rather than compute in
    /// floats. Cleared with the other caches: at every leader, on any store to the slot, and
    /// on a widened join.
    pub slot_num_cache: &'a std::cell::RefCell<Vec<Option<NumberRead>>>,
    /// A vector slot's element storage, `(pointer to element 0, length)`, decoded once by the
    /// first fused `(nth slot k)` of a run that reads the slot again (`vector_reads_reused`)
    /// and reused by the rest: a destructuring `let` reads every element of one vector in a
    /// row, and each read used to re-check the tag, region and storage kind and call for the
    /// slab base. The pointer is into storage the next allocation may move (a small vector's
    /// elements live in its slab slot), so the emit loop clears this before every instruction
    /// `keeps_vector_storage` does not vouch for, as well as at leaders and on a store to
    /// the slot.
    pub slot_vec_cache: &'a std::cell::RefCell<Vec<Option<VectorStorage>>>,
}

/// A slot or operand read as a number on the float path: `value` is the `f64` it reads as
/// (a float's own bits, or an int promoted the VM's way), `is_float` an `i8` that is 1 when
/// the value really was a float. See [`Frame::slot_num_cache`].
#[derive(Clone, Copy)]
pub(super) struct NumberRead {
    pub value: cranelift_codegen::ir::Value,
    pub is_float: cranelift_codegen::ir::Value,
}

/// A vector's element storage, decoded once: the address of element 0 and the element
/// count. See [`Frame::slot_vec_cache`].
#[derive(Clone, Copy)]
pub(super) struct VectorStorage {
    pub elements: cranelift_codegen::ir::Value,
    pub length: cranelift_codegen::ir::Value,
}

/// How one operand-stack entry crosses a block boundary. Block params are `I64`, so the
/// representation has to be agreed by every predecessor (`record_block_flags`); a
/// disagreeing edge is routed to `deopt`.
///
/// `Slot` exists because of KI-49: carrying every operand through `as_int` (tag-check `Int`,
/// deopt otherwise) makes an arm carrying a non-Int value across a merge — any tagged-tuple
/// matcher — deopt on every activation and latch to the interpreter. Carrying "the value is
/// in frame slot k" needs no unboxing and cannot deopt.
///
/// Int is kept as the default for profiled-`Int` slots so the unboxed integer carry —
/// what makes `fib`/`collatz` fast — is untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ParamRepr {
    Int,
    Bool,
    /// A materialised `Op::Float` (a literal, an arithmetic result): its bits cross in the
    /// `i64` param and the target bitcasts them back. Without this a float join read as
    /// `Int` and `as_int` deopted on it every time — `(+ acc (if c 0.5 1.5))` thrashed.
    Float,
    /// The value lives in frame slot `k`; the arg word is a placeholder. Every predecessor
    /// must name the *same* slot, which `control::resolve_edges` settles.
    Slot(usize),
    /// A join entry that has no spill slot to land in (a call-free arm reserves none —
    /// `jit_spill_reserve`) and carries a boxed value: the value crosses as its three
    /// tagged words, the entry's own param plus two extra block params appended for the
    /// `n`th widened entry (`control::resolve_edges`), and the target rebuilds an
    /// `Op::Handle`. Unification assigns `n`; [`param_repr`] asks for it with the
    /// [`WORDS_WANTED`] placeholder for a `Handle` it cannot spill. Letting such a handle fall
    /// through to `Int` deopts whenever every predecessor agrees on it (a `[1]` literal on one
    /// edge, `nil` on the other — the `receive` matcher's inner join): nothing widens and
    /// `as_int` deopts on the vector every message.
    Words(usize),
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
        Op::Float(_) => ParamRepr::Float,
        // A slot the tier-time profile did NOT see an Int in: carry it as a slot reference
        // rather than unboxing it (which would deopt for a vector/map/string/…).
        Op::Slot(k) if !f.slot_int_profile.get(k).copied().unwrap_or(false) => ParamRepr::Slot(k),
        // A boxed value with no slot of its own — the KI-49 case (a `MakeVector` result
        // crossing an `if` merge). Spill it to the block-argument region at its stack index.
        Op::Handle(..) if idx < f.blockarg_spill_len => {
            ParamRepr::Slot(f.blockarg_spill_base + idx)
        }
        // A boxed value with no slot to spill into: cross as words (see `Words`).
        Op::Handle(..) => ParamRepr::Words(WORDS_WANTED),
        _ => ParamRepr::Int,
    }
}

/// The `Words` index [`param_repr`] hands unification for a handle that must cross as
/// words; `resolve_edges` replaces it with the entry's real index, so no edge is ever
/// emitted against it.
pub(super) const WORDS_WANTED: usize = usize::MAX;

/// Does `op` carry a `Value::Float`? (An `Op::Float`, or a `Slot` flagged float.)
pub(super) fn op_is_float(op: Op, f: Frame) -> bool {
    match op {
        Op::Float(_) => true,
        Op::Slot(k) => f.slot_float.borrow().get(k).copied().unwrap_or(false),
        _ => false,
    }
}

/// Could `op` hold a float at runtime? An unboxed `Op::Int`/`Op::Bool` or a hoisted
/// global never can (and `as_f64_pair` deopts on one unconditionally), nor can a slot the
/// profile typed `Int` — its carry is an i64 register. Everything else is a tag-checked read.
pub(super) fn op_maybe_float(op: Op, f: Frame) -> bool {
    match op {
        Op::Float(_) | Op::Handle(..) => true,
        Op::Slot(k) => {
            !matches!(f.carry_vars.get(k).copied().flatten(), Some((_, false)))
                && !f.slot_bool.borrow().get(k).copied().unwrap_or(false)
        }
        _ => false,
    }
}

/// Nothing the lowering knows types slot `k`: not a profiled `Int` (no unboxed carry), not
/// flagged float or bool by a store — a let-bound `nth`/`get` result, typically. In a
/// float-context arm such a slot is read optimistically as a float (tag-guarded).
pub(super) fn slot_untyped(k: usize, f: Frame) -> bool {
    f.carry_vars.get(k).copied().flatten().is_none()
        && !f.slot_int_profile.get(k).copied().unwrap_or(false)
        && !f.slot_float.borrow().get(k).copied().unwrap_or(false)
        && !f.slot_bool.borrow().get(k).copied().unwrap_or(false)
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

/// A condition under which integer arithmetic cannot produce the VM's answer in an `i64`.
/// [`emit_int_arith`]'s caller decides what a guard does: the general lowering deopts, the
/// scalar-register worker sets its overflow sentinel and unwinds.
#[derive(Clone, Copy)]
pub(super) enum IntGuard {
    /// `+`/`-`/`*` overflowed (the VM promotes to a bignum).
    Overflow,
    /// Division by zero (the VM raises).
    DivideByZero,
    /// `i64::MIN / -1`, the one signed-division overflow.
    DivideOverflow,
    /// `%div` with a nonzero remainder (the VM answers an exact `Ratio`).
    DivideInexact,
}

/// The integer arithmetic both lowerings share, on two `i64` SSA operands `(x, y)` (operand
/// order already applied). `guard(b, kind, condition)` is called wherever `condition` true
/// means the `i64` result would be wrong; it must leave `b` switched to the block where the
/// arithmetic continues. Returns `None` for an op this table does not lower.
///
/// Cranelift's `sdiv`/`srem` *trap* on a zero divisor and on `i64::MIN / -1`, so both are
/// guarded before the op; `%div` is an `Int` only on an exact quotient.
pub(super) fn emit_int_arith(
    b: &mut FunctionBuilder,
    op: PrimOp,
    x: cranelift_codegen::ir::Value,
    y: cranelift_codegen::ir::Value,
    guard: &mut dyn FnMut(&mut FunctionBuilder, IntGuard, cranelift_codegen::ir::Value),
) -> Option<cranelift_codegen::ir::Value> {
    Some(match op {
        PrimOp::Add => {
            let (r, ov) = b.ins().sadd_overflow(x, y);
            guard(b, IntGuard::Overflow, ov);
            r
        }
        PrimOp::Sub => {
            let (r, ov) = b.ins().ssub_overflow(x, y);
            guard(b, IntGuard::Overflow, ov);
            r
        }
        PrimOp::Mul => {
            let (r, ov) = b.ins().smul_overflow(x, y);
            guard(b, IntGuard::Overflow, ov);
            r
        }
        PrimOp::Lt => b.ins().icmp(IntCC::SignedLessThan, x, y),
        PrimOp::Le => b.ins().icmp(IntCC::SignedLessThanOrEqual, x, y),
        PrimOp::Eq => b.ins().icmp(IntCC::Equal, x, y),
        PrimOp::Rem | PrimOp::Quot | PrimOp::Div => {
            let zero = b.ins().iconst(types::I64, 0);
            let div0 = b.ins().icmp(IntCC::Equal, y, zero);
            guard(b, IntGuard::DivideByZero, div0);
            let min = b.ins().iconst(types::I64, i64::MIN);
            let neg1 = b.ins().iconst(types::I64, -1);
            let x_min = b.ins().icmp(IntCC::Equal, x, min);
            let y_m1 = b.ins().icmp(IntCC::Equal, y, neg1);
            let ov = b.ins().band(x_min, y_m1);
            guard(b, IntGuard::DivideOverflow, ov);
            match op {
                PrimOp::Rem => b.ins().srem(x, y),
                PrimOp::Quot => b.ins().sdiv(x, y),
                _ => {
                    let r = b.ins().srem(x, y);
                    let inexact = b.ins().icmp(IntCC::NotEqual, r, zero);
                    guard(b, IntGuard::DivideInexact, inexact);
                    b.ins().sdiv(x, y)
                }
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
        PrimOp::Cons
        | PrimOp::VectorRef
        | PrimOp::TableHas
        | PrimOp::TableGet
        | PrimOp::MapGet
        | PrimOp::SetHas
        | PrimOp::ContractAwait => return None,
    })
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
    match op {
        PrimOp::Cons => return super::bail("prim-cons-allocates"), // never in the JIT subset
        PrimOp::VectorRef => return super::bail("prim-vector-ref-slab-read"), // out of subset
        // Table ops and the CHAMP probe are runtime callbacks lowered in `prim.rs`.
        PrimOp::TableHas | PrimOp::TableGet => return super::bail("prim-table-op-not-int-arith"),
        PrimOp::MapGet => return super::bail("prim-map-get-not-int-arith"),
        PrimOp::SetHas => return super::bail("prim-set-has-not-int-arith"),
        // A contract shim's mark — out of the subset (`chunk_in_jit_subset`), never here.
        PrimOp::ContractAwait => return super::bail("prim-contract-await-vm-only"),
        _ => {}
    }
    let mut guard = |b: &mut FunctionBuilder, kind: IntGuard, condition| {
        let reason = match kind {
            IntGuard::Overflow => DeoptReason::IntArithmeticOverflow,
            IntGuard::DivideByZero => DeoptReason::IntDivideByZero,
            IntGuard::DivideOverflow => DeoptReason::IntDivideOverflow,
            IntGuard::DivideInexact => DeoptReason::IntDivideInexact,
        };
        let cont = b.create_block();
        let deopt_reason = deopt_code(b, reason);
        b.ins().brif(
            condition,
            deopt,
            &[BlockArg::Value(deopt_reason)],
            cont,
            &[],
        );
        b.switch_to_block(cont);
    };
    emit_int_arith(b, op, x, y, &mut guard)
}

/// Float arithmetic/comparison lowering (f64). Returns an `Op::Float`/`Op::Int`,
/// or None if `op` is not lowered for floats (e.g. structural `=`). Exhaustive on purpose: a
/// new `PrimOp` must decide its float lowering here, not fall into a catch-all that bails the
/// whole arm under another op's reason (float `max`/`min` did, until ADR-808).
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
            let deopt_reason = deopt_code(b, DeoptReason::FloatDivideByZero);
            b.ins()
                .brif(is_zero, deopt, &[BlockArg::Value(deopt_reason)], cont, &[]);
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
        PrimOp::Eq => return super::bail("float-eq-mixed-int-float-unsafe"),
        // The answer is an OPERAND, not an f64 (an int that wins stays an int), so these take
        // `min_max`, which every caller routes them to first. Reaching here is a routing gap.
        PrimOp::Max | PrimOp::Min => return super::bail("float-min-max-not-routed"),
        // Int-domain operations: a float operand is the VM's type error (or its numeric tower).
        PrimOp::Rem | PrimOp::Quot | PrimOp::BitAnd | PrimOp::BitOr | PrimOp::BitXor => {
            return super::bail("float-operand-to-int-op");
        }
        // Not arithmetic: their callers lower them before the float path is considered.
        PrimOp::Cons
        | PrimOp::VectorRef
        | PrimOp::TableHas
        | PrimOp::TableGet
        | PrimOp::MapGet
        | PrimOp::SetHas
        | PrimOp::ContractAwait => return super::bail("float-path-not-arithmetic"),
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
/// operand branches to `deopt`). A register-carried param slot skips the check, and so
/// does a slot this lowering just stored an `i64` to (`slot_i64_cache`).
pub(super) fn load_slot_int(
    b: &mut FunctionBuilder,
    k: i64,
    f: Frame,
) -> cranelift_codegen::ir::Value {
    if let Some((var, false)) = f.carry_vars.get(k as usize).copied().flatten() {
        return b.use_var(var);
    }
    if let Some(v) = f.slot_i64_cache.borrow().get(k as usize).copied().flatten() {
        return v;
    }
    let roots_base = b.use_var(f.rb_var);
    let idx = b.ins().iadd_imm_s(f.base, k);
    let off = b.ins().imul_imm_s(idx, STRIDE);
    let addr = b.ins().iadd(roots_base, off);
    let tag = b.ins().load(types::I8, MemFlagsData::trusted(), addr, 0);
    let is_int = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_INT as i64);
    let cont = b.create_block();
    // The observed tag rides as the payload (as `as_int`'s does, KI-49): a deopt on a
    // `Float` here is the arm's float-context feedback (`float_deopt_feedback`).
    let tag32 = b.ins().uextend(types::I32, tag);
    let deopt_reason = deopt_code_with(b, DeoptReason::SlotIntGuard, tag32);
    b.ins()
        .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(deopt_reason)]);
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
            // bug we're hunting (#2). Validation lives on the Rust side, at the callee's
            // frame entry (`dispatch.rs`).
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
            // bug (see Op::Slot above); the callee's frame entry checks what arrives.
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
/// boxes those as `Value::Bool`) must not pass through raw, for two reasons:
///
///   * **engine divergence.** A `Const` boolean materialises as `Op::Bool(iconst 0/1)`
///     (`jit_lower.rs`), so a raw `(+ acc true)` would compute `acc + 1` where the tree-walker
///     and the VM both raise `+: expected number, got bool`. Guarded by
///     `tests/jit_bool_arith_test.blsp`.
///   * **`i8` into `i64` arithmetic.** `(+ (< x 0) 1)` would reach `emit_arith` as
///     `sadd_overflow(i8, i64)`, which Cranelift refuses — so a cold nonsense branch would
///     silently cost the *hot* arm its native code.
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
        let deopt_reason = deopt_code(b, DeoptReason::BoolWhereNumberRequired);
        b.ins().jump(f.deopt, &[BlockArg::Value(deopt_reason)]);
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
            // KI-49: carry the OBSERVED tag as the payload, so the deopt says not just "a
            // Handle wasn't an Int here" but which type actually arrived.
            let tag32 = b.ins().ireduce(types::I32, tagb);
            let deopt_reason = deopt_code_with(b, DeoptReason::HandleIntGuard, tag32);
            b.ins()
                .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(deopt_reason)]);
            b.switch_to_block(cont);
            w1
        }
        // A hoisted global vector/table used as an int (neither is one) — tag-check
        // its word like a `Handle` and deopt; sound, never expected to fire.
        Op::HoistedVec { w0, w1, .. } | Op::HoistedTable { w0, w1, .. } => {
            let tagb = b.ins().band_imm_s(w0, 0xff);
            let is_int = b.ins().icmp_imm_s(IntCC::Equal, tagb, TAG_INT as i64);
            let cont = b.create_block();
            let deopt_reason = deopt_code(b, DeoptReason::HoistedValueNotInt);
            b.ins()
                .brif(is_int, cont, &[], f.deopt, &[BlockArg::Value(deopt_reason)]);
            b.switch_to_block(cont);
            w1
        }
        // A float where an int is required (a mixed-type op the lowering didn't
        // specialize) — deopt to the VM. Shouldn't arise once arith dispatches by
        // operand type, but kept sound. (Dead block after the unconditional jump.)
        Op::Float(_) => {
            let deopt_reason = deopt_code(b, DeoptReason::FloatWhereIntRequired);
            b.ins().jump(f.deopt, &[BlockArg::Value(deopt_reason)]);
            let dead = b.create_block();
            b.switch_to_block(dead);
            b.ins().iconst(types::I64, 0)
        }
    }
}

/// The word an operand crosses a block edge as, for the repr [`param_repr`] assigned it
/// when the edge was emitted, which is when
/// the single-pass slot flags described the value crossing it. `control::resolve_edges`
/// fills an edge block later, after other blocks' stores have moved those flags, so it
/// must not re-derive the repr here: a slot flagged bool by a store on another path
/// would be read as its payload byte without a tag check.
pub(super) fn as_block_arg_for(
    b: &mut FunctionBuilder,
    op: Op,
    repr: ParamRepr,
    f: Frame,
) -> cranelift_codegen::ir::Value {
    match repr {
        // KI-49: an operand crossing as `ParamRepr::Slot` is NOT materialised — the target
        // rebuilds `Op::Slot(k)` and reads the frame when it needs the value. Forcing it
        // through `as_int` here is what deopted every tagged-tuple matcher.
        ParamRepr::Slot(k) => {
            // A Handle has to be materialised INTO the slot first; a Slot operand already
            // lives in one (and `k` is that same slot, so this would be a self-copy).
            if matches!(op, Op::Handle(..)) {
                store_op(b, k as i64, op, f);
            }
            b.ins().iconst(types::I64, 0)
        }
        // A bool crossing the edge is legitimate (an `(and …)`/`(or …)` short-circuits its
        // bound operand through the merge) and must NOT go through `as_int`, which deopts
        // on a boolean operand: the target reconstructs `Op::Bool` from the `bool_param`
        // flag, so what crosses is the 0/1 word, not a number.
        ParamRepr::Bool => match op {
            Op::Bool(v) => v, // already an i64 0/1
            Op::Int(v) if b.func.dfg.value_type(v) == types::I8 => b.ins().uextend(types::I64, v),
            // A slot proven to hold a `Value::Bool` (`slot_bool`): load its payload byte
            // (0/1) as the i64 arg. `as_int` would instead tag-check `Int` and deopt on
            // the `Bool`.
            Op::Slot(k) => {
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
                b.ins().band_imm_s(pl, 0xff)
            }
            _ => {
                let v = as_int(b, op, f);
                if b.func.dfg.value_type(v) == types::I8 {
                    b.ins().uextend(types::I64, v)
                } else {
                    v
                }
            }
        },
        ParamRepr::Int => {
            let v = as_int(b, op, f);
            if b.func.dfg.value_type(v) == types::I8 {
                b.ins().uextend(types::I64, v)
            } else {
                v
            }
        }
        ParamRepr::Float => match op {
            Op::Float(v) => b.ins().bitcast(types::I64, MemFlagsData::new(), v),
            _ => unreachable!("only a materialised float is typed Float"),
        },
        // The first of the three words; `resolve_edges` passes the other two itself.
        ParamRepr::Words(_) => read_words(b, op, f)[0],
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
    reason: DeoptReason,
    promote: bool,
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
    let deopt_reason = deopt_code(b, reason);
    // `promote` decides what an INT operand means here, and it is the whole of KI-114.
    // Promoting is the VM's semantics for a MIXED operation — `(+ 1.5 2)` really is 3.5 —
    // but only when some operand is genuinely a float. When the float path was entered on
    // a GUESS (`op_is_float` reads `slot_float`, a single-pass approximation) and every
    // operand turns out to be an int, the VM computes in ints and answers an int; a native
    // that promotes answers a float instead. That is a miscompile, not a slow path.
    let int_target = if promote { as_int } else { f.deopt };
    let int_args: &[BlockArg] = if promote {
        &[]
    } else {
        &[BlockArg::Value(deopt_reason)]
    };
    b.ins().brif(
        is_i,
        int_target,
        int_args,
        f.deopt,
        &[BlockArg::Value(deopt_reason)],
    );
    b.switch_to_block(as_int);
    let promoted = b.ins().fcvt_from_sint(types::F64, payload);
    b.ins().jump(merge, &[BlockArg::Value(promoted)]);
    b.switch_to_block(merge);
    b.block_params(merge)[0]
}

pub(super) fn as_f64(b: &mut FunctionBuilder, op: Op, f: Frame) -> cranelift_codegen::ir::Value {
    as_f64_guarded(b, op, f, true)
}

/// One operand of a two-operand float lowering, classified for [`as_f64_pair`].
enum FOperand {
    /// A float with no check needed: an `Op::Float`, or a slot already carried/cached as f64.
    Float(cranelift_codegen::ir::Value),
    /// A number already read this run: a slot in [`Frame::slot_num_cache`].
    Number(NumberRead),
    /// A tagged value whose type is only known at runtime. `slot` names the frame slot it
    /// was loaded from, so its number read can be kept for the rest of the run.
    Tagged {
        tag: cranelift_codegen::ir::Value,
        payload: cranelift_codegen::ir::Value,
        slot: Option<usize>,
    },
    /// Cannot be a number at all: a comparison's answer (an `Op::Int` of type `I8`), a
    /// bool, a hoisted vector/table. (An unboxed `I64` int is a `Number` that is not a
    /// float.)
    Never,
}

/// Classify one operand, mirroring `as_f64_guarded`'s own fast paths so a slot it would
/// answer without a tag check is `Float` here too.
fn classify_float_operand(b: &mut FunctionBuilder, op: Op, f: Frame) -> FOperand {
    match op {
        Op::Float(v) => FOperand::Float(v),
        Op::Slot(k) => {
            if let Some((var, true)) = f.carry_vars.get(k).copied().flatten() {
                return FOperand::Float(b.use_var(var));
            }
            if let Some(v) = f.slot_f64_cache.borrow().get(k).copied().flatten() {
                return FOperand::Float(v);
            }
            if let Some(number) = f.slot_num_cache.borrow().get(k).copied().flatten() {
                return FOperand::Number(number);
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
            FOperand::Tagged {
                tag,
                payload,
                slot: Some(k),
            }
        }
        Op::Handle(w0, w1, _) => FOperand::Tagged {
            tag: b.ins().band_imm_s(w0, 0xff),
            payload: w1,
            slot: None,
        },
        // An unboxed int is a number that is known not to be a float: beside a float it
        // promotes, the VM's own mixed rule, exactly as a tagged int read from a slot does
        // ([`number_of`]); beside another int the pair deopts with `FloatPairBothInt`, and
        // the VM answers an int. Classed `Never` it deopted on EVERY run, so the prelude's
        // one-argument `-` (`(%sub 0 x)`) and `/` (`(%div 1 x)`) latched any float function
        // that negated or inverted a value off the native tier: a quaternion product written
        // with `(- (* a b))` ran ten times slower than the same sum written `(- 0.0 (* a b))`.
        // (An `Op::Int` of type `I8` is a comparison's answer, not a number: still `Never`.)
        Op::Int(v) if b.func.dfg.value_type(v) == types::I64 => FOperand::Number(NumberRead {
            value: b.ins().fcvt_from_sint(types::F64, v),
            is_float: b.ins().iconst(types::I8, 0),
        }),
        Op::Int(_) | Op::Bool(_) | Op::HoistedVec { .. } | Op::HoistedTable { .. } => {
            FOperand::Never
        }
    }
}

/// Read a tagged value as a number: a float's bits, or an int promoted with
/// `fcvt_from_sint` (the VM's `i64 as f64`), with an `i8` saying which; anything else
/// deopts with `reason`. One branch, then two selects' worth of straight-line code — so
/// the answer can be kept ([`Frame::slot_num_cache`]) and reused by every later read of
/// the slot in the run with no load and no tag test.
///
/// Promoting here is not yet a decision to compute in floats: [`as_f64_pair`] makes that
/// from both operands' `is_float`, the VM's own mixed rule (KI-109, KI-114).
fn number_of(
    b: &mut FunctionBuilder,
    tag: cranelift_codegen::ir::Value,
    payload: cranelift_codegen::ir::Value,
    f: Frame,
    reason: DeoptReason,
) -> NumberRead {
    // A branch, not a select: float code reads floats, so `is_float` predicts almost
    // perfectly, and the float path is one bitcast. Computing both the bitcast and the int's
    // `fcvt_from_sint` and selecting put the conversion's latency and a data dependency on
    // every read — a point transform ran 18% slower that way than on the branch it replaced.
    let is_float = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_FLOAT as i64);
    let as_float = b.create_block();
    let not_float = b.create_block();
    let as_int = b.create_block();
    let merge = b.create_block();
    b.append_block_param(merge, types::F64);
    b.ins().brif(is_float, as_float, &[], not_float, &[]);
    b.switch_to_block(as_float);
    let bits = b.ins().bitcast(types::F64, MemFlagsData::new(), payload);
    b.ins().jump(merge, &[BlockArg::Value(bits)]);
    b.switch_to_block(not_float);
    let is_int = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_INT as i64);
    let deopt_reason = deopt_code(b, reason);
    b.ins().brif(
        is_int,
        as_int,
        &[],
        f.deopt,
        &[BlockArg::Value(deopt_reason)],
    );
    b.switch_to_block(as_int);
    let promoted = b.ins().fcvt_from_sint(types::F64, payload);
    b.ins().jump(merge, &[BlockArg::Value(promoted)]);
    b.switch_to_block(merge);
    let value = b.block_params(merge)[0];
    NumberRead { value, is_float }
}

/// A classified operand as a number read, tag-checking a `Tagged` one (and keeping the
/// read of a slot for the rest of the run). `Float` and `Never` are the caller's.
fn operand_number(
    b: &mut FunctionBuilder,
    operand: FOperand,
    f: Frame,
    reason: DeoptReason,
) -> Option<NumberRead> {
    match operand {
        FOperand::Number(number) => Some(number),
        FOperand::Tagged { tag, payload, slot } => {
            let number = number_of(b, tag, payload, f, reason);
            if let Some(k) = slot
                && let Some(entry) = f.slot_num_cache.borrow_mut().get_mut(k)
            {
                *entry = Some(number);
            }
            Some(number)
        }
        FOperand::Float(_) | FOperand::Never => None,
    }
}

/// Read BOTH operands of a float arithmetic/compare as f64 — the sound alternative to two
/// separate [`as_f64`] calls.
///
/// The float path is chosen by `op_is_float`, which reads `slot_float` — a single-pass
/// approximation whose stated contract is that *a wrong guess is safe: a deopt, not a
/// miscompile*. KI-109 broke that contract while fixing a real stall: it made an `Int`
/// operand promote via `fcvt_from_sint` rather than deopt, which is right for MIXED
/// arithmetic (`(+ 1.5 1)` really is 2.5) but was applied unconditionally, so an arm whose
/// guess was simply wrong computed two ints in floats and published a float where the VM
/// publishes an int (KI-114).
///
/// The rule that makes both cases right is the VM's own: an op is float arithmetic **iff at
/// least one operand is a float**, and only then is the other coerced. So promotion is
/// licensed per-operand by the *other* operand having been proven float. Reading the pair
/// together is what makes that expressible — and it is why this is one function rather than
/// two calls: neither operand can decide alone.
///
/// Each operand not already a float is read as a number ([`number_of`]: float bits or a
/// promoted int, and whether it was a float); beside a certain float that is the answer,
/// and with neither certain, one test that at least one really was a float licenses both
/// (two ints deopt with `FloatPairBothInt`, and the VM answers an int). A slot's number
/// read is kept for the run, so a value used in several products — every element of a
/// destructured matrix — is loaded and tag-checked once, not once per use.
pub(super) fn as_f64_pair(
    b: &mut FunctionBuilder,
    oa: Op,
    ob: Op,
    f: Frame,
) -> (cranelift_codegen::ir::Value, cranelift_codegen::ir::Value) {
    let ca = classify_float_operand(b, oa, f);
    let cb = classify_float_operand(b, ob, f);
    match (ca, cb) {
        // An operand that can never be a float deopts, exactly as `as_f64` always did.
        (FOperand::Never, _) | (_, FOperand::Never) => {
            let deopt_reason = deopt_code(b, DeoptReason::FloatPairNeverFloat);
            b.ins().jump(f.deopt, &[BlockArg::Value(deopt_reason)]);
            let dead = b.create_block();
            b.switch_to_block(dead);
            let zero = b.ins().f64const(0.0);
            (zero, zero)
        }
        (FOperand::Float(x), FOperand::Float(y)) => (x, y),
        // One side is certainly a float, so the other may promote (KI-109).
        (FOperand::Float(x), right) => {
            let reason = DeoptReason::FloatPairRightNotNumber;
            let number = operand_number(b, right, f, reason);
            (x, number.map_or(x, |read| read.value))
        }
        (left, FOperand::Float(y)) => {
            let reason = DeoptReason::FloatPairLeftNotNumber;
            let number = operand_number(b, left, f, reason);
            (number.map_or(y, |read| read.value), y)
        }
        // Neither is certainly a float: whichever proves to be one licenses the other's
        // promotion, and two ints deopt — the VM computes in ints and answers an int.
        (left, right) => {
            let first = operand_number(b, left, f, DeoptReason::FloatPairLeftNotNumber);
            let second = operand_number(b, right, f, DeoptReason::FloatPairRightNotNumber);
            let (Some(first), Some(second)) = (first, second) else {
                unreachable!("both operands are numbers or tagged here")
            };
            let some_float = b.ins().bor(first.is_float, second.is_float);
            let float_arithmetic = b.create_block();
            let deopt_reason = deopt_code(b, DeoptReason::FloatPairBothInt);
            b.ins().brif(
                some_float,
                float_arithmetic,
                &[],
                f.deopt,
                &[BlockArg::Value(deopt_reason)],
            );
            b.switch_to_block(float_arithmetic);
            (first.value, second.value)
        }
    }
}

/// [`as_f64`], but the caller says whether an `Int` operand may be **promoted** to f64 or
/// must **deopt** (KI-114).
///
/// Promote only when the float path was entered because some operand is CERTAINLY a float
/// (an `Op::Float`, so the arm really is doing mixed arithmetic and f64 is the VM's own
/// answer — KI-109). When it was entered on `op_is_float`'s `slot_float` GUESS, pass
/// `false`: if every operand then turns out to be an int, the VM computes in ints and
/// answers an int, so promoting would publish `-33.0` where the VM says `-33`.
pub(super) fn as_f64_guarded(
    b: &mut FunctionBuilder,
    op: Op,
    f: Frame,
    promote: bool,
) -> cranelift_codegen::ir::Value {
    match op {
        Op::Float(v) => v,
        Op::Slot(k) => {
            if let Some((var, true)) = f.carry_vars.get(k).copied().flatten() {
                return b.use_var(var);
            }
            if let Some(v) = f.slot_f64_cache.borrow().get(k).copied().flatten() {
                return v;
            }
            // Read as a number earlier in the run: no load. An int may stand only where
            // promotion is licensed; otherwise it deopts, as the tag check below would.
            if let Some(number) = f.slot_num_cache.borrow().get(k).copied().flatten() {
                if promote {
                    return number.value;
                }
                let float = b.create_block();
                let deopt_reason = deopt_code(b, DeoptReason::SlotNumberNotFloat);
                b.ins().brif(
                    number.is_float,
                    float,
                    &[],
                    f.deopt,
                    &[BlockArg::Value(deopt_reason)],
                );
                b.switch_to_block(float);
                return number.value;
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
            float_or_promoted_int(b, tag, payload, f, DeoptReason::SlotFloatGuard, promote)
        }
        Op::Handle(w0, w1, _) => {
            // A type-erased boxed `Value` (a `nth`/vector read, a call result) used as a
            // float: tag-check `Float` and extract its payload bits, else deopt (the VM
            // then runs the arm with the real type). Mirrors the `Op::Slot` path but on
            // words already in registers. This is what lets `(nth v k)`-fed float
            // arithmetic stay native instead of deopting on the int-path `as_int`.
            let tagb = b.ins().band_imm_s(w0, 0xff);
            float_or_promoted_int(b, tagb, w1, f, DeoptReason::HandleFloatGuard, promote)
        }
        Op::Int(_) | Op::Bool(_) | Op::HoistedVec { .. } | Op::HoistedTable { .. } => {
            let deopt_reason = deopt_code(b, DeoptReason::NeverFloatOperand);
            b.ins().jump(f.deopt, &[BlockArg::Value(deopt_reason)]);
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
    let (is_float, is_bool, f64v, i64v) = match op {
        // A comparison `i8` (`store_int`/`box_scalar` boxes it as `Value::Bool`) marks the
        // slot bool; a real `i64` int does not — and only a real `i64` is cached as one.
        Op::Int(v) => {
            let is_i8 = b.func.dfg.value_type(v) == types::I8;
            (
                false,
                is_i8,
                None,
                // `BROOD_NO_INT_SLOT_CACHE=1` disables the cache (the A/B and bisect lever).
                (!is_i8 && crate::diagnostics::debug_flags::int_slot_cache()).then_some(v),
            )
        }
        Op::Float(v) => (true, false, Some(v), None),
        Op::Bool(_) => (false, true, None, None),
        Op::Slot(k) => (
            f.slot_float.borrow().get(k).copied().unwrap_or(false),
            f.slot_bool.borrow().get(k).copied().unwrap_or(false),
            f.slot_f64_cache.borrow().get(k).copied().flatten(),
            f.slot_i64_cache.borrow().get(k).copied().flatten(),
        ),
        Op::Handle(..) | Op::HoistedVec { .. } | Op::HoistedTable { .. } => {
            (false, false, None, None)
        }
    };
    // A slot copy carries the source's number read with it; any other store ends it, and
    // ends a decoded vector storage for the slot (the slot now holds another value).
    let number = match op {
        Op::Slot(k) => f.slot_num_cache.borrow().get(k).copied().flatten(),
        _ => None,
    };
    if let Some(s) = f.slot_num_cache.borrow_mut().get_mut(dst as usize) {
        *s = number;
    }
    let storage = match op {
        Op::Slot(k) => f.slot_vec_cache.borrow().get(k).copied().flatten(),
        _ => None,
    };
    if let Some(s) = f.slot_vec_cache.borrow_mut().get_mut(dst as usize) {
        *s = storage;
    }
    set_slot_float(dst, is_float, f);
    set_slot_bool(dst, is_bool, f);
    if let Some(s) = f.slot_f64_cache.borrow_mut().get_mut(dst as usize) {
        *s = f64v;
    }
    if let Some(s) = f.slot_i64_cache.borrow_mut().get_mut(dst as usize) {
        *s = i64v;
    }
}

/// The arm's **return** store: write the single result `Value` to `addr` as one 16-byte
/// vector store of `[tag, payload]` plus an 8-byte store of the third word.
///
/// `addr` is the caller-supplied `out` pointer, not a frame slot: returning through
/// `roots[base]` and having `jit_run_fast_link` load it straight back is a costly memory
/// round trip (docs/compute-frontier.md §2h). That cost is the round trip itself, not a
/// store-forwarding width mismatch — widening the store alone did not help. The wide store
/// is kept because it matches the consumer's width for free.
pub(super) fn store_result(
    b: &mut FunctionBuilder,
    op: Op,
    addr: cranelift_codegen::ir::Value,
    f: Frame,
) {
    let [w0, w1, w2] = read_words(b, op, f);
    // An `I64X2` vector store, NOT `iconcat` + `store.i128`: Cranelift's x64 backend keeps
    // an `i128` in a GPR *pair*, so that form legalizes back into two 8-byte `mov`s and the
    // load still straddles them. A vector store is one `movups`.
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
    handle_from_out_slot(b, fu.out_slot)
}

/// The address of RUNTIME vector `w1`'s `VecStore`, computed inline, or `None` when this
/// binary's `boxcar` does not match the layout (`rt_vec_entry_stride`), and the caller keeps
/// the callback. Emitted into the current block; every guard that fails branches to
/// `fallback`, and the block left current holds the address:
///
/// 1. the handle's region is RUNTIME (PRELUDE, the other shared region, takes `fallback`);
/// 2. the heap's cached version for the handle's generation equals the runtime's live
///    `gen_version`, so `gen_slabs_addr[g]` is the generation's current `CodeSlabs`. A
///    replaced generation (Stage-4 free, compaction) bumps the version, so native code never
///    indexes a retired one; the callback's `code_gen_ref` refreshes both;
/// 3. the bucket's base is known (`CodeSlabs::vec_buckets`, noted by the callback).
///
/// A RUNTIME object never moves within its generation (`boxcar` is append-only) and its
/// `VecStore` is the LOCAL one, so the caller decodes it exactly as a LOCAL slot.
pub(super) fn shared_vec_slot(
    b: &mut FunctionBuilder,
    w1: cranelift_codegen::ir::Value,
    fu: Funcs,
    fallback: Block,
) -> Option<cranelift_codegen::ir::Value> {
    use crate::core::heap::rt_vec_offsets as off;
    let stride = crate::core::heap::rt_vec_entry_stride()? as i64;
    let flags = MemFlagsData::trusted();
    let region = b.ins().ushr_imm_s(w1, 62);
    let is_runtime = b
        .ins()
        .icmp_imm_s(IntCC::Equal, region, crate::core::value::RUNTIME as i64);
    let gen_ok = b.create_block();
    b.ins().brif(is_runtime, gen_ok, &[], fallback, &[]);
    b.switch_to_block(gen_ok);
    // The generation bit (32), as a byte offset into the two-entry heap arrays.
    let g = b.ins().ushr_imm_s(w1, 32);
    let g = b.ins().band_imm_s(g, 1);
    let g8 = b.ins().ishl_imm_s(g, 3);
    let heap_g = b.ins().iadd(fu.heap, g8);
    let cached = b.ins().load(types::I64, flags, heap_g, off::GEN_CACHE_VER);
    let live_at = b
        .ins()
        .load(types::I64, flags, fu.heap, off::RT_GEN_VERSION);
    let live = b.ins().load(types::I64, flags, live_at, 0);
    let current = b.ins().icmp(IntCC::Equal, cached, live);
    let slabs_ok = b.create_block();
    b.ins().brif(current, slabs_ok, &[], fallback, &[]);
    b.switch_to_block(slabs_ok);
    let slabs = b.ins().load(types::I64, flags, heap_g, off::GEN_SLABS_ADDR);
    // `rt_vec_location`: bucket ⌊log2(i + 32)⌋ − 5 at offset i + 32 − 2^⌊log2(i + 32)⌋.
    let index = b.ins().band_imm_s(w1, 0xFFFF_FFFFi64);
    let inner = b.ins().iadd_imm_s(index, 32);
    let lz = b.ins().clz(inner);
    let sixty_three = b.ins().iconst(types::I64, 63);
    let log = b.ins().isub(sixty_three, lz);
    let one = b.ins().iconst(types::I64, 1);
    let bucket_start = b.ins().ishl(one, log);
    let within = b.ins().isub(inner, bucket_start);
    let bucket8 = b.ins().ishl_imm_s(log, 3);
    let cell = b.ins().iadd(slabs, bucket8);
    // `log - 5` buckets in: the table index scaled by 8 is `log*8 - 40`.
    let base = b.ins().load(types::I64, flags, cell, off::VEC_BUCKETS - 40);
    let known = b.ins().icmp_imm_s(IntCC::NotEqual, base, 0);
    let slot_ok = b.create_block();
    b.ins().brif(known, slot_ok, &[], fallback, &[]);
    b.switch_to_block(slot_ok);
    let within_bytes = b.ins().imul_imm_s(within, stride);
    Some(b.ins().iadd(base, within_bytes))
}

/// Dynamic-index vector read, fully inline for a LOCAL or RUNTIME vector (either storage):
/// tag/int-index/region checks → slab slot (a RUNTIME one through `shared_vec_slot`, e.g.
/// matmul's `def`'d rows) → inline or spill element read — no FFI on the hot path (this
/// was ~20 ns/element on the json/regex code-vector scans, and 31% of matmul's cycles for
/// the RUNTIME rows). Anything else — non-vector, PRELUDE, a RUNTIME generation or bucket
/// not yet placed, non-int index, out-of-range — falls back to the
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
    // index must be an Int.
    let itag = b.ins().band_imm_s(idx[0], 0xff);
    let is_int = b.ins().icmp_imm_s(IntCC::Equal, itag, TAG_INT as i64);
    let c3 = b.create_block();
    b.ins().brif(is_int, c3, &[], ffi_blk, &[]);
    b.switch_to_block(c3);
    let idxv = idx[1];
    // Both regions' slot pointers converge on the one storage decode below.
    let decode = b.create_block();
    b.append_block_param(decode, fu.ptr_ty);
    // region: high 2 bits of the handle == 0 (LOCAL); RUNTIME → `shared_vec_slot`;
    // PRELUDE → FFI.
    let high2 = b.ins().ushr_imm_s(vec[1], 62);
    let is_local = b.ins().icmp_imm_s(IntCC::Equal, high2, 0);
    let c2 = b.create_block();
    let shared = b.create_block();
    b.ins().brif(is_local, c2, &[], shared, &[]);
    b.switch_to_block(shared);
    match shared_vec_slot(b, vec[1], fu, ffi_blk) {
        Some(slot) => {
            b.ins().jump(decode, &[BlockArg::Value(slot)]);
        }
        None => {
            b.ins().jump(ffi_blk, &[]);
        }
    }
    b.switch_to_block(c2);
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
    let local_slot = b.ins().iadd(sbase, soff);
    b.ins().jump(decode, &[BlockArg::Value(local_slot)]);
    b.switch_to_block(decode);
    let slotp = b.block_params(decode)[0];
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
    // Spill or Shared storage (the same `ptr`/`len` offsets, ADR-511): bounds vs the cached
    // len, elements via the cached ptr.
    b.switch_to_block(not_inl);
    let pointer_read =
        b.ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, disc, VS::JIT_SPILL_TAG);
    let spl = b.create_block();
    b.ins().brif(pointer_read, spl, &[], ffi_blk, &[]);
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
    let deopt_reason = deopt_code(b, DeoptReason::VectorRefDeclined);
    b.ins()
        .brif(status, f.deopt, &[BlockArg::Value(deopt_reason)], cont, &[]);
    b.switch_to_block(cont);
    let (w0, w1, w2) = read_out_slot(b, fu.out_slot);
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
    let is_err = b
        .ins()
        .icmp_imm_s(IntCC::Equal, status, crate::jit::rt::status::ERROR);
    let deopt_reason = deopt_code(b, DeoptReason::TableReadDeclined);
    b.ins().brif(
        is_err,
        fu.error,
        &[],
        f.deopt,
        &[BlockArg::Value(deopt_reason)],
    );
    b.switch_to_block(cont);
    handle_from_out_slot(b, fu.out_slot)
}

/// Runtime-dispatched `=` on materialised operands — the codegen twin of the VM's
/// inline `=` in `prim2_inline_exec` (Int × Int, then `Heap::equal` for anything but a
/// seq-view). Cases, by runtime tags:
///   * Int × Int → payload compare (the same two tag-checks the old int path paid);
///   * either side Sym/Keyword → interned identity: equal iff tags equal AND ids
///     equal (a keyword/symbol equals nothing but its same-tag same-id self — never
///     numerically coerced, so `(= :a 1)`/`(= :a 'a)` are correctly false);
///   * anything else (strings, floats, bignums, structural values) → `brood_rt_equal`,
///     i.e. `Heap::equal` — the exact `%eq` — through an FFI call; only a lazy seq-view
///     (status 2) deopts, since realising it needs the evaluator.
/// This is what keeps keyword-dispatching arms (`(= (get st :t) :split)` — the regex
/// NFA walkers, any tagged-map code) running native instead of deopting per compare.
/// Deopting on the residual case would make a string compare per activation a guaranteed
/// deopt-thrash latch, leaving the arm interpreted for good (KI-132).
pub(super) fn eq_dispatch(
    b: &mut FunctionBuilder,
    wa: [cranelift_codegen::ir::Value; 3],
    wb: [cranelift_codegen::ir::Value; 3],
    f: Frame,
    fu: Funcs,
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
    let residual = b.create_block();
    b.ins().brif(either, kwb, &[], residual, &[]);
    // Neither inline case: ask the kernel. `Heap::equal` allocates nothing and cannot
    // error, so this is a plain call with the answer in the status; 2 means a seq-view
    // operand, which only the VM's `%eq` can realise → deopt.
    b.switch_to_block(residual);
    let c = b.ins().call(
        fu.equal,
        &[fu.heap, wa[0], wa[1], wa[2], wb[0], wb[1], wb[2]],
    );
    let status = b.inst_results(c)[0];
    let declined = b.ins().icmp_imm_s(IntCC::Equal, status, 2);
    let decided = b.create_block();
    let deopt_reason = deopt_code(b, DeoptReason::EqualSeqViewOperand);
    b.ins().brif(
        declined,
        f.deopt,
        &[BlockArg::Value(deopt_reason)],
        decided,
        &[],
    );
    b.switch_to_block(decided);
    let req = b.ins().icmp_imm_s(IntCC::NotEqual, status, 0);
    b.ins().jump(done, &[BlockArg::Value(req)]);
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

/// What the lowering knows for certain about an operand's numeric type.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NumberKind {
    /// Certainly an `Int`: an unboxed `i64`, an int-carried slot, a slot an int store
    /// cached in this block.
    Int,
    /// Certainly a `Float`: an unboxed `f64`, a float-carried slot, a cached float store.
    Float,
    /// Only the runtime knows — a `Handle`, a frame slot whose flags are a guess.
    Unknown,
    /// Not a number the arithmetic paths read (a bool, a hoisted vector or table).
    Other,
}

pub(super) fn number_kind(b: &FunctionBuilder, op: Op, f: Frame) -> NumberKind {
    match op {
        Op::Int(value) if b.func.dfg.value_type(value) == types::I64 => NumberKind::Int,
        Op::Float(_) => NumberKind::Float,
        Op::Handle(..) => NumberKind::Unknown,
        Op::Slot(slot) => {
            if f.slot_bool.borrow().get(slot).copied().unwrap_or(false) {
                return NumberKind::Other;
            }
            match f.carry_vars.get(slot).copied().flatten() {
                Some((_, true)) => return NumberKind::Float,
                Some((_, false)) => return NumberKind::Int,
                None => {}
            }
            if f.slot_f64_cache
                .borrow()
                .get(slot)
                .copied()
                .flatten()
                .is_some()
            {
                NumberKind::Float
            } else if f
                .slot_i64_cache
                .borrow()
                .get(slot)
                .copied()
                .flatten()
                .is_some()
            {
                NumberKind::Int
            } else {
                NumberKind::Unknown
            }
        }
        _ => NumberKind::Other,
    }
}

/// Should this binary op dispatch by tag instead of guessing? Only in an arm whose deopt
/// feedback found its arithmetic polymorphic (`Frame::arith_dispatch`), only for `+ - * /
/// < <=`, and only when the static type would be a guess: some operand is `Unknown` and
/// none is certainly a `Float` (then promotion is the VM's rule and the float path is no
/// guess) nor anything but a number.
pub(super) fn dispatch_instead_of_guess(
    b: &FunctionBuilder,
    op: PrimOp,
    first: Op,
    second: Op,
    f: Frame,
) -> bool {
    if !f.arith_dispatch
        || !matches!(
            op,
            PrimOp::Add
                | PrimOp::Sub
                | PrimOp::Mul
                | PrimOp::Div
                | PrimOp::Lt
                | PrimOp::Le
                | PrimOp::Max
                | PrimOp::Min
        )
    {
        return false;
    }
    let kinds = [number_kind(b, first, f), number_kind(b, second, f)];
    kinds.contains(&NumberKind::Unknown)
        && !kinds.contains(&NumberKind::Float)
        && !kinds.contains(&NumberKind::Other)
}

/// An operand as the three words of a whole `Value`, for the dispatchers: a register-
/// carried slot's value lives in its variable, not the frame, so a certain int goes through
/// `as_int` and is boxed here; everything else is `read_words`.
fn dispatch_words(b: &mut FunctionBuilder, op: Op, f: Frame) -> [cranelift_codegen::ir::Value; 3] {
    if number_kind(b, op, f) == NumberKind::Int {
        let value = as_int(b, op, f);
        let tag = b.ins().iconst(types::I64, TAG_INT as i64);
        let zero = b.ins().iconst(types::I64, 0);
        [tag, value, zero]
    } else {
        read_words(b, op, f)
    }
}

/// Lower a binary op `dispatch_instead_of_guess` (or a caller's own reasoning) chose to
/// dispatch: a comparison through `cmp_dispatch` (a bool), arithmetic through
/// `arith_dispatch` (a boxed number).
pub(super) fn emit_dispatched(
    b: &mut FunctionBuilder,
    op: PrimOp,
    first: Op,
    second: Op,
    map: [u8; 2],
    f: Frame,
) -> Option<Op> {
    if matches!(op, PrimOp::Max | PrimOp::Min) {
        return Some(min_max(b, op, first, second, map, f));
    }
    let first_words = dispatch_words(b, first, f);
    let second_words = dispatch_words(b, second, f);
    if matches!(op, PrimOp::Lt | PrimOp::Le) {
        Some(Op::Int(cmp_dispatch(
            b,
            op,
            first_words,
            second_words,
            map,
            f,
        )?))
    } else {
        arith_dispatch(b, op, first_words, second_words, map, f)
    }
}

/// Should `max`/`min` on these operands take [`min_max`] rather than the integer path? When an
/// operand is a float, or the arm computes in floats and neither operand is a certain int.
/// Two certain ints keep the integer path's unboxed select.
pub(super) fn min_max_applies(
    b: &FunctionBuilder,
    op: PrimOp,
    first: Op,
    second: Op,
    f: Frame,
) -> bool {
    if !matches!(op, PrimOp::Max | PrimOp::Min) {
        return false;
    }
    let kinds = [number_kind(b, first, f), number_kind(b, second, f)];
    kinds.contains(&NumberKind::Float) || (f.float_context && !kinds.contains(&NumberKind::Int))
}

/// `(%max x y)` / `(%min x y)` with a float about — the VM's `extremum`, exactly: the FIRST
/// argument stands unless `first < second` (max) or `first > second` (min) as a numeric
/// ordering, so a tie keeps the first, a NaN (unordered) keeps the first, and the answer is
/// the winning OPERAND itself — an int that wins stays an int (`(math/max 5 3.0)` is `5`).
/// Two certain floats compare and select unboxed; otherwise the operands' words are
/// compared by tag (int × int as ints, any int/float mix as floats, an int promoted as the
/// VM's `num_to_f64` does) and the winner's words are the answer, boxed in registers. A
/// non-number deopts: a record's `Ord` and a decimal are the VM's. Before this a float
/// `max`/`min` bailed the whole arm off native code — `math/clamp` and everything it inlines
/// into ran on the VM. `map` orders the operands as for `emit_arith`.
pub(super) fn min_max(
    b: &mut FunctionBuilder,
    op: PrimOp,
    first: Op,
    second: Op,
    map: [u8; 2],
    f: Frame,
) -> Op {
    use cranelift_codegen::ir::condcodes::FloatCC;
    let (float_cc, int_cc) = if matches!(op, PrimOp::Max) {
        (FloatCC::LessThan, IntCC::SignedLessThan)
    } else {
        (FloatCC::GreaterThan, IntCC::SignedGreaterThan)
    };
    let (first, second) = if map[0] == 0 {
        (first, second)
    } else {
        (second, first)
    };
    if number_kind(b, first, f) == NumberKind::Float
        && number_kind(b, second, f) == NumberKind::Float
    {
        let x = as_f64(b, first, f);
        let y = as_f64(b, second, f);
        let replace = b.ins().fcmp(float_cc, x, y);
        return Op::Float(b.ins().select(replace, y, x));
    }
    let x = dispatch_words(b, first, f);
    let y = dispatch_words(b, second, f);
    let x_tag = b.ins().band_imm_s(x[0], 0xff);
    let y_tag = b.ins().band_imm_s(y[0], 0xff);
    let done = b.create_block();
    b.append_block_param(done, types::I8);
    let x_int = b.ins().icmp_imm_s(IntCC::Equal, x_tag, TAG_INT as i64);
    let y_int = b.ins().icmp_imm_s(IntCC::Equal, y_tag, TAG_INT as i64);
    let both_int = b.ins().band(x_int, y_int);
    let int_block = b.create_block();
    let float_block = b.create_block();
    b.ins().brif(both_int, int_block, &[], float_block, &[]);
    b.switch_to_block(int_block);
    let replace = b.ins().icmp(int_cc, x[1], y[1]);
    b.ins().jump(done, &[BlockArg::Value(replace)]);
    b.switch_to_block(float_block);
    let x_float = float_or_promoted_int(b, x_tag, x[1], f, DeoptReason::MinMaxFirstOperand, true);
    let y_float = float_or_promoted_int(b, y_tag, y[1], f, DeoptReason::MinMaxSecondOperand, true);
    let replace = b.ins().fcmp(float_cc, x_float, y_float);
    b.ins().jump(done, &[BlockArg::Value(replace)]);
    b.switch_to_block(done);
    let replace = b.block_params(done)[0];
    let pick = |b: &mut FunctionBuilder, index: usize| b.ins().select(replace, y[index], x[index]);
    let (w0, w1, w2) = (pick(b, 0), pick(b, 1), pick(b, 2));
    Op::Handle(w0, w1, w2)
}

/// `+ - * /` dispatched at RUNTIME by tag, for an operand whose type no guess can settle:
/// both `Int` compute as ints (overflow, a zero divisor and an inexact quotient deopt as on
/// the integer path), anything else reads both as floats — an `Int` beside a `Float`
/// promoted, the VM's own rule — and a non-number deopts. The answer is boxed (an
/// `Op::Handle` of words in registers, no allocation), since its type is the runtime's.
///
/// For the case neither static path can lower: an operand that can never be a float (an
/// unboxed `Int`, a `count`) beside a type-erased one in a float-context arm —
/// `(* (count cards) (hash key 1 seed))`. The integer path deopts every time the erased
/// operand is a float, the float path every time no operand is (`as_f64_pair` refuses the
/// never-float side), so a self-tail loop of that shape — which is never deopt-watched —
/// fell out of native code on every activation for good. `map` orders the operands as for
/// `emit_arith`.
pub(super) fn arith_dispatch(
    b: &mut FunctionBuilder,
    op: PrimOp,
    first: [cranelift_codegen::ir::Value; 3],
    second: [cranelift_codegen::ir::Value; 3],
    map: [u8; 2],
    f: Frame,
) -> Option<Op> {
    let first_tag = b.ins().band_imm_s(first[0], 0xff);
    let second_tag = b.ins().band_imm_s(second[0], 0xff);
    let done = b.create_block();
    b.append_block_param(done, types::I64);
    b.append_block_param(done, types::I64);
    let first_int = b.ins().icmp_imm_s(IntCC::Equal, first_tag, TAG_INT as i64);
    let second_int = b.ins().icmp_imm_s(IntCC::Equal, second_tag, TAG_INT as i64);
    let both_int = b.ins().band(first_int, second_int);
    let int_block = b.create_block();
    let float_block = b.create_block();
    b.ins().brif(both_int, int_block, &[], float_block, &[]);
    b.switch_to_block(int_block);
    let (left, right) = if map[0] == 0 {
        (first[1], second[1])
    } else {
        (second[1], first[1])
    };
    let int_result = emit_arith(b, op, left, right, f.deopt)?;
    let int_tag = b.ins().iconst(types::I64, TAG_INT as i64);
    b.ins().jump(
        done,
        &[BlockArg::Value(int_tag), BlockArg::Value(int_result)],
    );
    b.switch_to_block(float_block);
    let first_float = float_or_promoted_int(
        b,
        first_tag,
        first[1],
        f,
        DeoptReason::ArithDispatchFirstOperand,
        true,
    );
    let second_float = float_or_promoted_int(
        b,
        second_tag,
        second[1],
        f,
        DeoptReason::ArithDispatchSecondOperand,
        true,
    );
    let (left, right) = if map[0] == 0 {
        (first_float, second_float)
    } else {
        (second_float, first_float)
    };
    let float_result = match emit_float_arith(b, op, left, right, f.deopt)? {
        Op::Float(value) => value,
        _ => return super::bail("arith-dispatch-not-arithmetic"),
    };
    let float_bits = b
        .ins()
        .bitcast(types::I64, MemFlagsData::new(), float_result);
    let float_tag = b.ins().iconst(types::I64, TAG_FLOAT as i64);
    b.ins().jump(
        done,
        &[BlockArg::Value(float_tag), BlockArg::Value(float_bits)],
    );
    b.switch_to_block(done);
    let tag = b.block_params(done)[0];
    let payload = b.block_params(done)[1];
    let zero = b.ins().iconst(types::I64, 0);
    Some(Op::Handle(tag, payload, zero))
}

/// `<` / `<=` on two operands nothing types (`Handle`s, or slots with no profile and no
/// store's flag) — dispatched at RUNTIME by tag, the way `eq_dispatch` does `=`: both
/// `Int` compares as ints, anything else reads both as floats (an `Int` beside a `Float`
/// promoted, the VM's own rule; a non-number deopts). A comparison's answer is a bool
/// whichever path took it, so nothing has to be boxed afterwards, and no guess is made:
/// `(< j (count xs))` on two let-bound ints in a float-context arm stays native and exact
/// where the optimistic float path deopted on every call. `map` orders the operands as
/// for `emit_arith`. Costs the int×int case two tag tests, as `eq_dispatch` does.
pub(super) fn cmp_dispatch(
    b: &mut FunctionBuilder,
    op: PrimOp,
    wa: [cranelift_codegen::ir::Value; 3],
    wb: [cranelift_codegen::ir::Value; 3],
    map: [u8; 2],
    f: Frame,
) -> Option<cranelift_codegen::ir::Value> {
    let ta = b.ins().band_imm_s(wa[0], 0xff);
    let tb = b.ins().band_imm_s(wb[0], 0xff);
    let done = b.create_block();
    b.append_block_param(done, types::I8);
    let a_int = b.ins().icmp_imm_s(IntCC::Equal, ta, TAG_INT as i64);
    let b_int = b.ins().icmp_imm_s(IntCC::Equal, tb, TAG_INT as i64);
    let both_int = b.ins().band(a_int, b_int);
    let intb = b.create_block();
    let floatb = b.create_block();
    b.ins().brif(both_int, intb, &[], floatb, &[]);
    b.switch_to_block(intb);
    let (x, y) = if map[0] == 0 {
        (wa[1], wb[1])
    } else {
        (wb[1], wa[1])
    };
    let ir = emit_arith(b, op, x, y, f.deopt)?;
    b.ins().jump(done, &[BlockArg::Value(ir)]);
    b.switch_to_block(floatb);
    // Not both ints: each is a float or an int promoted beside one, or it deopts (a
    // string compared with a number is the VM's type error to raise).
    let fa = float_or_promoted_int(b, ta, wa[1], f, DeoptReason::CompareFirstOperand, true);
    let fb = float_or_promoted_int(b, tb, wb[1], f, DeoptReason::CompareSecondOperand, true);
    let (x, y) = if map[0] == 0 { (fa, fb) } else { (fb, fa) };
    let fr = match emit_float_arith(b, op, x, y, f.deopt)? {
        Op::Int(v) => v,
        _ => return super::bail("cmp-dispatch-not-a-comparison"),
    };
    b.ins().jump(done, &[BlockArg::Value(fr)]);
    b.switch_to_block(done);
    Some(b.block_params(done)[0])
}

/// Inline read of `(nth v <const idx>)` for a LOCAL small (inline) vector, the analog of
/// the pair `first`/`rest` inline. Fetches the vector-slab base *per read* (a trivial
/// FFI, not the hoist used for pairs) so it is safe even in arms with GC safepoints (a
/// non-tail `Call` between reads) — `bintree`'s `check` is exactly that. A non-LOCAL
/// (PRELUDE/RUNTIME) vector reads through the `brood_rt_vector_ref` callback; the other
/// slow conditions (not a `Vector`, an unexpected storage layout, or an out-of-range
/// index) deopt to the VM, which produces `nth`'s exact result. Element read is `slot +
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
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefNotVector);
    b.ins()
        .brif(is_vec, c1, &[], deopt, &[BlockArg::Value(deopt_reason)]);
    b.switch_to_block(c1);
    // Region: high 2 bits of the handle == 0 (LOCAL). A RUNTIME handle's slot is computed
    // inline (`shared_vec_slot`), and PRELUDE, or a RUNTIME one it cannot place yet, takes
    // the FFI (`brood_rt_vector_ref`, which reads any region) rather than deopting.
    // Deopting here instead would latch any arm destructuring a literal or `def`'d vector
    // off the native tier (the vector form of the pair path's cliff — see
    // `PrimOp1::First`'s lowering).
    let high2 = b.ins().ushr_imm_s(w1, 62);
    let is_local = b.ins().icmp_imm_s(IntCC::Equal, high2, 0);
    let c2 = b.create_block();
    let ffi_blk = b.create_block();
    // A RUNTIME vector's slot is computed inline too (`shared_vec_slot`); both slot
    // pointers converge on the one storage decode.
    let decode = b.create_block();
    b.append_block_param(decode, ptr_ty);
    let shared = b.create_block();
    b.ins().brif(is_local, c2, &[], shared, &[]);
    b.switch_to_block(shared);
    match shared_vec_slot(b, w1, funcs, ffi_blk) {
        Some(slot) => {
            b.ins().jump(decode, &[BlockArg::Value(slot)]);
        }
        None => {
            b.ins().jump(ffi_blk, &[]);
        }
    }
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
    let local_slot = b.ins().iadd(base, slot_off);
    b.ins().jump(decode, &[BlockArg::Value(local_slot)]);
    b.switch_to_block(decode);
    let slot_ptr = b.block_params(decode)[0];
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
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefOutOfRange);
    b.ins()
        .brif(in_bounds, c4, &[], deopt, &[BlockArg::Value(deopt_reason)]);
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
    // Spill or Shared: both read through the cached `ptr`/`len` (ADR-511).
    let is_spill = b
        .ins()
        .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, disc, VS::JIT_SPILL_TAG);
    let spill_blk = b.create_block();
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefUnexpectedStore);
    b.ins().brif(
        is_spill,
        spill_blk,
        &[],
        deopt,
        &[BlockArg::Value(deopt_reason)],
    );
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
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefSpilledOutOfRange);
    b.ins()
        .brif(in_b, sok, &[], deopt, &[BlockArg::Value(deopt_reason)]);
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
    // The shared-region read: one bounds-checked callback, status 1 (non-vector / out of
    // range — neither possible past the tag check above except the range) deopts so the
    // VM produces `nth`'s exact result.
    b.switch_to_block(ffi_blk);
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
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefDeclined);
    b.ins()
        .brif(hstatus, deopt, &[BlockArg::Value(deopt_reason)], hok, &[]);
    b.switch_to_block(hok);
    let (h0, h1, h2) = read_out_slot(b, out_slot);
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

/// Decode a vector's element storage once, for a run of reads (`Frame::slot_vec_cache`):
/// the tag must be `Vector` (else deopt, as [`inline_vec_ref`] does), and
/// `brood_rt_vector_base` answers the address of element 0 and the length for a vector in
/// any region. One callback per vector instead of a tag, region, storage-kind check and a
/// slab-base callback per element.
///
/// The address is valid only until something allocates or calls — the caller keeps it no
/// longer than `prepass::keeps_vector_storage` allows.
pub(super) fn vector_storage(
    b: &mut FunctionBuilder,
    vec: [cranelift_codegen::ir::Value; 3],
    frame: Frame,
    funcs: Funcs,
) -> VectorStorage {
    let tag = b.ins().band_imm_s(vec[0], 0xff);
    let is_vector = b.ins().icmp_imm_s(IntCC::Equal, tag, TAG_VECTOR as i64);
    let decode = b.create_block();
    let deopt_reason = deopt_code(b, DeoptReason::InlineVectorRefNotVector);
    b.ins().brif(
        is_vector,
        decode,
        &[],
        frame.deopt,
        &[BlockArg::Value(deopt_reason)],
    );
    b.switch_to_block(decode);
    let length_slot = b.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
        cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
        8,
        3,
    ));
    let length_address = b.ins().stack_addr(funcs.ptr_ty, length_slot, 0);
    let call = b.ins().call(
        funcs.vbase,
        &[funcs.heap, vec[0], vec[1], vec[2], length_address],
    );
    let elements = b.inst_results(call)[0];
    let length = b.ins().stack_load(types::I64, types::I64, length_slot, 0);
    VectorStorage { elements, length }
}

/// Element `index` of a decoded vector storage ([`vector_storage`]): a bounds check (out of
/// range deopts — the VM owns `nth`'s exact answer) and the element's three words.
pub(super) fn storage_element(
    b: &mut FunctionBuilder,
    storage: VectorStorage,
    index: i64,
    frame: Frame,
) -> Op {
    let index_value = b.ins().iconst(types::I64, index);
    let in_bounds = b
        .ins()
        .icmp(IntCC::UnsignedLessThan, index_value, storage.length);
    let read = b.create_block();
    let deopt_reason = deopt_code(b, DeoptReason::CachedVectorRefOutOfRange);
    b.ins().brif(
        in_bounds,
        read,
        &[],
        frame.deopt,
        &[BlockArg::Value(deopt_reason)],
    );
    b.switch_to_block(read);
    let element = b.ins().iadd_imm_s(storage.elements, index * STRIDE);
    let word0 = b
        .ins()
        .load(types::I64, MemFlagsData::trusted(), element, 0);
    let word1 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        element,
        PAYLOAD_OFFSET as i32,
    );
    let word2 = b.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        element,
        PAYLOAD_OFFSET as i32 + 8,
    );
    Op::Handle(word0, word1, word2)
}
