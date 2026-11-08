//! VM arm dispatcher: Call/SelfCall/IC/JIT fast path.
use super::*;
#[cfg(feature = "jit")]
use crate::jit::outcome;

/// Restores the `capture_top_level` flag on drop — so the gate is reset even if the
/// guarded tree-walker `apply` *panics* (caught by `run_one`'s `catch_unwind`); a manual
/// save/restore would leak a `false` flag on a panic.
pub(crate) struct CaptureTopGuard(pub(crate) bool);
impl Drop for CaptureTopGuard {
    fn drop(&mut self) {
        crate::process::set_capture_top_level(self.0);
    }
}

/// Dispatch a call whose callee and `argv` are already evaluated. A VM-eligible
/// closure of matching arity runs on the VM (or, in tail position, returns a
/// `Tail` for the trampoline); everything else (natives, multi-arm / ineligible
/// closures, arity mismatches) defers to the tree-walker via `eval::apply`.
///
/// A builtin it calls may hand its call back (ADR-505) — the record's `num/add` a `+` would
/// have called from Rust — and that call is dispatched in the builtin's place, so with
/// `tail` a VM closure comes back un-run as `Step::Tail` like any other, and the caller
/// pushes its frame.
pub(crate) fn dispatch(
    heap: &mut Heap,
    callee: Value,
    argv: SmallVec<[Value; 4]>,
    tail: bool,
    genv: EnvId,
) -> Result<Step, LispError> {
    let mut callee = callee;
    let mut argv = argv;
    loop {
        match dispatch_once(heap, callee, argv, tail, genv) {
            Err(error) if error.is_redirect() => (callee, argv) = heap.take_redirect(),
            outcome => return outcome,
        }
    }
}

fn dispatch_once(
    heap: &mut Heap,
    callee: Value,
    argv: SmallVec<[Value; 4]>,
    tail: bool,
    genv: EnvId,
) -> Result<Step, LispError> {
    // A builtin other than `apply` — the arm-less call-IC hit's usual callee — is exactly
    // `call_native`: the passthrough loop below only redirects closures, the unfolding only
    // `apply`, and the fall-through `eval::apply` of a native is `call_native` itself. Taken
    // up front, so a native call skips that walk.
    if let ValueRef::Native(id) = callee.unpack()
        && !heap.native(id).is_apply
    {
        return Ok(Step::Done(crate::eval::call_native_redirectable(
            heap, id, &argv, genv,
        )?));
    }
    let mut cur_callee = callee;
    let mut cur_argv = argv;
    // Outer `'apply` loop: mirrors the TW's `'dispatch` loop (eval.rs). Each
    // iteration runs the passthrough-redirect inner loop, then checks for `apply`
    // unfolding. On unfold, `cur_callee`/`cur_argv` are rewritten and the outer
    // loop continues so passthrough can redirect the unfolded callee (e.g.
    // `(apply + '(1 2))` unfolds to `(+ 1 2)`, then passthrough elides `+`).
    // On no-unfold, `break` falls through to the VM/TW dispatch below.
    //
    // O(1) stack: no new Rust frame per `apply` iteration — the unfolding and
    // re-dispatch all happen inside this single `dispatch` call, then `vm_apply`
    // (or a `Step::Tail` trampoline) handles the real callee.
    'apply: loop {
        // Thin-wrapper passthrough redirect (ADR-069), mirroring `eval`'s `'dispatch`
        // loop: a pure pass-through prelude op (`(< n 2)` → `<` whose 2-arg arm is
        // `(%lt n 2)`, etc.) redirects straight to its inner `%native` on remapped
        // args — so the hot loop reaches `call_native` directly instead of re-entering
        // `apply_closure` (a frame alloc + param binds + a body eval) for every
        // arithmetic/comparison op. Late-binding safe: it reads the *live* closure and
        // re-resolves the inner head each call (a symbol lookup — no GC, so `cur_argv`
        // stays valid). Looped for chained passthroughs.
        loop {
            let id = match cur_callee.unpack() {
                ValueRef::Fn(id) => id,
                _ => break,
            };
            let Some((head, map)) = crate::eval::passthrough_arm(heap, id, cur_argv.len()) else {
                break;
            };
            let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            // VM inner-head resolution: a direct `env_get` for a symbol head (no GC, so
            // `cur_argv` stays valid), else the head value itself. The shared
            // `passthrough_redirect_ok` then gates the redirect (callable inner only),
            // counts the reduction, and honours the deadline.
            let inner = match head.unpack() {
                ValueRef::Sym(s) => heap.env_get(cl_env, s),
                _ => Some(head),
            };
            let Some(inner) = inner else { break };
            // A wrapper forwarding to a contract shim of its own module forwards to the
            // original instead (the boundary, ADR-383).
            let inner =
                crate::builtins::contracts::boundary_redirect(heap, id, inner).unwrap_or(inner);
            // A redirect back to the *same* closure is direct self-recursion
            // (`(defn hog () (hog))`), not a thin wrapper: looping it here would spin
            // un-preemptibly (this redirect path has no captureable safepoint). Break
            // so it dispatches as a normal call → its VM arm, whose loop-top reduction
            // check preempts it (ADR-100 §8.1).
            if matches!(inner.unpack(), ValueRef::Fn(iid) if iid.0 == id.0) {
                break;
            }
            if !crate::eval::passthrough_redirect_ok(heap, inner)? {
                break;
            }
            let mut next: SmallVec<[Value; 4]> = SmallVec::with_capacity(map.len());
            for &i in &map {
                next.push(cur_argv[i]);
            }
            cur_callee = inner;
            cur_argv = next;
        }
        // `apply` unfolding: `(apply real arg... list)` → `(real arg... ...list)`.
        // Mirrors the TW's inline unfolding (eval.rs `while let Native … "apply"`).
        // After unfolding, `continue 'apply` re-runs passthrough on the real callee.
        // If the callee is not `apply`, or arity < 2, break and dispatch normally.
        if let ValueRef::Native(id) = cur_callee.unpack()
            && heap.native(id).is_apply
            && cur_argv.len() >= 2
        {
            let list = cur_argv
                .pop()
                .expect("cur_argv non-empty (len >= 2, checked)");
            let mut real = cur_argv.remove(0);
            // A lazy seq-view as the spliced arg list must realise first —
            // `seq_items` can't run its transducer. The realise re-enters `eval`
            // (a GC safepoint that relocates LOCAL handles), so the callee `real`
            // and the remaining leading args must be rooted across it and re-read
            // after — never trusted as pre-safepoint copies (ADR-114 re-read
            // discipline; mirrors `realize_seqviews`/`prim_eq`). Without this,
            // `(apply <local-closure> … <seq-view>)` derefs a stale closure/arg
            // handle → use-after-GC.
            let list = if matches!(list.unpack(), ValueRef::SeqView(_)) {
                heap.root_scope(|heap| -> Result<Value, LispError> {
                    let real_r = heap.root(real);
                    let arg_roots: SmallVec<[_; 4]> =
                        cur_argv.iter().map(|&v| heap.root(v)).collect();
                    let realized = crate::builtins::realize_seqview(heap, genv, list)?;
                    real = heap.read_root(real_r);
                    for (slot, &r) in cur_argv.iter_mut().zip(arg_roots.iter()) {
                        *slot = heap.read_root(r);
                    }
                    Ok(realized)
                })?
            } else {
                list
            };
            cur_argv.extend(
                heap.seq_items(list)
                    .map_err(|e| crate::eval::named_by("apply", e))?,
            );
            cur_callee = real;
            continue 'apply;
        }
        break;
    }
    // A VM-eligible closure of matching arity runs on the VM (or yields a tail
    // call for the trampoline); a native or non-passthrough/ineligible callee goes
    // to the tree-walker via `eval::apply` (which is just `call_native` for a
    // native — cheap).
    if let ValueRef::Fn(id) = cur_callee.unpack() {
        // Resolve to the arm's memoized process-local handle (never the enclosing
        // `CompiledClosure`, and never a fresh allocation) — this is the computed-head /
        // no-IC path, so it runs per call: per element of a transducer chain, per message
        // handled, per callback invoked.
        if let Some(arm) = compiled_arm_for(heap, id, cur_argv.len()) {
            // Run the callee in *its own* captured env (Stage 2c): a
            // global-capturing closure (`env == None`) resolves to the process
            // global as before, while a local-capturing one resolves its free
            // vars in the env it closed over. `genv` (the caller's env) is only
            // for natives below.
            let callee_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
            if tail {
                let bases = heap.vm_arm_block(&arm);
                return Ok(Step::Tail {
                    compiled: arm,
                    args: cur_argv,
                    genv: callee_env,
                    bases,
                });
            }
            // JIT fast path: call jit_tier directly, bypassing vm_run_bc's per-call
            // overhead (GcBlockGuard, TopLevelGuard thread-locals, BcFrame Vec alloc,
            // loop-top safepoint checks). Gated on: JIT code installed, no runtime GC
            // handles (which need live_arm_push registration), no installed inline
            // upgrade (which needs the larger inline_nslots frame vm_run_bc sets up).
            // The pre-check on `jit_code` avoids push_frame + vm_apply double-setup
            // for arms that haven't tiered yet (those fall straight through to vm_apply).
            #[cfg(feature = "jit")]
            {
                use std::sync::atomic::Ordering::Acquire;
                let code = arm.jit_code.load(Acquire);
                if !arm.has_runtime_handles
                    && !arm.inline_installed.load(Acquire)
                    && !code.is_null()
                    && code != crate::jit::BAILED
                    && code != crate::jit::QUEUED
                {
                    let env_base = heap.env_roots_len();
                    let env_root = heap.root_env(callee_env);
                    let base = heap.roots_len();
                    // Callee block installed BEFORE push_frame: an optional default is
                    // compiled in the callee's scope, so its sites index the callee's
                    // block during frame fill.
                    let saved_bases = heap.set_ic_bases(heap.vm_arm_block(&arm));
                    if let Err(e) = push_frame(heap, &arm, &cur_argv, env_root) {
                        heap.set_ic_bases(saved_bases);
                        heap.truncate_env_roots(env_base);
                        return Err(e);
                    }
                    // `jit_tier_in_frame`, not `jit_tier`: `push_frame` ALWAYS sizes to
                    // `arm.nslots`, and the `!inline_installed` gate above was read before
                    // that — but `jit_tier` re-loads `jit_code`, and a peer process sharing
                    // this `CompiledArm` (ADR-215) can swap the inlined body in inside that
                    // window. Telling it the size this frame was built to lets it decline
                    // rather than raw-write past the frame top. See its doc (KI-48 family).
                    // Destination for a Done result: this entry hands one back, so it owns
                    // a stack local (see `crate::jit::JitArmFn`).
                    let mut jit_ret = Value::Nil;
                    let jit_outcome = jit_tier_in_frame(
                        arm.arc(),
                        heap,
                        base,
                        env_root,
                        arm.nslots,
                        &mut jit_ret as *mut Value,
                        false,
                    );
                    heap.set_ic_bases(saved_bases);
                    // The same booking the driver's frame path does (counters, the sysmon
                    // deopt event, `BROOD_DEOPT_TRACE`), so a deopt taken here is seen too.
                    book_native_outcome(heap, &arm, base, jit_outcome);
                    match jit_outcome {
                        Some(outcome::DONE) => {
                            crate::perf_bump!(jit_apply_fast);
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(jit_ret));
                        }
                        Some(outcome::ERROR) => {
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            let e = heap
                                .jit_pending_error
                                .take()
                                .expect("JIT error outcome with no parked error");
                            return Err(e);
                        }
                        Some(outcome::TAIL) => {
                            crate::perf_bump!(jit_fast_tail4);
                            // Follow the staged call directly instead of re-running this arm
                            // on the VM. The staged handles were pushed *after* any GC inside
                            // the native's safepoints, so they are current-epoch — unlike
                            // `cur_argv`. The frame here is `push_frame`'s, which ALWAYS sizes
                            // to `arm.nslots` (KI-48: never re-derive it).
                            let local = heap.take_jit_tail_local_head();
                            let (callee, args) = read_staged_tail(heap, &arm, base, arm.nslots);
                            let callee_env2 = heap.read_root_env(env_root);
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(
                                apply_value(heap, callee, &args, callee_env2).map_err(|e| {
                                    crate::eval::name_uncallable_local(heap, e, callee, local)
                                })?,
                            ));
                        }
                        _ => {
                            crate::perf_bump!(jit_fast_deopt);
                            // Epoch reset (→ None), deopt (1), or preempt (2): re-run
                            // on the VM, with arguments re-read from the frame
                            // (`fresh_frame_args`).
                            let callee_env2 = heap.read_root_env(env_root);
                            // Deopt-resume (see `CompiledArm::ckpt_slot`): a deopt after
                            // a completed non-tail call keeps the frame and resumes AT
                            // the checkpoint — never re-running its side effects.
                            // This path is gated on `!inline_installed` above, so the
                            // resume arm is always `arm` itself here.
                            // Deopt AND preempt, consistent with the other three consumers —
                            // see `jit_run_fast_link` (KI-18).
                            if matches!(jit_outcome, Some(outcome::DEOPT | outcome::PREEMPT))
                                && let Some((resume, rip, depth)) =
                                    jit_ckpt_resume(heap, arm.arc(), base, arm.nslots)
                            {
                                heap.truncate_env_roots(env_base);
                                return Ok(Step::Done(vm_resume_deopt(
                                    heap,
                                    resume,
                                    base,
                                    callee_env2,
                                    rip,
                                    depth,
                                )?));
                            }
                            let fresh_argv = fresh_frame_args(heap, &arm, base, cur_argv);
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(vm_apply(heap, arm, &fresh_argv, callee_env2)?));
                        }
                    }
                }
            }
            return Ok(Step::Done(vm_apply(heap, arm, &cur_argv, callee_env)?));
        }
        // A closure with no VM-eligible arm for this argc — a true defer to the
        // tree-walker. Native frames created by the tree-walker can't be captured
        // by the state-capture machinery; gate off so any `receive` inside blocks
        // the worker (§7.4 dirty-scheduler carve-out) instead of attempting a
        // state-capture that can't cross the native boundary.
        crate::perf_bump!(tw_defer);
        // Name each deferring closure under BROOD_DEFER_DBG=1. The `tw_defer` counter
        // says how MANY calls fell to the tree-walker, never WHO. Cold path by
        // definition; one cached bool when off.
        {
            if crate::diagnostics::debug_flags::defer_debug() {
                let name = match cur_callee.unpack() {
                    crate::core::value::ValueRef::Fn(id) => heap
                        .closure(id)
                        .name
                        .map(crate::core::value::symbol_name_ref)
                        .unwrap_or("<anon>"),
                    _ => "<non-fn>",
                };
                eprintln!("[tw-defer] {name} argc={}", cur_argv.len());
            }
        }
        let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
        let result = crate::eval::apply(heap, cur_callee, &cur_argv, genv);
        return Ok(Step::Done(result?));
    }
    // The builtin a wrapper forwarded to (`+` → `%add`): `eval::apply` of a native is
    // `call_native`, made here with the redirect offered.
    if let ValueRef::Native(id) = cur_callee.unpack() {
        return Ok(Step::Done(crate::eval::call_native_redirectable(
            heap, id, &cur_argv, genv,
        )?));
    }
    Ok(Step::Done(crate::eval::apply(
        heap, cur_callee, &cur_argv, genv,
    )?))
}

/// DEBUG ONLY: assert every value in `args` has a valid `Value` discriminant. A value
/// with an out-of-range tag byte is non-`Value` memory — a JIT frame-slot corruption
/// (the bug #2 family). Aborting here, at the earliest frame entry that sees it, makes
/// the backtrace point at the call chain that produced the garbage.
#[cfg(debug_assertions)]
pub(crate) fn dbg_check_args(args: &[Value], label: &str) {
    for (i, a) in args.iter().enumerate() {
        let tag = a.raw_tag();
        // Anything above the last discriminant is garbage. The bound lives beside the
        // enum (`MAX_VALUE_DISCRIMINANT`) rather than as a literal here: as a literal it
        // silently went stale when ADR-310 appended `Value::Failure`, and every valid
        // failure value then read as corrupt memory.
        if tag > crate::core::value::MAX_VALUE_DISCRIMINANT {
            panic!(
                "[arg-origin] {label}: arg[{i}] has invalid Value tag {tag:#x} — corrupt (non-Value) memory passed into a frame"
            );
        }
    }
}

/// Fill `arm`'s capture slots (`capture_base + k`) of the frame at `base` from the captured
/// env `cenv` — [`push_frame`]'s step for them, shared with every frame built without it:
/// the driver's in-place adoption (`ChunkExit::CallInPlace`) and the native entries'
/// fast frames. Without it a callee reads its captured lexicals as nil and fails far away.
/// No allocation, so no collection. The emptiness test is split from the fill so it inlines:
/// with the loop in one body it stayed a call, on every frame of an arm with no captures.
#[inline]
pub(crate) fn fill_captures(heap: &mut Heap, arm: &CompiledArm, base: usize, cenv: EnvId) {
    if !arm.capture_names.is_empty() {
        fill_captures_from(heap, arm, base, cenv);
    }
}

#[inline(never)]
fn fill_captures_from(heap: &mut Heap, arm: &CompiledArm, base: usize, cenv: EnvId) {
    {
        let capture_base = arm.nrequired + arm.noptional + arm.rest_slot.is_some() as usize;
        for (k, &name) in arm.capture_names.iter().enumerate() {
            let v = heap.capture_value(cenv, k, name);
            heap.set_root_at(base + capture_base + k, v);
        }
    }
}

/// The arm a call site caches for callee `v` called with `argc` arguments: a closure the
/// VM runs directly — not a pass-through wrapper, which `dispatch` redirects (ADR-069) —
/// with a compiled arm for that arity, plus the env it closed over. Everything else caches
/// just the value (still skipping the lookup walk). Native code's dispatch deliberately
/// caches a wrapper's own arm instead (ADR-408), so it does not use this.
pub(super) fn call_site_arm(heap: &Heap, v: Value, argc: usize) -> Option<(Arc<ArmHandle>, EnvId)> {
    match v.unpack() {
        ValueRef::Fn(id) if crate::eval::passthrough_arm(heap, id, argc).is_none() => {
            compiled_arm_for(heap, id, argc).map(|arm| {
                let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
                (arm, cenv)
            })
        }
        _ => None,
    }
}

/// The arguments to re-run `arm` with on the VM after its native code bailed. A collection
/// can fire at any safepoint inside the native call — even on a deopt (a sub-call returns
/// a non-int, collects, then the arm deopts on it) — leaving `cur_argv`, captured before
/// the call, holding stale LOCAL handles. The frame at `roots[base..]` is updated in place
/// by every collection, so the arguments are re-read from there.
///
/// A rest arm folded its rest elements into a list at bind time, so they can't be rebuilt
/// per slot; it falls back to `cur_argv`. That is sound only because a rest arm never
/// reaches here after a real safepoint (it is outside the JIT int-subset). A stale handle
/// would be the bug #2 class (ADR-114) and corrupt, so debug builds enforce the invariant
/// rather than leave it a silent assumption.
#[cfg(feature = "jit")]
fn fresh_frame_args(
    heap: &Heap,
    arm: &CompiledArm,
    base: usize,
    cur_argv: SmallVec<[Value; 4]>,
) -> SmallVec<[Value; 4]> {
    if arm.rest_slot.is_none() {
        return (0..cur_argv.len())
            .map(|k| heap.root_at(base + k))
            .collect();
    }
    #[cfg(debug_assertions)]
    for v in &cur_argv {
        debug_assert!(
            heap.dbg_value_stale(*v).is_none(),
            "dispatch: stale LOCAL handle in the rest-arm cur_argv fallback after a JIT \
             safepoint — the 'rest arms never JIT post-safepoint' invariant broke (ADR-114; \
             re-read from roots instead)"
        );
    }
    cur_argv
}

/// Push a fresh activation frame for `arm` onto `Heap::roots`: required args, then
/// `&optional` slots (the provided arg, or nil if missing), then the `&` rest list
/// (the trailing args conased into a fresh list), then nil for the `let`/`letrec`
/// binders — `nslots` values total. Selection guarantees `args.len() >= nrequired`.
/// No eval runs here (the rest is a plain `list_from_slice`), so no collection can
/// happen between reading `args` and rooting the slots.
pub(crate) fn push_frame(
    heap: &mut Heap,
    arm: &CompiledArm,
    args: &[Value],
    genv: EnvRoot,
) -> Result<(), LispError> {
    // DEBUG ONLY: catch a corrupt argument at the EARLIEST frame entry — the first call
    // that receives an invalid-tag `Value` is closest to the origin of the JIT GC bug.
    // Abort so the backtrace shows the call chain that produced it.
    #[cfg(debug_assertions)]
    dbg_check_args(args, "push_frame");

    let base = heap.roots_len();
    // Pre-allocate the whole frame as nil: every slot (params, optionals, rest, and
    // the body's `let`/`letrec` binders) must exist before anything writes it via
    // `set_root_at` — including a real `&optional` default whose body may bind its
    // own `let` slots. One `resize` rather than a per-slot push loop (call hot path).
    heap.extend_roots_to_nil(base + arm.nslots);
    // Consume ALL provided args into their (now-rooted) slots FIRST, before any
    // default is evaluated: a default's eval can collect, which would strand the
    // still-live `args` slice (LOCAL handles) if it were read afterwards.
    for i in 0..arm.nrequired {
        heap.set_root_at(base + i, args.get(i).copied().unwrap_or(Value::nil()));
    }
    // Provided optionals are a left-to-right prefix of `args`; the remainder are
    // missing and take their defaults below.
    let provided_opt = args.len().saturating_sub(arm.nrequired).min(arm.noptional);
    for j in 0..provided_opt {
        heap.set_root_at(base + arm.nrequired + j, args[arm.nrequired + j]);
    }
    if let Some(rslot) = arm.rest_slot {
        let start = (arm.nrequired + arm.noptional).min(args.len());
        let rest = heap.list_from_slice(&args[start..]);
        heap.set_root_at(base + rslot, rest);
    }
    // #3 lexical addressing: fill the capture slots from the closure's captured env, so
    // the body reads captured lexicals as fast `Node::Local` slots rather than `env_get`
    // symbol-scans. Each `capture_names[k]` occupies slot `capture_base + k`. `capture_value`
    // takes an index fast-path when the captured env is a flat frame (`vars[k]` is that name
    // — the VM-built common case) and falls back to a by-name `env_get` for a chained /
    // tree-walker env, so it's correct in both engines. Filled before optional defaults so a
    // default form may reference a capture. No GC between here and the body (no alloc).
    fill_captures(heap, arm, base, heap.read_root_env(genv));
    // Mark each missing optional that has a real default: the arm's prologue evaluates
    // that default into its slot (`compile_arm`), in the arm's own bytecode. A nil default
    // has no flag — its slot is already nil.
    for j in provided_opt..arm.noptional {
        let flag = arm.optional_flags[j];
        if flag != u32::MAX {
            heap.set_root_at(base + flag as usize, Value::boolean(true));
        }
    }
    Ok(())
}

/// Run a chunked closure arm and its whole chain of chunked calls on the explicit
/// bytecode frame stack ([`vm_run_bc`]) — the sole VM executor (ADR-100 Stage 5). Every
/// `CompiledArm` from `compile_arm` has a chunk, so this always routes to the driver;
/// `vm_run_bc` does the per-frame live-arm registration + the runaway frame guard.
/// Callers: `dispatch` (non-tail VM-closure branch), `force` (a tail `Step`), and a
/// top-level form (`run`). The tree-walker (`BROOD_TIER=0`) is the remaining fallback.
pub(crate) fn vm_apply(
    heap: &mut Heap,
    compiled0: Arc<ArmHandle>,
    args: &[Value],
    genv0: EnvId,
) -> LispResult {
    // `top_level = false`: this is a nested run (the process-body driver is
    // `run_process_body`), so it does no loop-top preempt/kill capture — only the
    // body driver does. A `receive` suspend that surfaces here re-raises (§8.1).
    //
    // Native-stack guard. This is the single chokepoint every *nested* activation goes
    // through, and a nested run is the one VM shape that consumes real Rust stack per
    // Brood level: an ordinary call pushes a heap frame inside one `vm_run_bc` loop
    // (capped by `MAX_BC_FRAMES`), but a native callback (a `try` body, a `map`
    // function, a top-level form run from a native) re-enters here. Nothing on that path
    // reaches `eval`, so the tree-walker's byte guard never saw it and a recursive `try`
    // body died on the guard page — a SIGSEGV `try`/`catch` cannot observe and no
    // supervisor can restart (the OS process goes, not the green process). Fail here
    // instead, with the same catchable error the tree-walker raises.
    if !crate::process::native_stack_headroom_ok() {
        let left = stacker::remaining_stack().unwrap_or(0);
        return Err(crate::eval::native_stack_error(left));
    }
    // IC cursors (ADR-175 Phase A): `vm_run_bc`'s fresh entry installs the callee's
    // block; the CALLER (whose arm continues after this nested run) needs its own
    // block back, so save/restore around the run — the single chokepoint every
    // nested activation goes through.
    let saved_bases = heap.ic_bases();
    let out = vm_run_bc(heap, compiled0, args, genv0, None, false);
    heap.set_ic_bases(saved_bases);
    match out? {
        VmOutcome::Done(v) => Ok(v),
        // A `receive` suspended inside this VM run — but this run is **nested** under a
        // native (a `map`/`try`/`binding`/`%isolate` callback that re-entered the VM via
        // `dispatch`/`apply_value`), so its continuation can't be returned as a value.
        // This is the native-nested case (ADR-100 §8.1): discard the captured inner
        // frames (their roots were left intact for a top-level resume — unwind them to
        // the entry mark) and re-raise the `Control::Suspend` so the enclosing native
        // re-raises it untouched. The *outer* `vm_run_bc` then re-runs this native's
        // `Inst::Call` on resume — correct because the only shape that occurs has no
        // irreversible side effect before the `receive`. (A native-nested receive that
        // *would* repeat a side effect is gated off this path by `capture_top_level()`
        // and blocks its worker instead — the §7.4 dirty carve-out.)
        VmOutcome::Suspended(s) => {
            let s = s.into_vm();
            let deadline = s.deadline;
            heap.truncate_roots(s.entry_roots);
            heap.truncate_env_roots(s.entry_env);
            heap.live_arm_truncate(s.entry_arms);
            Err(LispError::suspend(deadline))
        }
        // `top_level = false` ⇒ no loop-top capture, so a nested run never preempts or
        // self-kills; these are produced only by the body driver (`run_process_body`).
        VmOutcome::Preempted(_) | VmOutcome::Killed => {
            unreachable!("a nested vm_apply run does no loop-top preempt/kill capture")
        }
    }
}
