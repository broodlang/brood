//! The Brood→Brood call and tail-call paths out of native code: link if the callee is
//! native and linkable, else the slow path through the VM.

use super::*;

/// Run a JIT'd arm's **non-tail** Brood→Brood call. The `argc` args are the top operands
/// on `roots`. A **free-global** head (`site != NO_SITE`) is *not* staged — the callee is
/// resolved here via the call-site IC (`head` + `site`), so the args occupy `[n-argc, n)`
/// and the frame starts at `n-argc`. A **computed** head leaves the callee staged below the
/// args (`[n-argc-1]`). The fast path links straight to the callee's native code; otherwise
/// [`dispatch`] runs it (`tail = false` ⇒ to completion) as a **nested** (non-top-level)
/// run, so it never preempts/suspends across the native boundary (the §7.4 carve-out).
#[cfg(feature = "jit")]
pub(crate) fn jit_dispatch_call(
    heap: &mut Heap,
    argc: usize,
    site: u32,
    head: Symbol,
) -> Option<Value> {
    use std::sync::atomic::Ordering::Acquire;
    let n = heap.roots_len();
    // KI-14: probe the stack **unconditionally** here, not `jit_native_headroom_ok(depth)`.
    // That helper skips the probe below depth 64 as a hot-path optimisation, which is sound
    // only where `jit_native_depth` actually counts the recursion. On this path it does not:
    // a JIT'd arm recursing through `brood_rt_call_slow` → `jit_dispatch_call` re-enters
    // Rust every level, yet the depth stays near zero, so the probe was never reached and
    // neither cap ever fired — 100 000 levels of JSON nesting piled a JIT frame plus these
    // Rust frames each, and the worker died on its guard page (an abort `try`/`catch` can't
    // see, taking the OS process rather than the green one).
    //
    // Probing every slow call costs a thread-local read; the slow call already does far
    // more work than that, and it is the only place that can see this recursion coming.
    let over_cap = heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT || !stack_headroom_ok();
    let epoch = heap.global_epoch();
    // A free-global head isn't staged (`elided`): the callee is resolved via the call IC.
    // `stage_base` is where the callee frame starts — directly at the args for an elided
    // head, one slot lower (over the staged callee) for a computed one.
    let elided = site != NO_SITE;
    let stage_base = if elided { n - argc } else { n - argc - 1 };

    #[cfg(debug_assertions)]
    {
        for k in stage_base..n {
            let v = heap.root_at(k);
            if let Some((kind, g, e)) = heap.dbg_value_stale(v) {
                let raw = unsafe { std::mem::transmute::<Value, [i64; 3]>(v) };
                eprintln!(
                    "[jit-staged-stale] STALE {kind} (gen {g} != live {e}) staged at roots[{k}] \
                     BY arm '{}' for call to '{}' at {} (site={site}, argc={argc}); raw=[{:#x},{:#x},{:#x}]",
                    crate::core::value::symbol_name_opt(heap.jit_dbg_fn).unwrap_or("<unknown>"),
                    crate::core::value::symbol_name_opt(head).unwrap_or("<computed>"),
                    heap.dbg_site_loc(site),
                    raw[0], raw[1], raw[2],
                );
            }
        }
    }
    // Runtime BROOD_JIT_VERIFY: same scan in a plain --release build.
    if jit_verify_active() {
        jit_verify_staged(heap, stage_base, n, head, site, argc);
    }

    // ---- Fast native link (no per-call Arc clone) ----
    // The hot recursive case (`fib`, a free-global head). `vm_call_ic_fast_link` validates
    // the whole link (sym/argc/epoch + installed + simple arm) and returns Copy data — no
    // `Arc::clone` (the one atomic-RMW per call the older cloning path below pays ~30M
    // times). Args are already staged at `[stage_base, stage_base+argc)`. Mirrors the
    // cloning path's frame setup + outcome handling; deopt (rare) re-probes for the arm.
    if elided && !over_cap {
        if let Some((code, nslots, callee_env, callee_bases)) =
            heap.vm_call_ic_fast_link(site, head, argc as u32, epoch)
        {
            // This entry hands back a `Value`, so it owns the destination: a stack local,
            // which is where the result would have been copied to anyway.
            let mut ret = Value::Nil;
            match jit_run_fast_link(
                heap,
                argc,
                site,
                head,
                epoch,
                stage_base,
                code as usize,
                nslots,
                callee_env,
                callee_bases,
                &mut ret as *mut Value,
            ) {
                FastLinkOutcome::Done => return Some(ret),
                FastLinkOutcome::Error => return None,
                // IC changed under us (astronomically rare): the args were re-staged at
                // `[stage_base, ..)` — fall through to the slow path below.
                FastLinkOutcome::Fallthrough => {}
            }
        }
    }

    // ---- Native-to-native call linking ----
    // Link straight to the callee's installed, epoch-current native code — set up its frame
    // at `stage_base` and call its entry — skipping `dispatch → vm_apply → vm_run_bc →
    // jit_tier`. The arm (and captured env) come from the call-site IC (reusing the VM's
    // `vm_call_ic`, epoch-stamped): a hit costs no `env_get` and no `compiled_arm_for`. The
    // frame is exactly where the VM puts a callee frame, so this holds no more roots than the
    // interpreter. These sites bypass `exec_chunk`, so the JIT self-populates the IC on a miss.
    {
        // Direct-call a BUILTIN callee: read the staged args (rooted at
        // `[stage_base, n)` — the same discipline the VM's `Inst::Call` uses),
        // invoke the native fn pointer, park any error. This is the native-callee
        // fast path: no `env_get`, no `dispatch` (passthrough loop + `apply`
        // unfold + arity re-checks) — the ~55–75 ns/call protocol every `str`/
        // `string/char->int`/`string-length` from JIT'd code used to pay. `apply` itself
        // has a real native body (`apply_builtin`), so direct invocation is exact.
        macro_rules! call_native_direct {
            ($nid:expr) => {{
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                for k in 0..argc {
                    argv.push(heap.root_at(stage_base + k));
                }
                let env = heap.read_root_env(heap.jit_call_env);
                let r = crate::eval::call_native(heap, $nid, &argv, env);
                heap.truncate_roots(stage_base);
                return match r {
                    Ok(v) => Some(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        None
                    }
                };
            }};
        }
        let resolved: Option<(Arc<ArmHandle>, EnvId, (u32, u32))> = if elided {
            match heap.vm_call_ic_probe(site, head, argc as u32, epoch) {
                Some((_, Some(t))) => Some(t),
                // IC hit on a NATIVE callee (arm-less entry, filled below on first
                // resolve): the whole call is one arity-checked fn-pointer call.
                Some((v, None)) if !over_cap => {
                    if let ValueRef::Native(nid) = v.unpack() {
                        // Reaching here means the IR's flat-cell fast path missed (cold
                        // site, cleared table, or new epoch) — republish so the next call
                        // stays entirely in IR (arity pre-validated for this argc).
                        let nat = heap.native(nid);
                        if nat.arity.accepts(argc) && !value::is_dynamic(head) {
                            let func = nat.func as usize as u64;
                            heap.vm_fast_link_publish_native(site, head, argc as u32, epoch, func);
                        }
                        call_native_direct!(nid)
                    }
                    None
                }
                _ => {
                    // Miss: resolve the callee global (the only `env_get` on the call path,
                    // and only while cold) and fill the IC.
                    let cenv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(cenv, head).map(|v| v.unpack()) {
                        Some(ValueRef::Fn(id)) => {
                            compiled_arm_for(heap, id, argc).map(|a| {
                                let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                                let cb = heap.vm_arm_block(&a);
                                if !value::is_dynamic(head) {
                                    heap.vm_call_ic_put(
                                        site,
                                        crate::core::heap::CallIcEntry {
                                            sym: head,
                                            argc: argc as u32,
                                            epoch,
                                            callee: Value::func(id),
                                            arm: Some((a.clone(), env)),
                                            // Overwritten inside `vm_call_ic_put`.
                                            callee_bases: (0, 0),
                                        },
                                    );
                                }
                                (a, env, cb)
                            })
                        }
                        // A builtin callee: fill an arm-less IC entry (so the next call
                        // takes the direct path above) and call it now. Dynamic heads are
                        // never cached (they can shadow per call) but still call direct.
                        Some(ValueRef::Native(nid)) if !over_cap => {
                            if !value::is_dynamic(head) {
                                heap.vm_call_ic_put(
                                    site,
                                    crate::core::heap::CallIcEntry {
                                        sym: head,
                                        argc: argc as u32,
                                        epoch,
                                        callee: Value::native(nid),
                                        arm: None,
                                        callee_bases: (0, 0),
                                    },
                                );
                                // Flat-cell publish: the IR's next call at this site goes
                                // straight to the fn pointer (arity pre-validated here).
                                let nat = heap.native(nid);
                                if nat.arity.accepts(argc) {
                                    let func = nat.func as usize as u64;
                                    heap.vm_fast_link_publish_native(
                                        site,
                                        head,
                                        argc as u32,
                                        epoch,
                                        func,
                                    );
                                }
                            }
                            call_native_direct!(nid)
                        }
                        _ => None,
                    }
                }
            }
        } else if let ValueRef::Fn(id) = heap.root_at(stage_base).unpack() {
            // The non-elided (computed-head) resolve: no IC, so this runs per call — the
            // handle it hands back is memoized, not freshly allocated.
            compiled_arm_for(heap, id, argc).map(|a| {
                let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                let cb = heap.vm_arm_block(&a);
                (a, env, cb)
            })
        } else {
            None
        };
        if let Some((arm, callee_env, callee_bases)) = resolved {
            let code = arm.jit_code.load(Acquire);
            let installed =
                !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED;
            // `nslots > 0` mirrors `jit_lower_arm`'s return-via-`roots[base]` requirement;
            // no-optional/no-rest keeps the inline frame setup trivial and infallible. The
            // epoch guard mirrors `jit_tier`. Over the recursion cap → skip (the slow path
            // drains on the VM via `jit_force_vm`).
            if installed
                && arm.nslots > 0
                && arm.noptional == 0
                && arm.rest_slot.is_none()
                && !over_cap
                && arm.compile_epoch.load(Acquire) == epoch
            {
                let depth = heap.jit_native_depth;
                // Build the callee frame at `stage_base`. For an elided head the args are
                // already in place (`[stage_base, stage_base+argc)`); for a computed head the
                // dead callee slot sits below them, so shift the args down one (forward-safe:
                // each write is below its read). Then nil-fill the let/spill slots.
                if !elided {
                    for k in 0..argc {
                        let a = heap.root_at(stage_base + 1 + k);
                        heap.set_root_at(stage_base + k, a);
                    }
                }
                heap.truncate_roots(stage_base + argc);
                // Two-stage tiering: size the callee frame to the native version we are about
                // to CALL (inlined upgrade → `inline_nslots`; small → `nslots`). Keyed on the
                // `code` pointer loaded above, NOT on `inline_installed`: the flag is a second,
                // independently-racing read of the same fact, and a peer process swapping the
                // upgrade in between the two would have us size to `inline_nslots` while
                // calling the small native — whose outcome-4 tail staging then lands at
                // `base + nslots` and is read back here at `base + inline_nslots`. Captured
                // once and reused for both the frame extension and the staged_start
                // calculation — the two must agree on the same frame boundary.
                let frame_nslots = frame_size_for_code(&arm, code);
                heap.extend_roots_to_nil(stage_base + frame_nslots);
                let base = stage_base;
                // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from
                // `jit_lower_arm`, living for the process in `GLOBAL_JIT`; the frame is set
                // up at `roots[base..]`.
                let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code) };
                // Destination for a Done result — this entry hands back a `Value`, so it is
                // a stack local (see `crate::jit::JitArmFn`).
                let mut ret = Value::Nil;
                // Root callee_env via env_roots so GC tenure inside the callee forwards it.
                let env_base = heap.env_roots_len();
                let env_root = heap.root_env(callee_env);
                let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
                let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, head);
                // Fill the closure's capture slots from its captured env. The fast frame
                // setup above placed only the params (and `extend_roots_to_nil` zeroed the
                // rest) — it bypasses `push_frame`, which is where captures are normally
                // filled. Without this, a callee WITH captures reads its captured lexicals
                // (e.g. a fold reducer's free `dir`) as nil, producing wrong results /
                // type errors far away (`path-join nil …` → `string-length: got nil`).
                // capture_base == argc here: noptional == 0 && rest_slot is none (guarded
                // above) and nrequired == argc (the arm was selected for this argc). Reads
                // are alloc-free (no GC), so the nil-filled body slots above stay valid.
                if !arm.capture_names.is_empty() {
                    let cenv = heap.read_root_env(env_root);
                    for (k, &name) in arm.capture_names.iter().enumerate() {
                        let v = heap.capture_value(cenv, k, name);
                        heap.set_root_at(stage_base + argc + k, v);
                    }
                }
                heap.jit_native_depth = depth + 1;
                stamp_stack_limit_if_outermost(heap, depth);
                let saved_force_vm = heap.jit_force_vm;
                // The callee's native code reads its OWN IC block through the heap
                // cursors (fast-link base, IC callbacks) — install it for the call and
                // restore the caller's around it, like `jit_call_env` above.
                let saved_bases = heap.set_ic_bases(callee_bases);
                heap.native_gateway_seq += 1;
                let gw_seq = heap.native_gateway_seq;
                let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
                let outcome = f(heap as *mut Heap, base as i64, &mut ret as *mut Value);
                heap.cur_native_gateway = saved_gw;
                jit_suspend_feedback(heap, &arm, outcome, gw_seq);
                heap.set_ic_bases(saved_bases);
                heap.jit_force_vm = saved_force_vm;
                heap.jit_native_depth = depth;
                heap.jit_call_env = saved;
                heap.jit_dbg_fn = saved_fn;
                // Deopt feedback (see `jit_deopt_feedback`) for the native→native link.
                if arm.deopt_watch {
                    use std::sync::atomic::Ordering::Relaxed;
                    if outcome == 1 {
                        jit_deopt_feedback(&arm);
                    } else if arm.jit_deopts.load(Relaxed) != 0 {
                        arm.jit_deopts.store(0, Relaxed);
                    }
                }
                // `f()` runs the callee, which allocates freely and so may have triggered a
                // collection that *relocated* the captured env. `minor_collect` forwarded the
                // rooted copy (`env_root`) but NOT the local `callee_env` EnvId — re-read the
                // live id from its root before dropping it. Without this the deopt path below
                // hands `vm_apply` a stale env handle → `push_frame`/`env_frame` use-after-GC
                // (the whole reason `callee_env` was env-rooted at all). The other outcomes
                // read their results from `roots` (already GC-updated), so this is the one
                // post-`f()` consumer of the locally-held handle.
                let callee_env = heap.read_root_env(env_root);
                heap.truncate_env_roots(env_base);
                match outcome {
                    // Done: the arm wrote the result through `ret`. Drop the frame.
                    0 => {
                        crate::perf_bump!(jit_link_done);
                        heap.truncate_roots(stage_base);
                        return Some(ret);
                    }
                    // Error: callee parked it. PROPAGATE — never re-run, or an already-failed
                    // subtree re-errors at every unwinding level (quadratic).
                    3 => {
                        heap.truncate_roots(stage_base);
                        return None;
                    }
                    // Tail call (4): the callee JIT'd a tail — [callee, arg0..argN] staged above
                    // its frame at `[base+frame_nslots, roots_len)`. Follow the chain rather than
                    // re-running the callee via `vm_apply` (which would pay both JIT and VM cost).
                    4 => {
                        let staged_start = base + frame_nslots;
                        let staged_end = heap.roots_len();
                        if staged_end > staged_start {
                            let staged_callee = heap.root_at(staged_start);
                            let staged_argc = staged_end - staged_start - 1;
                            let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                                .map(|k| heap.root_at(staged_start + k))
                                .collect();
                            heap.truncate_roots(stage_base);
                            return match apply_value(
                                heap,
                                staged_callee,
                                &staged_args,
                                heap.global(),
                            ) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    heap.jit_pending_error = Some(e);
                                    None
                                }
                            };
                        }
                        heap.truncate_roots(stage_base);
                        return None;
                    }
                    // deopt (1) / preempt (2): re-run the callee on the VM. The args
                    // survive in the frame's param slots `[base, base+argc)` (params aren't
                    // overwritten by the arm body), so re-read, drop the frame, and `vm_apply`.
                    _ => {
                        crate::perf_bump!(jit_link_rerun);
                        // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the
                        // checkpoint, frame intact — never re-running side effects.
                        // `1 | 2` (deopt AND preempt), consistent with the other three
                        // consumers — see the note at the fast-link resume above (KI-18).
                        if matches!(outcome, 1 | 2) {
                            if let Some((resume, rip, depth)) =
                                jit_ckpt_resume(heap, arm.arc(), base, frame_nslots)
                            {
                                return match vm_resume_deopt(
                                    heap, resume, base, callee_env, rip, depth,
                                ) {
                                    Ok(v) => Some(v),
                                    Err(e) => {
                                        heap.jit_pending_error = Some(e);
                                        None
                                    }
                                };
                            }
                        }
                        let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                        for k in 0..argc {
                            argv2.push(heap.root_at(base + k));
                        }
                        heap.truncate_roots(stage_base);
                        return match vm_apply(heap, arm, &argv2, callee_env) {
                            Ok(v) => Some(v),
                            Err(e) => {
                                heap.jit_pending_error = Some(e);
                                None
                            }
                        };
                    }
                }
            }
        }
    }

    // ---- Slow path ---- (not linkable: not yet native, over the cap, or a non-closure /
    // unbound callee). Resolve the callee (elided: via `env_get`; computed: the staged slot)
    // and run it on the VM. The args are the top `argc` operands either way.
    let callee = if elided {
        let cenv = heap.read_root_env(heap.jit_call_env);
        match heap.env_get(cenv, head) {
            Some(v) => v,
            None => {
                heap.jit_pending_error = Some(crate::eval::unbound_error(heap, head));
                return None;
            }
        }
    } else {
        heap.root_at(stage_base)
    };
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(n - argc + k));
    }
    let env = heap.read_root_env(heap.jit_call_env);
    // Over the native cap: force this dispatch (and all it recurses into) onto the VM, so the
    // remaining recursion drains through the bounded heap-frame loop. Restored after.
    let saved_force = if over_cap {
        Some(std::mem::replace(&mut heap.jit_force_vm, true))
    } else {
        None
    };
    let result = match dispatch(heap, callee, argv, false, env) {
        Ok(Step::Done(v)) => Ok(v),
        Ok(Step::Tail {
            compiled,
            args,
            genv,
            bases: _,
        }) => vm_apply(heap, compiled, &args, genv),
        Err(e) => Err(e),
    };
    if let Some(prev) = saved_force {
        heap.jit_force_vm = prev;
    }
    match result {
        Ok(v) => {
            heap.truncate_roots(stage_base);
            // GC safepoint: mirrors vm_run_bc's outer-loop check so native
            // calls from the JIT get GC opportunities at the same cadence as
            // the BcFrame path. Root `v` first so it survives relocation.
            if !crate::process::macro_block_active() && heap.gc_due() {
                heap.push_root(v);
                heap.collect(&mut [], &mut []);
                let relocated = heap.root_at(heap.roots_len() - 1);
                heap.truncate_roots(heap.roots_len() - 1);
                Some(relocated)
            } else {
                Some(v)
            }
        }
        Err(e) => {
            // Symmetric with the `Ok` arm: drop the call's staged operands
            // (callee + args at `[stage_base, n)`) now that the call failed. Safe —
            // the thrown value rides in `e` (off the roots stack), and this arm does
            // no GC (only the `Ok` arm collects), so nothing can go stale; this just
            // frees the staged roots immediately instead of leaving them for the
            // `try` handler's `truncate_roots(entry_roots)` to reclaim later.
            heap.truncate_roots(stage_base);
            heap.jit_pending_error = Some(e);
            None
        }
    }
}

/// Run a JIT'd arm's **tail** Brood→Brood call (outcome 4). The callee + `argc` args were
/// staged on `roots` *above the frame top* (`base + nslots`) in the VM's `Inst::Call`
/// layout (`[.., callee, arg0 .. arg_{argc-1}]`) — `argc` is recovered from the root
/// length since the JIT keeps its own operands in registers (so the frame top is always
/// exactly `base + nslots`). Unlike the non-tail path, the call *is* the arm's result
/// (TCO), so this resolves it with `tail = true` and hands [`vm_run_bc`] a [`ChunkExit`]
/// to **reuse** the current frame with — `Tail` for a VM-closure callee (run on the main
/// driver loop, keeping full preempt/suspend support), `Done` for an already-run
/// native/tree-walked callee. The native stack never grows: the driver's loop is the
/// trampoline. Mirrors the tail branch of the VM's `Inst::Call`.
#[cfg(feature = "jit")]
pub(crate) fn jit_dispatch_tail(
    heap: &mut Heap,
    base: usize,
    arm: &CompiledArm,
    env: EnvRoot,
    // The size this frame was actually BUILT to, captured by the trampoline at native entry.
    // **Must be passed, never re-derived here** (KI-48). It used to be
    // `base + arm.frame_size_for_new_entry()`, which re-reads `inline_installed` — the KI-26 / ADR-210
    // anti-pattern. The background inline upgrade can flip that flag between the native
    // entering (small frame) and this callback running, after which the staged
    // `[callee, args…]` is written at one offset and read at another: measured live on 123
    // arms, `fold` among them at nslots=13 vs inline_nslots=25, i.e. a 12-slot overshoot
    // straight past the staged area and off the roots stack. The caller already captures the
    // size once for exactly this reason ("the two must agree on the same frame boundary"),
    // and the deopt-resume helpers are already told it rather than re-deriving; this path was
    // the one that was not.
    frame_nslots: usize,
) -> Result<ChunkExit, LispError> {
    // A tail call is staged by the native code ABOVE its own frame top.
    let top = base + frame_nslots;
    let n = heap.roots_len();
    // KI-48 tripwire. `top` is derived from `frame_size_for_new_entry()`, which re-reads
    // `inline_installed` — the KI-26 / ADR-210 anti-pattern — while the frame this native
    // actually built was sized by whichever body was installed when it was ENTERED. If the
    // background inline upgrade lands in that window, the two disagree and `top` points past
    // the staged `[callee, args…]` region, which is how KI-48 was captured: `root_at(9)` on
    // a len-8 stack.
    //
    // Two reasons this is checked rather than left to `root_at`'s own bounds check. The
    // subtraction below underflows first on exactly this input (`top >= n` ⇒ wrapped `argc`),
    // so the OOB panic is the *lucky* ordering — the other one loops `root_at(top + 1 + k)`
    // over a huge count. And `root_at`'s panic names neither the arm nor the frame sizes, so
    // the original report could say only "index 9, len 8" and nothing about why.
    if top >= n {
        let name = arm
            .dbg_name
            .map(crate::core::value::symbol_name_ref)
            .unwrap_or("<closure>");
        panic!(
            "KI-48: jit_dispatch_tail staged-area desync — arm={name} base={base} \
             frame_nslots={frame_nslots} (nslots={} inline_nslots={} inline_installed={}) \
             top={top} roots_len={n}; the frame was built for a different body than the \
             caller reported",
            arm.nslots,
            arm.inline_nslots,
            arm.inline_installed
                .load(std::sync::atomic::Ordering::Acquire),
        );
    }
    let argc = n - top - 1;
    let callee = heap.root_at(top);
    // Verify the staged tail-call args too (BROOD_JIT_VERIFY / _FN) — the tail path is
    // separate from jit_dispatch_call, and a tail-called callee (e.g. pong's lambda
    // tail-calling `badge-ops`) stages its args here. The callee is a Value, so resolve
    // its closure name for the `_FN` match (u32::MAX = anonymous → "<computed>").
    if jit_verify_active() {
        let head = match callee.unpack() {
            crate::core::value::ValueRef::Fn(id) => heap.closure(id).name.unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        jit_verify_staged(heap, top + 1, n, head, NO_SITE, argc);
    }
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(top + 1 + k));
    }
    let env_id = heap.read_root_env(env);
    // `dispatch(.., tail = true, ..)` resolves a VM-closure callee to a `Step::Tail`
    // **without running it** (no native recursion) and runs a native/tree-walked callee
    // to a `Step::Done`. An error (incl. a control/suspend from a directly tail-called
    // suspending native — unreachable from surface `receive`, whose match closure puts
    // the arm out of subset) propagates; `vm_run_bc` unwinds the staged operands.
    let step = dispatch(heap, callee, argv, true, env_id)?;
    // Success: drop the staged operands. The driver next truncates to `base` and rebuilds
    // the frame for the callee (reuse), so leaving them would be harmless — but truncating
    // keeps the root stack tight if the callee turned out native (`Done`).
    heap.truncate_roots(top);
    Ok(match step {
        Step::Tail {
            compiled,
            args,
            genv,
            bases,
        } => ChunkExit::Tail {
            arm: compiled,
            args,
            genv,
            bases,
        },
        Step::Done(v) => ChunkExit::Done(v),
    })
}
