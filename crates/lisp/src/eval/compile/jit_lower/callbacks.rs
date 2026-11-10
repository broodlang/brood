//! The `brood_rt_*` runtime callbacks a lowered arm may call: their Cranelift signatures,
//! declared as module imports once per arm ([`declare_callbacks`]), then imported into the
//! function being built ([`import_callbacks`]). The callbacks themselves live in
//! `crate::jit::rt`; a symbol declared here must be registered there.
#![cfg(feature = "jit")]
use super::OrBail;
use cranelift_codegen::ir::{AbiParam, FuncRef, Function, SigRef, Signature, Type, types};
use cranelift_frontend::FunctionBuilder;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Linkage, Module};

/// The module-level ids of every runtime callback, plus the callee-arm signature the inline
/// call path uses for `call_indirect`.
pub(super) struct CallbackIds {
    note_deopt: FuncId,
    trace_push: FuncId,
    tick_n: FuncId,
    car: FuncId,
    cdr: FuncId,
    first: FuncId,
    rest: FuncId,
    is_empty: FuncId,
    pair_nursery_base: FuncId,
    stack_recheck: FuncId,
    pair_old_base: FuncId,
    vector_nursery_base: FuncId,
    vector_old_base: FuncId,
    cons: FuncId,
    vector2_room: FuncId,
    make_vector_n: FuncId,
    make_map_n: FuncId,
    gc_safepoint: FuncId,
    #[cfg(debug_assertions)]
    debug_set_staging: FuncId,
    #[cfg(debug_assertions)]
    debug_check_slot: FuncId,
    global: FuncId,
    global_probe: FuncId,
    global_ic: FuncId,
    call_slow: FuncId,
    push_room: FuncId,
    in_call_journal: FuncId,
    call_native_flat: FuncId,
    fastlink_base: FuncId,
    fast_frame: FuncId,
    xcall_latch: FuncId,
    xcall_cold: FuncId,
    vector_ref: FuncId,
    table_has: FuncId,
    table_get: FuncId,
    map_get: FuncId,
    vector_len: FuncId,
    count: FuncId,
    equal: FuncId,
    table_put: FuncId,
    map_get3: FuncId,
    vector_ref3: FuncId,
    map_assoc: FuncId,
    vector_base: FuncId,
    table_dense_base: FuncId,
    global_epoch_pointer: FuncId,
    const_load: FuncId,
    /// The callee-arm signature (`crate::jit::JitArmFn`): `(heap, base, out, ctx) -> outcome`.
    arm_function: Signature,
}

/// The callbacks imported into the function being built — what the emitters call.
pub(super) struct CallbackRefs {
    pub(super) note_deopt: FuncRef,
    pub(super) trace_push: FuncRef,
    pub(super) tick_n: FuncRef,
    pub(super) car: FuncRef,
    pub(super) cdr: FuncRef,
    pub(super) first: FuncRef,
    pub(super) rest: FuncRef,
    pub(super) is_empty: FuncRef,
    pub(super) pair_nursery_base: FuncRef,
    pub(super) stack_recheck: FuncRef,
    pub(super) pair_old_base: FuncRef,
    pub(super) vector_nursery_base: FuncRef,
    pub(super) vector_old_base: FuncRef,
    pub(super) cons: FuncRef,
    pub(super) vector2_room: FuncRef,
    pub(super) make_vector_n: FuncRef,
    pub(super) make_map_n: FuncRef,
    pub(super) gc_safepoint: FuncRef,
    #[cfg(debug_assertions)]
    pub(super) debug_set_staging: FuncRef,
    pub(super) global: FuncRef,
    pub(super) global_probe: FuncRef,
    pub(super) global_ic: FuncRef,
    pub(super) call_slow: FuncRef,
    pub(super) push_room: FuncRef,
    pub(super) in_call_journal: FuncRef,
    pub(super) call_native_flat: FuncRef,
    pub(super) fastlink_base: FuncRef,
    pub(super) fast_frame: FuncRef,
    pub(super) xcall_latch: FuncRef,
    pub(super) xcall_cold: FuncRef,
    pub(super) arm_function: SigRef,
    pub(super) vector_ref: FuncRef,
    pub(super) table_has: FuncRef,
    pub(super) table_get: FuncRef,
    pub(super) map_get: FuncRef,
    pub(super) vector_len: FuncRef,
    pub(super) count: FuncRef,
    pub(super) equal: FuncRef,
    pub(super) table_put: FuncRef,
    pub(super) map_get3: FuncRef,
    pub(super) vector_ref3: FuncRef,
    pub(super) map_assoc: FuncRef,
    pub(super) vector_base: FuncRef,
    pub(super) table_dense_base: FuncRef,
    pub(super) global_epoch_pointer: FuncRef,
    pub(super) const_load: FuncRef,
}

/// A signature of `params`, returning `returns`.
fn signature(module: &JITModule, params: &[Type], returns: &[Type]) -> Signature {
    let mut sig = module.make_signature();
    sig.params.extend(params.iter().map(|t| AbiParam::new(*t)));
    sig.returns
        .extend(returns.iter().map(|t| AbiParam::new(*t)));
    sig
}

fn import(module: &mut JITModule, name: &str, sig: &Signature) -> Option<FuncId> {
    module
        .declare_function(name, Linkage::Import, sig)
        .ok()
        .or_bail("cranelift-declare-function")
}

/// Declare every runtime callback in `module`. A `Value` is 24 bytes, so a callback that
/// takes one takes its three words, and one that produces one writes it through an
/// out-pointer. A `status` return is 0 = ok, nonzero = deopt / error parked (per callback).
/// Most callbacks that consult activation state take the `JitCallCtx` pointer last.
pub(super) fn declare_callbacks(module: &mut JITModule, pointer: Type) -> Option<CallbackIds> {
    let i8 = types::I8;
    let i32 = types::I32;
    let i64 = types::I64;
    let word3 = [i64, i64, i64];
    // brood_rt_note_deopt(heap, reason): records WHY the arm is deopting — the shared deopt
    // block takes the reason as a block param, so every guard names itself (`DeoptReason`).
    let note_deopt = import(
        module,
        "brood_rt_note_deopt",
        &signature(module, &[pointer, i32], &[]),
    )?;
    // brood_rt_trace_push(heap, arm): records this arm in the parked error's `:trace` as the
    // error exits it (KI-117).
    let trace_push = import(
        module,
        "brood_rt_trace_push",
        &signature(module, &[pointer, i64], &[]),
    )?;
    // brood_rt_tick_n(heap, n, ctx) -> u8: the batched back-edge poll (burns n reductions);
    // the activation context says whether a spent budget may preempt (ADR-401).
    let tick_n = import(
        module,
        "brood_rt_tick_n",
        &signature(module, &[pointer, i64, pointer], &[i8]),
    )?;
    // brood_rt_{car,cdr}(heap, out, w0, w1, w2).
    let car_signature = signature(module, &[pointer, pointer, i64, i64, i64], &[]);
    let car = import(module, "brood_rt_car", &car_signature)?;
    let cdr = import(module, "brood_rt_cdr", &car_signature)?;
    // brood_rt_{first,rest,is_empty}(heap, out, w0, w1, w2) -> status: the NON-pair fallback
    // of the inline `first`/`rest` (0 = `*out` holds the answer, 1 = deopt, 2 = error parked).
    let sequence_signature = signature(module, &[pointer, pointer, i64, i64, i64], &[i64]);
    let first = import(module, "brood_rt_first", &sequence_signature)?;
    let rest = import(module, "brood_rt_rest", &sequence_signature)?;
    let is_empty = import(module, "brood_rt_is_empty", &sequence_signature)?;
    // LOCAL pair- and vector-slab base pointers (heap -> *const u8), fetched once per entry
    // so the inline `first`/`rest`/`nth` reads are plain loads.
    let base_signature = signature(module, &[pointer], &[pointer]);
    let pair_nursery_base = import(module, "brood_rt_pair_nursery_base", &base_signature)?;
    // brood_rt_stack_recheck(heap) -> i8: the stack guard's slow path (1 = room, go on).
    let stack_recheck = import(
        module,
        "brood_rt_stack_recheck",
        &signature(module, &[pointer], &[i8]),
    )?;
    let pair_old_base = import(module, "brood_rt_pair_old_base", &base_signature)?;
    let vector_nursery_base = import(module, "brood_rt_vec_nursery_base", &base_signature)?;
    let vector_old_base = import(module, "brood_rt_vec_old_base", &base_signature)?;
    // brood_rt_cons(heap, out, car 3 words, cdr 3 words).
    let cons = import(
        module,
        "brood_rt_cons",
        &signature(
            module,
            &[pointer, pointer, i64, i64, i64, i64, i64, i64],
            &[],
        ),
    )?;
    // brood_rt_vec2_room(heap, out) -> *mut Value: allocate a 2-element vector, write the
    // handle to `*out`, and return its element storage for the arm to fill in place.
    let vector2_room = import(
        module,
        "brood_rt_vec2_room",
        &signature(module, &[pointer, pointer], &[pointer]),
    )?;
    // brood_rt_make_{vector,map}_n(heap, out, elems: *const Value, n) — the variadic
    // literals, from `Value`s staged contiguously in a stack slot the arm owns. Neither
    // collects, so the staged bytes stay live across the call.
    let literal_signature = signature(module, &[pointer, pointer, pointer, i64], &[]);
    let make_vector_n = import(module, "brood_rt_make_vector_n", &literal_signature)?;
    let make_map_n = import(module, "brood_rt_make_map_n", &literal_signature)?;
    // brood_rt_gc_safepoint(heap): collect if due (bounds the nursery for cons loops).
    let gc_safepoint = import(
        module,
        "brood_rt_gc_safepoint",
        &signature(module, &[pointer], &[]),
    )?;
    // DEBUG ONLY: brood_rt_dbg_set_staging(heap, site) — record the staging call site.
    #[cfg(debug_assertions)]
    let debug_set_staging = import(
        module,
        "brood_rt_dbg_set_staging",
        &signature(module, &[pointer, i32], &[]),
    )?;
    // DEBUG ONLY: brood_rt_dbg_check_slot(heap, w0, w1, w2, absolute_index) — declared for
    // ad-hoc slot-read validation during bug hunts; no emitted code calls it.
    #[cfg(debug_assertions)]
    let debug_check_slot = import(
        module,
        "brood_rt_dbg_check_slot",
        &signature(module, &[pointer, i64, i64, i64, i64], &[]),
    )?;
    // brood_rt_global(heap, out, sym, ctx) -> status: resolve a free global into `*out`.
    // brood_rt_global_probe: the same, but WITHOUT parking an unbound error — the entry
    // hoist deopts on unbound rather than raising.
    let global_signature = signature(module, &[pointer, pointer, i32, pointer], &[i64]);
    let global = import(module, "brood_rt_global", &global_signature)?;
    let global_probe = import(module, "brood_rt_global_probe", &global_signature)?;
    // brood_rt_global_ic(heap, out, sym, site, ctx) -> status: through the per-site cache.
    let global_ic = import(
        module,
        "brood_rt_global_ic",
        &signature(module, &[pointer, pointer, i32, i32, pointer], &[i64]),
    )?;
    // brood_rt_call_slow(heap, out, argc, site, head, ctx) -> status: dispatch the staged
    // call into `*out` (site `NO_SITE` / head `u32::MAX` when absent).
    let call_slow = import(
        module,
        "brood_rt_call_slow",
        &signature(module, &[pointer, pointer, i32, i32, i32, pointer], &[i64]),
    )?;
    // brood_rt_push_room(heap, n) -> *mut Value: reserve n argument slots at the top of
    // `roots` so the arm's operand stores land in place (`Heap::push_roots_room`).
    let push_room = import(
        module,
        "brood_rt_push_room",
        &signature(module, &[pointer, i64], &[pointer]),
    )?;
    // brood_rt_in_call_journal(heap, packed) -> *mut Value: on an error path, record this
    // activation as paused mid-call for the preempt unwinding through it (ADR-401) and
    // reserve room for its operands — null when the error is a real one.
    let in_call_journal = import(
        module,
        "brood_rt_in_call_journal",
        &signature(module, &[pointer, i64], &[pointer]),
    )?;
    // brood_rt_call_native_fl(heap, out, func, args, argc, ctx) -> status: direct builtin
    // call for a native flat-cell hit — no roots staging at all.
    let call_native_flat = import(
        module,
        "brood_rt_call_native_fl",
        &signature(
            module,
            &[pointer, pointer, i64, pointer, i32, pointer],
            &[i64],
        ),
    )?;
    // brood_rt_fastlink_base(heap, out_len, ctx) -> *const FastLink: base + length of the
    // IR-readable fast-link mirror.
    let fastlink_base = import(
        module,
        "brood_rt_fastlink_base",
        &signature(module, &[pointer, pointer, pointer], &[pointer]),
    )?;
    // brood_rt_fast_frame(heap, out, site, slot, ctx) -> status: run the (already
    // epoch-validated) native fast-link — every argument register-passed; the callee reads
    // the link's fields through `slot`. 0 = done, 1 = error parked, 2 = could not link.
    let fast_frame = import(
        module,
        "brood_rt_fast_frame",
        &signature(module, &[pointer, pointer, i32, pointer, pointer], &[i64]),
    )?;
    // §7.5 — the inline fast-frame path's two cold callbacks.
    // brood_rt_xcall_latch(heap, code, site<<32|head, argc, epoch, ctx): suspend-host latch
    // resolution after the gateway-token compare matched.
    let xcall_latch = import(
        module,
        "brood_rt_xcall_latch",
        &signature(module, &[pointer, i64, i64, i64, i64, pointer], &[]),
    )?;
    // brood_rt_xcall_cold(heap, outcome, out, site<<32|head, argc<<32|nslots, epoch,
    // stage_base, ctx) -> status: the deopt/preempt/tail/error outcomes (everything but 0).
    let xcall_cold = import(
        module,
        "brood_rt_xcall_cold",
        &signature(
            module,
            &[pointer, i64, pointer, i64, i64, i64, i64, pointer],
            &[i64],
        ),
    )?;
    let arm_function = signature(module, &[pointer, i64, pointer, pointer], &[i64]);
    // (heap, out, a 3 words, b 3 words) -> status: brood_rt_vector_ref (bounds-checked slab
    // read; 1 = deopt), brood_rt_table_has / _get2 and brood_rt_map_get (2 = error parked).
    let pair_signature = signature(
        module,
        &[pointer, pointer, i64, i64, i64, i64, i64, i64],
        &[i64],
    );
    let vector_ref = import(module, "brood_rt_vector_ref", &pair_signature)?;
    let table_has = import(module, "brood_rt_table_has", &pair_signature)?;
    let table_get = import(module, "brood_rt_table_get2", &pair_signature)?;
    let map_get = import(module, "brood_rt_map_get", &pair_signature)?;
    // brood_rt_vector_len(heap, vec 3 words) -> len, or -1 for a non-vector (deopt).
    let vector_len = import(
        module,
        "brood_rt_vector_len",
        &signature(module, &[pointer, i64, i64, i64], &[i64]),
    )?;
    // brood_rt_count(heap, value 3 words) -> count, or -1 to decline (deopt).
    let count = import(
        module,
        "brood_rt_count",
        &signature(module, &[pointer, i64, i64, i64], &[i64]),
    )?;
    // brood_rt_equal(heap, a 3 words, b 3 words) -> 1 equal / 0 not / 2 declined — the
    // residual case of `eq_dispatch` (KI-132). The answer is the status.
    let equal = import(
        module,
        "brood_rt_equal",
        &signature(module, &[pointer, i64, i64, i64, i64, i64, i64], &[i64]),
    )?;
    // (heap, out, 3 words ×3) -> status: brood_rt_table_put, and the ADR-368 map ops
    // brood_rt_map_get3 / brood_rt_map_assoc (receiver, key, default-or-value).
    let mut triple_params = vec![pointer, pointer];
    triple_params.extend([word3, word3, word3].concat());
    let triple_signature = signature(module, &triple_params, &[i64]);
    let table_put = import(module, "brood_rt_table_put", &triple_signature)?;
    let map_get3 = import(module, "brood_rt_map_get3", &triple_signature)?;
    let vector_ref3 = import(module, "brood_rt_vector_ref3", &triple_signature)?;
    let map_assoc = import(module, "brood_rt_map_assoc", &triple_signature)?;
    // (heap, 3 words, out: *mut i64) -> *const: brood_rt_vector_base resolves an invariant
    // vector's element (data_ptr, len) once for the LICM hoist (null = not a vector);
    // brood_rt_table_dense_base a hoisted global table's dense slot region (null = per-op
    // callbacks instead).
    let base_of_signature = signature(module, &[pointer, i64, i64, i64, pointer], &[pointer]);
    let vector_base = import(module, "brood_rt_vector_base", &base_of_signature)?;
    let table_dense_base = import(module, "brood_rt_table_dense_base", &base_of_signature)?;
    // brood_rt_global_epoch_ptr(heap) -> *const u64: the global-rebind epoch counter's
    // address, fetched once at entry so the back-edge guard and the per-call epoch check
    // are raw loads.
    let global_epoch_pointer = import(
        module,
        "brood_rt_global_epoch_ptr",
        &signature(module, &[pointer], &[pointer]),
    )?;
    // brood_rt_const_load(cv: *const ConstVal, out: *mut Value): the current bits of a
    // GC-movable constant. Never fails.
    let const_load = import(
        module,
        "brood_rt_const_load",
        &signature(module, &[pointer, pointer], &[]),
    )?;
    Some(CallbackIds {
        note_deopt,
        trace_push,
        tick_n,
        car,
        cdr,
        first,
        rest,
        is_empty,
        pair_nursery_base,
        stack_recheck,
        pair_old_base,
        vector_nursery_base,
        vector_old_base,
        cons,
        vector2_room,
        make_vector_n,
        make_map_n,
        gc_safepoint,
        #[cfg(debug_assertions)]
        debug_set_staging,
        #[cfg(debug_assertions)]
        debug_check_slot,
        global,
        global_probe,
        global_ic,
        call_slow,
        push_room,
        in_call_journal,
        call_native_flat,
        fastlink_base,
        fast_frame,
        xcall_latch,
        xcall_cold,
        vector_ref,
        table_has,
        table_get,
        map_get,
        vector_len,
        count,
        equal,
        table_put,
        map_get3,
        vector_ref3,
        map_assoc,
        vector_base,
        table_dense_base,
        global_epoch_pointer,
        const_load,
        arm_function,
    })
}

/// Import every declared callback into the function `b` is building.
pub(super) fn import_callbacks(
    module: &mut JITModule,
    ids: &CallbackIds,
    b: &mut FunctionBuilder,
) -> CallbackRefs {
    let mut declare = |id: FuncId, func: &mut Function| module.declare_func_in_func(id, func);
    let tick_n = declare(ids.tick_n, b.func);
    let car = declare(ids.car, b.func);
    let first = declare(ids.first, b.func);
    let rest = declare(ids.rest, b.func);
    let is_empty = declare(ids.is_empty, b.func);
    let cdr = declare(ids.cdr, b.func);
    let pair_nursery_base = declare(ids.pair_nursery_base, b.func);
    let stack_recheck = declare(ids.stack_recheck, b.func);
    let pair_old_base = declare(ids.pair_old_base, b.func);
    let vector_nursery_base = declare(ids.vector_nursery_base, b.func);
    let vector_old_base = declare(ids.vector_old_base, b.func);
    let cons = declare(ids.cons, b.func);
    let vector2_room = declare(ids.vector2_room, b.func);
    let make_vector_n = declare(ids.make_vector_n, b.func);
    let make_map_n = declare(ids.make_map_n, b.func);
    let gc_safepoint = declare(ids.gc_safepoint, b.func);
    #[cfg(debug_assertions)]
    let debug_set_staging = declare(ids.debug_set_staging, b.func);
    #[cfg(debug_assertions)]
    declare(ids.debug_check_slot, b.func);
    let global = declare(ids.global, b.func);
    let global_probe = declare(ids.global_probe, b.func);
    let global_ic = declare(ids.global_ic, b.func);
    let call_slow = declare(ids.call_slow, b.func);
    let push_room = declare(ids.push_room, b.func);
    let in_call_journal = declare(ids.in_call_journal, b.func);
    let call_native_flat = declare(ids.call_native_flat, b.func);
    let fastlink_base = declare(ids.fastlink_base, b.func);
    let fast_frame = declare(ids.fast_frame, b.func);
    let xcall_latch = declare(ids.xcall_latch, b.func);
    let xcall_cold = declare(ids.xcall_cold, b.func);
    let arm_function = b.import_signature(ids.arm_function.clone());
    let note_deopt = declare(ids.note_deopt, b.func);
    let trace_push = declare(ids.trace_push, b.func);
    let vector_ref = declare(ids.vector_ref, b.func);
    let table_has = declare(ids.table_has, b.func);
    let table_get = declare(ids.table_get, b.func);
    let map_get = declare(ids.map_get, b.func);
    let vector_len = declare(ids.vector_len, b.func);
    let count = declare(ids.count, b.func);
    let equal = declare(ids.equal, b.func);
    let table_put = declare(ids.table_put, b.func);
    let map_get3 = declare(ids.map_get3, b.func);
    let vector_ref3 = declare(ids.vector_ref3, b.func);
    let map_assoc = declare(ids.map_assoc, b.func);
    let vector_base = declare(ids.vector_base, b.func);
    let table_dense_base = declare(ids.table_dense_base, b.func);
    let global_epoch_pointer = declare(ids.global_epoch_pointer, b.func);
    let const_load = declare(ids.const_load, b.func);
    CallbackRefs {
        note_deopt,
        trace_push,
        tick_n,
        car,
        cdr,
        first,
        rest,
        is_empty,
        pair_nursery_base,
        stack_recheck,
        pair_old_base,
        vector_nursery_base,
        vector_old_base,
        cons,
        vector2_room,
        make_vector_n,
        make_map_n,
        gc_safepoint,
        #[cfg(debug_assertions)]
        debug_set_staging,
        global,
        global_probe,
        global_ic,
        call_slow,
        push_room,
        in_call_journal,
        call_native_flat,
        fastlink_base,
        fast_frame,
        xcall_latch,
        xcall_cold,
        arm_function,
        vector_ref,
        table_has,
        table_get,
        map_get,
        vector_len,
        count,
        equal,
        table_put,
        map_get3,
        vector_ref3,
        map_assoc,
        vector_base,
        table_dense_base,
        global_epoch_pointer,
        const_load,
    }
}
