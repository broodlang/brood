//! VM arm dispatcher: Call/SelfCall/IC/JIT fast path (extracted from mod.rs).
use super::*;

/// The combination executor for the surviving `Node` path ([`exec_value`] — used by
/// `push_frame`'s `&optional` defaults and top-level `run`). Resolves the callee
/// through the call-site IC, evaluates the arguments onto the operand stack, and
/// dispatches; the returned [`Step`] is forced in value position. (The bytecode
/// engine uses its own `Inst::Call` path in [`exec_chunk`].)
#[allow(clippy::too_many_arguments)]
pub(crate) fn exec_call(
    heap: &mut Heap,
    callee: &Node,
    args: &[Node],
    tail: bool,
    pos: Option<Pos>,
    file: Option<&str>,
    site: u32,
    frame_base: usize,
    genv: EnvRoot,
) -> Result<Step, LispError> {
    // Tag an error with this combination's source position (and file when known)
    // if it doesn't already carry one — so the *innermost* failing call wins
    // (mirrors the tree-walker's `or_form_pos`); a sub-call that already tagged
    // itself is left untouched.
    let tag = |e: LispError| match pos {
        Some(p) => match file {
            Some(f) => e.or_pos(p).or_file(f),
            None => e.or_pos(p),
        },
        None => e,
    };
    // Resolve the callee — through this site's inline cache when it has one
    // (ADR-096). A hit skips the `env_get` walk entirely and may carry the
    // VM fast path (the callee's compiled arm + captured env); a miss
    // resolves normally and installs the entry, stamped with `probe_epoch`
    // (read *before* the resolution, so an arg-eval `def` below can't be
    // attributed to this resolution). Engages only when the body's free
    // names resolve through the process global (`is_global`): a
    // local-capturing closure's captured frames could shadow the symbol,
    // and they differ per closure *instance* while the site is shared.
    let probe_epoch = heap.global_epoch();
    let mut fast: Option<(Arc<CompiledArm>, EnvId, (u32, u32))> = None;
    let cv: Value;
    'resolve: {
        if site != NO_SITE {
            if let Node::Global(sym) = callee {
                if heap.is_global(heap.read_root_env(genv)) {
                    let argc = args.len() as u32;
                    if let Some((v, payload)) = heap.vm_call_ic_probe(site, *sym, argc, probe_epoch)
                    {
                        crate::perf_bump!(call_ic_hit);
                        cv = v;
                        fast = payload;
                        break 'resolve;
                    }
                    crate::perf_bump!(call_ic_miss);
                    // Miss: resolve (exactly what `exec_value` on the callee
                    // would do), then install. A *dynamic* symbol is never
                    // cached — a `binding` re-binds it without bumping the
                    // epoch, so a cached resolution would bypass it. (A
                    // later `defdyn` of a cached symbol bumps the epoch, so
                    // the entry self-invalidates and the re-install refuses.)
                    let env = heap.read_root_env(genv);
                    let v = match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(tag(crate::eval::unbound_error(heap, *sym))),
                    };
                    if !value::is_dynamic(*sym) {
                        let arm = match v.unpack() {
                            // Cache the VM fast path only for a callee
                            // `dispatch` would run on the VM directly: a
                            // non-passthrough closure with a compiled arm
                            // for this argc. Everything else caches just
                            // the value (still skips the lookup walk).
                            ValueRef::Fn(id)
                                if crate::eval::passthrough_arm(heap, id, args.len()).is_none() =>
                            {
                                compiled_arm_for(heap, id, args.len()).map(|arm| {
                                    let cenv =
                                        heap.closure(id).env.unwrap_or_else(|| heap.global());
                                    (arm, cenv)
                                })
                            }
                            _ => None,
                        };
                        fast = arm
                            .as_ref()
                            .map(|(a, cenv)| (a.clone(), *cenv, heap.vm_arm_block(a)));
                        // Mirror vm_call_ic_put's LOCAL-env guard: arg eval below can
                        // trigger a LOCAL minor collect that moves the env without
                        // bumping the global epoch, so the epoch check at the fast-path
                        // use site wouldn't catch a stale LOCAL cenv.  Clear fast now.
                        if let Some((_, cenv, _)) = &fast {
                            if *cenv != EnvId::GLOBAL && cenv.region() == value::LOCAL {
                                fast = None;
                            }
                        }
                        heap.vm_call_ic_put(
                            site,
                            crate::core::heap::CallIcEntry {
                                sym: *sym,
                                argc,
                                epoch: probe_epoch,
                                callee: v,
                                arm,
                                // Overwritten inside `vm_call_ic_put`.
                                callee_bases: (0, 0),
                            },
                        );
                    }
                    cv = v;
                    break 'resolve;
                }
            }
        }
        // No IC for this site/shape: evaluate the callee node as before.
        cv = exec_value(heap, callee, frame_base, genv).map_err(tag)?;
    }
    // Evaluate each argument, keeping the callee + results on the operand
    // stack so a collection during a later argument's eval relocates them in
    // place (mirrors `eval::eval_arguments`). `save` is this call's region;
    // it is always truncated back, including on the error path.
    let save = heap.roots_len();
    heap.push_root(cv);
    for a in args.iter() {
        match exec_value(heap, a, frame_base, genv) {
            Ok(v) => heap.push_root(v),
            Err(e) => {
                heap.truncate_roots(save);
                return Err(tag(e));
            }
        }
    }
    // Re-read post-collection from the (relocated) operand stack.
    let callee_v = heap.root_at(save);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(args.len());
    for k in 0..args.len() {
        argv.push(heap.root_at(save + 1 + k));
    }
    // The IC fast path: run the cached compiled arm directly, skipping
    // `dispatch`'s passthrough probe + body-cache lookup + env read —
    // but only if the global epoch is *still* `probe_epoch`. An arg's
    // eval can `def` (new resolution next call — but THIS call correctly
    // uses the pre-args callee, which is `callee_v`, rooted) or fire a
    // RUNTIME compaction (which rewrites the rooted `callee_v` in place
    // but NOT the un-registered `fast` arm's node tree or its env
    // handle) — either bumps the epoch, so the stale fast path is
    // dropped and the rooted callee takes the generic path below.
    if let Some((arm, cenv, bases)) = fast {
        if heap.global_epoch() == probe_epoch {
            let result = if tail {
                Ok(Step::Tail {
                    compiled: arm,
                    args: argv,
                    genv: cenv,
                    bases,
                })
            } else {
                vm_apply(heap, arm, &argv, cenv).map(Step::Done)
            };
            heap.truncate_roots(save);
            return result.map_err(tag);
        }
    }
    // The *current* env (read fresh post-collection) is what a native callee
    // runs in; a VM-eligible closure callee instead runs in its own captured
    // env, which `dispatch` reads off the closure.
    let cur_env = heap.read_root_env(genv);
    let result = dispatch(heap, callee_v, argv, tail, cur_env);
    heap.truncate_roots(save);
    result.map_err(tag)
}

/// Restores the `capture_top_level` flag on drop — so the gate is reset even if the
/// guarded tree-walker `apply` *panics* (caught by `run_one`'s `catch_unwind`). The
/// manual save/restore it replaces leaked a `false` flag on a panic until the next
/// `vm_run_bc` entry re-stamped it.
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
pub(crate) fn dispatch(
    heap: &mut Heap,
    callee: Value,
    argv: SmallVec<[Value; 4]>,
    tail: bool,
    genv: EnvId,
) -> Result<Step, LispError> {
    let mut cur_callee = callee;
    let mut cur_argv = argv;
    // Outer `'apply` loop: mirrors the TW's `'dispatch` loop (eval/mod.rs). Each
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
            // A redirect back to the *same* closure is direct self-recursion
            // (`(defn hog () (hog))`), not a thin wrapper: looping it here would spin
            // un-preemptibly (this redirect path has no captureable safepoint). Break
            // so it dispatches as a normal call → its VM arm, whose loop-top reduction
            // check preempts it (ADR-100 §8.1).
            if matches!(inner.unpack(), ValueRef::Fn(iid) if iid.0 == id.0) {
                break;
            }
            if !crate::eval::passthrough_redirect_ok(inner)? {
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
        // Mirrors the TW's inline unfolding (eval/mod.rs `while let Native … "apply"`).
        // After unfolding, `continue 'apply` re-runs passthrough on the real callee.
        // If the callee is not `apply`, or arity < 2, break and dispatch normally.
        if let ValueRef::Native(id) = cur_callee.unpack() {
            if heap.native(id).name == "apply" && cur_argv.len() >= 2 {
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
                cur_argv.extend(heap.seq_items(list)?);
                cur_callee = real;
                continue 'apply;
            }
        }
        break;
    }
    // A VM-eligible closure of matching arity runs on the VM (or yields a tail
    // call for the trampoline); a native or non-passthrough/ineligible callee goes
    // to the tree-walker via `eval::apply` (which is just `call_native` for a
    // native — cheap).
    if let ValueRef::Fn(id) = cur_callee.unpack() {
        // Resolve the arm cloning only the `Arc<CompiledArm>` (not the enclosing
        // `CompiledClosure`) — one fewer Arc clone per call on the hot path.
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
                    let jit_outcome = jit_tier(&arm, heap, base, env_root);
                    heap.set_ic_bases(saved_bases);
                    match jit_outcome {
                        Some(0) => {
                            crate::perf_bump!(jit_apply_fast);
                            let v = heap.root_at(base);
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(v));
                        }
                        Some(3) => {
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            let e = heap
                                .jit_pending_error
                                .take()
                                .expect("JIT outcome 3 with no parked error");
                            return Err(e);
                        }
                        Some(4) => {
                            crate::perf_bump!(jit_fast_tail4);
                            // Tail call staged by the JIT: [callee, arg0..argN] sit
                            // above the frame at roots[base+active_nslots..].  These
                            // were pushed *after* any GC that fired inside
                            // jit_dispatch_call's safepoint (line ~8890), so they hold
                            // current-epoch handles — unlike cur_argv which was captured
                            // before jit_tier ran and may be stale.  Follow the staged
                            // call directly instead of re-running this arm on the VM.
                            let frame_top = base + arm.active_nslots();
                            let n = heap.roots_len();
                            let callee_env2 = heap.read_root_env(env_root);
                            if n > frame_top {
                                let staged_callee = heap.root_at(frame_top);
                                let staged_argc = n - frame_top - 1;
                                let staged_argv: SmallVec<[Value; 4]> = (0..staged_argc)
                                    .map(|k| heap.root_at(frame_top + 1 + k))
                                    .collect();
                                heap.truncate_roots(base);
                                heap.truncate_env_roots(env_base);
                                return Ok(Step::Done(apply_value(
                                    heap,
                                    staged_callee,
                                    &staged_argv,
                                    callee_env2,
                                )?));
                            }
                            // Staged area is empty (shouldn't happen for outcome 4,
                            // but fall back gracefully).  GC may have run, so read
                            // fresh args from the frame rather than stale cur_argv.
                            let argc = cur_argv.len();
                            let fresh_argv: SmallVec<[Value; 4]> = if arm.rest_slot.is_none() {
                                (0..argc).map(|k| heap.root_at(base + k)).collect()
                            } else {
                                // Rest arm: the rest elements were folded into a list at
                                // bind time, so they can't be reconstructed per-slot from
                                // `roots`; fall back to the pre-call `cur_argv`. Sound ONLY
                                // if no GC relocated those handles since capture — i.e. a
                                // rest arm never reaches here after a real safepoint (it's
                                // outside the JIT int-subset). A stale handle here is the
                                // bug #2 class (ADR-114) and would corrupt, so enforce the
                                // invariant in debug rather than leaving it as a silent
                                // assumption.
                                #[cfg(debug_assertions)]
                                for v in &cur_argv {
                                    debug_assert!(
                                        heap.dbg_value_stale(*v).is_none(),
                                        "dispatch: stale LOCAL handle in the rest-arm \
                                             cur_argv fallback after a JIT safepoint — the \
                                             'rest arms never JIT post-safepoint' invariant \
                                             broke (ADR-114; re-read from roots instead)"
                                    );
                                }
                                cur_argv
                            };
                            heap.truncate_roots(base);
                            heap.truncate_env_roots(env_base);
                            return Ok(Step::Done(vm_apply(heap, arm, &fresh_argv, callee_env2)?));
                        }
                        _ => {
                            crate::perf_bump!(jit_fast_deopt);
                            // Epoch reset (→ None), deopt (1), or preempt (2): re-run
                            // on the VM.  GC can fire during any jit_dispatch_call
                            // safepoint (line ~8903) triggered by a sub-call inside
                            // jit_tier — even for deopt (the sub-call returns a
                            // non-int, GC fires in its safepoint, then the JIT deopts
                            // on the non-int result).  After that GC, cur_argv holds
                            // stale LOCAL handles.  The frame at roots[base..base+nslots]
                            // is updated in place by every GC, so read fresh args from
                            // there.  Arms with a rest slot collect the rest elements
                            // into a list; they're unreachable in the JIT int-subset
                            // and fall through to cur_argv as an inert dead-code path.
                            let callee_env2 = heap.read_root_env(env_root);
                            // Deopt-resume (see `CompiledArm::ckpt_slot`): a deopt after
                            // a completed non-tail call keeps the frame and resumes AT
                            // the checkpoint — never re-running its side effects.
                            if matches!(jit_outcome, Some(1)) {
                                if let Some((rip, depth)) = jit_ckpt_read(heap, &arm, base) {
                                    heap.truncate_env_roots(env_base);
                                    return Ok(Step::Done(vm_resume_deopt(
                                        heap,
                                        arm,
                                        base,
                                        callee_env2,
                                        rip,
                                        depth,
                                    )?));
                                }
                            }
                            let argc = cur_argv.len();
                            let fresh_argv: SmallVec<[Value; 4]> = if arm.rest_slot.is_none() {
                                (0..argc).map(|k| heap.root_at(base + k)).collect()
                            } else {
                                // Rest arm: the rest elements were folded into a list at
                                // bind time, so they can't be reconstructed per-slot from
                                // `roots`; fall back to the pre-call `cur_argv`. Sound ONLY
                                // if no GC relocated those handles since capture — i.e. a
                                // rest arm never reaches here after a real safepoint (it's
                                // outside the JIT int-subset). A stale handle here is the
                                // bug #2 class (ADR-114) and would corrupt, so enforce the
                                // invariant in debug rather than leaving it as a silent
                                // assumption.
                                #[cfg(debug_assertions)]
                                for v in &cur_argv {
                                    debug_assert!(
                                        heap.dbg_value_stale(*v).is_none(),
                                        "dispatch: stale LOCAL handle in the rest-arm \
                                             cur_argv fallback after a JIT safepoint — the \
                                             'rest arms never JIT post-safepoint' invariant \
                                             broke (ADR-114; re-read from roots instead)"
                                    );
                                }
                                cur_argv
                            };
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
        let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
        let result = crate::eval::apply(heap, cur_callee, &cur_argv, genv);
        return Ok(Step::Done(result?));
    }
    Ok(Step::Done(crate::eval::apply(
        heap, cur_callee, &cur_argv, genv,
    )?))
}

/// Push a fresh activation frame for `arm` onto `Heap::roots`: required args, then
/// `&optional` slots (the provided arg, or nil if missing), then the `&` rest list
/// (the trailing args conased into a fresh list), then nil for the `let`/`letrec`
/// binders — `nslots` values total. Selection guarantees `args.len() >= nrequired`.
/// No eval runs here (the rest is a plain `list_from_slice`), so no collection can
/// happen between reading `args` and rooting the slots.
/// DEBUG ONLY: assert every value in `args` has a valid `Value` discriminant. A value
/// with an out-of-range tag byte is non-`Value` memory — a JIT frame-slot corruption
/// (the bug #2 family). Aborting here, at the earliest frame entry that sees it, makes
/// the backtrace point at the call chain that produced the garbage.
#[cfg(debug_assertions)]
pub(crate) fn dbg_check_args(args: &[Value], label: &str) {
    for (i, a) in args.iter().enumerate() {
        let tag = (unsafe { std::mem::transmute::<Value, [i64; 3]>(*a) }[0] as u64 & 0xff) as u8;
        // Value has ~22 variants (max discriminant ~21); well above that is garbage.
        if tag > 24 {
            panic!("[arg-origin] {label}: arg[{i}] has invalid Value tag {tag:#x} — corrupt (non-Value) memory passed into a frame");
        }
    }
}

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
    if !arm.capture_names.is_empty() {
        let cenv = heap.read_root_env(genv);
        let capture_base = arm.nrequired + arm.noptional + arm.rest_slot.is_some() as usize;
        for (k, &name) in arm.capture_names.iter().enumerate() {
            let v = heap.capture_value(cenv, k, name);
            heap.set_root_at(base + capture_base + k, v);
        }
    }
    // Missing optionals take their default, left-to-right (so a later default sees an
    // earlier one). `None` is a nil-default — the slot is already nil. A real default
    // evaluates against the frame: earlier params/optionals are filled and rooted;
    // its own slot and later slots are still nil (the compiler bound it after the
    // default, so the default can't name itself).
    for j in provided_opt..arm.noptional {
        if let Some(node) = &arm.optional_defaults[j] {
            let v = exec_value(heap, node, base, genv)?;
            heap.set_root_at(base + arm.nrequired + j, v);
        }
    }
    Ok(())
}

/// Run a chunked closure arm and its whole chain of chunked calls on the explicit
/// bytecode frame stack ([`vm_run_bc`]) — the sole VM executor since ADR-100 Stage 5
/// (the `Node`-walking trampoline was retired with the bytecode default). Every
/// `CompiledArm` from `compile_arm` has a chunk, so this always routes to the driver;
/// `vm_run_bc` does the per-frame live-arm registration + the runaway frame guard.
/// Callers: `dispatch` (non-tail VM-closure branch), `exec_call`'s IC fast path, and
/// `force` (a tail `Step`). The tree-walker (`BROOD_VM=0`) is the remaining fallback.
pub(crate) fn vm_apply(
    heap: &mut Heap,
    compiled0: Arc<CompiledArm>,
    args: &[Value],
    genv0: EnvId,
) -> LispResult {
    // `top_level = false`: this is a nested run (the process-body driver is
    // `run_process_body`), so it does no loop-top preempt/kill capture — only the
    // body driver does. A `receive` suspend that surfaces here re-raises (§8.1).
    //
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
