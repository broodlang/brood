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
    // May the callee preempt by unwinding (ADR-401)? What the calling activation may pass on.
    unwind: bool,
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
    //
    // The two LINK branches below are the exception, and take the depth-gated probe: they
    // enter the callee through `enter_native` from a native caller, which raises the depth
    // for the call's duration, so a recursion of links IS counted — the same reasoning as
    // `jit_run_fast_link`'s in-IR callers. The raw probe was most of the gap between a
    // computed-head call and a named one. A builtin called directly can re-enter evaluation
    // (`apply`) without counting, so it keeps the raw probe with the slow path.
    let depth = heap.jit_native_depth;
    let link_over_cap = depth >= JIT_NATIVE_DEPTH_LIMIT || !jit_native_headroom_ok(depth);
    let over_cap = || link_over_cap || !stack_headroom_ok();
    let epoch = heap.global_epoch();
    // A free-global head isn't staged (`elided`): the callee is resolved via the call IC.
    // `stage_base` is where the callee frame starts — directly at the args for an elided
    // head, one slot lower (over the staged callee) for a computed one.
    let elided = site != NO_SITE;
    // A computed head's slot carries its local's name (or `POSITIONLESS_CALL`) for the error
    // path only (`emit_call`'s `slow_head`). Split it off here, so `head` below keeps its one
    // meaning — the symbol of an elided head, else `u32::MAX` — for the ICs, the fast links
    // and `dbg_fn`, where a profiler sample reads it as a function's name.
    let local_head = (!elided && head != u32::MAX).then_some(head);
    let head = if elided { head } else { u32::MAX };
    let stage_base = if elided { n - argc } else { n - argc - 1 };

    #[cfg(debug_assertions)]
    {
        for k in stage_base..n {
            let v = heap.root_at(k);
            if let Some((kind, g, e)) = heap.dbg_value_stale(v) {
                // SAFETY: a diagnostic dump of the words (see `Value::raw_words`).
                let raw = unsafe { v.raw_words() };
                eprintln!(
                    "[jit-staged-stale] STALE {kind} (gen {g} != live {e}) staged at roots[{k}] \
                     BY arm '{}' for call to '{}' at {} (site={site}, argc={argc}); raw=[{:#x},{:#x},{:#x}]",
                    crate::core::value::symbol_name_opt(heap.jit_dbg_fn).unwrap_or("<unknown>"),
                    crate::core::value::symbol_name_opt(head).unwrap_or("<computed>"),
                    heap.dbg_site_loc(site),
                    raw[0],
                    raw[1],
                    raw[2],
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
    if elided
        && !link_over_cap
        && let Some((code, nslots, callee_env, callee_bases)) =
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
            unwind,
        ) {
            FastLinkOutcome::Done => return Some(ret),
            FastLinkOutcome::Error => return None,
            // IC changed under us (astronomically rare): the args were re-staged at
            // `[stage_base, ..)` — fall through to the slow path below.
            FastLinkOutcome::Fallthrough => {}
        }
    }

    // The callee's arm when the resolve below found one it could not link (not native yet,
    // an optional or rest parameter, over the cap): the slow path runs it as it is.
    let mut vm_arm: Option<(Arc<ArmHandle>, EnvId)> = None;
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
        // unfold + arity re-checks, ~55–75 ns/call) for `str`/`string-length` and
        // friends called from JIT'd code. `apply` itself
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
        // Resolve a closure callee's arm for this site and cache it on the call IC, from
        // which the next call publishes its fast link.
        let cache_closure_arm = |heap: &mut Heap, id| {
            compiled_arm_for(heap, id, argc).map(|a| {
                let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                let cb = heap.vm_arm_block(&a);
                heap.vm_call_ic_install(
                    site,
                    head,
                    argc as u32,
                    epoch,
                    Value::func(id),
                    Some((a.clone(), env)),
                );
                (a, env, cb)
            })
        };
        let resolved: Option<(Arc<ArmHandle>, EnvId, (u32, u32))> = if elided {
            match heap.vm_call_ic_probe(site, head, argc as u32, epoch) {
                Some((_, Some(t))) => Some(t),
                // An arm-less IC hit: a builtin callee, or a closure the interpreter cached
                // without its arm.
                Some((v, None)) if !over_cap() => match v.unpack() {
                    // A builtin (arm-less entry, filled below on first resolve): the whole
                    // call is one arity-checked fn-pointer call.
                    ValueRef::Native(nid) => {
                        // Reaching here means the IR's flat-cell fast path missed (cold
                        // site, cleared table, or new epoch) — republish so the next call
                        // stays entirely in IR (arity pre-validated for this argc).
                        let nat = heap.native(nid);
                        if nat.arity.accepts(argc) && !heap.is_dynamic(head) {
                            let func = nat.func as usize as u64;
                            heap.vm_fast_link_publish_native(site, head, argc as u32, epoch, func);
                        }
                        call_native_direct!(nid)
                    }
                    // A closure cached WITHOUT its arm: the interpreter's entry for a thin
                    // wrapper, which the VM runs through the dispatcher's redirect (ADR-069)
                    // and so never gives an arm. Native code runs the wrapper's own arm
                    // (ADR-408), and a link is published only from an entry that carries
                    // one — so attach it here. Returning `None` instead sent every call
                    // from native code to such a callee down the slow path for good: the
                    // entry kept hitting, so the miss branch that fills the arm never ran
                    // (`(defn add (a x) (+ a x))` from a native loop, ~2 100 instructions a
                    // call against ~30 once linked and inlined).
                    ValueRef::Fn(id) => cache_closure_arm(heap, id),
                    _ => None,
                },
                _ => {
                    // Miss: resolve the callee global (the only `env_get` on the call path,
                    // and only while cold) and fill the IC.
                    let cenv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(cenv, head).map(|v| v.unpack()) {
                        Some(ValueRef::Fn(id)) => cache_closure_arm(heap, id),
                        // A builtin callee: fill an arm-less IC entry (so the next call
                        // takes the direct path above) and call it now. Dynamic heads are
                        // never cached (they can shadow per call) but still call direct.
                        Some(ValueRef::Native(nid)) if !over_cap() => {
                            if !heap.is_dynamic(head) {
                                heap.vm_call_ic_install(
                                    site,
                                    head,
                                    argc as u32,
                                    epoch,
                                    Value::native(nid),
                                    None,
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
            // The computed-head resolve, memoised per closure instance: a higher-order
            // loop calls the same closure every iteration, so the hit skips the body-cache
            // lookup, the closure read and the IC-block lookup.
            heap.computed_call_probe(id, argc as u32, epoch)
                .or_else(|| {
                    compiled_arm_for(heap, id, argc).map(|a| {
                        let env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                        let cb = heap.vm_arm_block(&a);
                        heap.computed_call_put(id, argc as u32, epoch, &a, env, cb);
                        (a, env, cb)
                    })
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
            // Not a receive loop: its native code is the driver's alone (ADR-410).
            if installed
                && arm.nslots > 0
                && !arm.receive_at_head
                && arm.noptional == 0
                && arm.rest_slot.is_none()
                && !link_over_cap
                && arm.compile_epoch.load(Acquire) == epoch
            {
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
                // Size the callee frame to the native version we are about to CALL — keyed on
                // the `code` pointer loaded above, NOT on `inline_installed`, a second,
                // independently-racing read of the same fact (a peer swapping the upgrade in
                // between would have the frame and the code disagree; KI-48).
                let frame_nslots = frame_size_for_code(&arm, code);
                heap.extend_roots_to_nil(stage_base + frame_nslots);
                fill_captures(heap, &arm, stage_base, callee_env);
                let native_depth = heap.jit_native_depth;
                let mut ret = Value::Nil;
                let exit = enter_native(
                    heap,
                    NativeEntry {
                        code,
                        base: stage_base,
                        env: EntryEnv::Bare(callee_env),
                        dbg_fn: head,
                        ic_bases: Some(callee_bases),
                        from: EntryFrom::Native,
                        unwind,
                    },
                    &mut ret,
                );
                native_feedback(heap, arm.arc(), stage_base, frame_nslots, &exit);
                // A value is finished here: `finish_nested`'s DONE is this truncate, and
                // the call cost more than it does on every computed-head call.
                if exit.outcome == outcome::DONE {
                    crate::perf_bump!(jit_link_done);
                    heap.truncate_roots(stage_base);
                    return Some(ret);
                }
                let frame = NestedFrame {
                    stage_base,
                    base: stage_base,
                    nslots: frame_nslots,
                    argc,
                    native_depth,
                    unwind,
                };
                let env = exit.env;
                return match finish_nested(heap, exit.outcome, ret, &frame, false, |_| {
                    Some((arm, env, true))
                }) {
                    Nested::Done(v) => Some(v),
                    Nested::Raised | Nested::Unresolved => None,
                };
            }
            vm_arm = Some((arm, callee_env));
        }
    }

    // ---- Slow path ---- (not linkable: not yet native, an optional or rest parameter, over
    // the cap, or a non-closure / unbound callee). Run it on the VM. The args are the top
    // `argc` operands either way.
    //
    // A closure the resolve above already has the arm of runs that arm: `dispatch` would end
    // in the same `vm_apply`, after looking the arm up again by closure — a hash lookup, the
    // closure read and its IC block, ~13% of a native → interpreted → native round trip
    // (`json` makes 470 000 a run at N=20 000). An arm a module load has staled (ADR-366)
    // goes the long way, where the lookup recompiles it.
    let ready_arm = vm_arm.filter(|(arm, _)| {
        !arm.stale_bindings
            .load(std::sync::atomic::Ordering::Acquire)
    });
    // Otherwise resolve the callee value (elided: the call IC, else `env_get`; computed: the
    // staged slot). Walking the environment for every call cost `json` 590 000 walks a run
    // for `(nth s i -1)` alone, when `nth`'s default arm took a rest parameter.
    let callee = match (&ready_arm, elided) {
        (Some(_), _) => None,
        (None, true) => Some(
            match heap.vm_call_ic_probe(site, head, argc as u32, epoch) {
                Some((v, _)) => v,
                None => {
                    let cenv = heap.read_root_env(heap.jit_call_env);
                    match heap
                        .env_get(cenv, head)
                        .or_else(|| crate::eval::derive::root_escape_value(heap, head))
                    {
                        Some(v) => v,
                        None => {
                            heap.jit_pending_error = Some(crate::eval::unbound_error(heap, head));
                            return None;
                        }
                    }
                }
            },
        ),
        (None, false) => Some(heap.root_at(stage_base)),
    };
    let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
    for k in 0..argc {
        argv.push(heap.root_at(n - argc + k));
    }
    let env = heap.read_root_env(heap.jit_call_env);
    // Over the native cap: force this dispatch (and all it recurses into) onto the VM, so the
    // remaining recursion drains through the bounded heap-frame loop. Restored after.
    let saved_force = if over_cap() {
        Some(std::mem::replace(&mut heap.jit_force_vm, true))
    } else {
        None
    };
    // A thin wrapper — `(fn (acc x) (+ acc x))`, the canonical reducer — is called from
    // native code by running its compiled arm, not by the dispatcher's redirect to the inner
    // head (ADR-069). The redirect spares the interpreters a frame; from here it is two
    // by-name hops (`+`'s own arm is a wrapper of `%add`) at ~1 900 instructions a call, and
    // because the wrapper's body then never runs, its arm never got hot enough to tier up
    // and be linked natively. Run like any closure, it tiers, and the next calls take the
    // native path above. Same answer: the compiled body resolves its inner head by the
    // same late-bound name, and applies the contract boundary at compile time (ADR-383)
    // where the redirect re-creates it by value.
    let direct_arm = ready_arm.or_else(|| match callee.map(|c| c.unpack()) {
        Some(ValueRef::Fn(id)) if crate::eval::passthrough_arm(heap, id, argc).is_some() => {
            compiled_arm_for(heap, id, argc)
                .map(|a| (a, heap.closure(id).env.unwrap_or_else(|| heap.global())))
        }
        _ => None,
    });
    let result = match direct_arm {
        Some((arm, cenv)) => vm_apply(heap, arm, &argv, cenv),
        None => {
            let callee = callee.expect("resolved whenever no arm was ready");
            match dispatch(heap, callee, argv, false, env) {
                Ok(Step::Done(v)) => Ok(v),
                Ok(Step::Tail {
                    compiled,
                    args,
                    genv,
                    bases: _,
                }) => vm_apply(heap, compiled, &args, genv),
                // A computed head's `head` slot carries its local's name, if it is one
                // (`emit_call`'s `slow_head`).
                Err(e) => Err(crate::eval::name_uncallable_local(
                    heap, e, callee, local_head,
                )),
            }
        }
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
/// The tail call a native arm staged when it returned `outcome::TAIL`: `[callee, args…]`
/// written ABOVE its frame top, `base + frame_nslots`. Both readers — the driver's
/// [`jit_dispatch_tail`] and `dispatch`'s computed-head fast path — read it here.
///
/// `frame_nslots` is the size the frame was actually BUILT to, captured at native entry.
/// **It must be passed, never re-derived** (KI-48): `frame_size_for_new_entry()` re-reads
/// `inline_installed` — the KI-26 / ADR-210 anti-pattern — and the background inline upgrade
/// can flip it between the native entering (small frame) and this read, after which the
/// staged region is read at the wrong offset. That desync is how KI-48 was captured:
/// `root_at(9)` on a len-8 stack.
///
/// An empty staged area is therefore a desync, never a legitimate TAIL, and panics naming
/// the arm and both frame sizes. Two reasons it is checked rather than left to `root_at`'s
/// own bounds check: the subtraction below underflows first on exactly this input (a
/// wrapped `argc`), so the OOB panic is the *lucky* ordering — the other loops over a huge
/// count — and `root_at`'s panic names neither the arm nor the sizes. (`dispatch`'s copy
/// once fell back to re-running the arm on the VM instead, which repeats every effect the
/// native body already performed.)
pub(crate) fn read_staged_tail(
    heap: &Heap,
    arm: &CompiledArm,
    base: usize,
    frame_nslots: usize,
) -> (Value, SmallVec<[Value; 4]>) {
    let top = base + frame_nslots;
    let n = heap.roots_len();
    if top >= n {
        panic!(
            "KI-48: staged tail-call desync — arm={} base={base} frame_nslots={frame_nslots} \
             (nslots={} inline_nslots={} inline_installed={}) top={top} roots_len={n}; the \
             frame was built for a different body than the caller reported",
            arm.dbg_label(),
            arm.nslots,
            arm.inline_nslots,
            arm.inline_installed
                .load(std::sync::atomic::Ordering::Acquire),
        );
    }
    let argc = n - top - 1;
    let callee = heap.root_at(top);
    // Verify the staged tail-call args too (BROOD_JIT_VERIFY / _FN) — a tail-called callee
    // (e.g. pong's lambda tail-calling `badge-ops`) stages its args here, not through
    // jit_dispatch_call. The callee is a Value, so resolve its closure name for the `_FN`
    // match (u32::MAX = anonymous → "<computed>").
    if jit_verify_active() {
        let head = match callee.unpack() {
            crate::core::value::ValueRef::Fn(id) => heap.closure(id).name.unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        jit_verify_staged(heap, top + 1, n, head, NO_SITE, argc);
    }
    let argv = (0..argc).map(|k| heap.root_at(top + 1 + k)).collect();
    (callee, argv)
}

pub(crate) fn jit_dispatch_tail(
    heap: &mut Heap,
    base: usize,
    arm: &CompiledArm,
    env: EnvRoot,
    // The size this frame was actually BUILT to, captured by the trampoline at native entry.
    // **Must be passed, never re-derived here** (KI-48). Re-deriving it as
    // `base + arm.frame_size_for_new_entry()` re-reads `inline_installed` — the KI-26 / ADR-210
    // anti-pattern: the background inline upgrade can flip that flag between the native
    // entering (small frame) and this callback running, after which the staged
    // `[callee, args…]` is written at one offset and read at another, overshooting off the
    // roots stack. The caller captures the size once for exactly this reason, as do the
    // deopt-resume helpers.
    frame_nslots: usize,
) -> Result<ChunkExit, LispError> {
    let (callee, argv) = read_staged_tail(heap, arm, base, frame_nslots);
    let env_id = heap.read_root_env(env);
    let local = heap.take_jit_tail_local_head();
    // `dispatch(.., tail = true, ..)` resolves a VM-closure callee to a `Step::Tail`
    // **without running it** (no native recursion) and runs a native/tree-walked callee
    // to a `Step::Done`. An error (incl. a control/suspend from a directly tail-called
    // suspending native — unreachable from surface `receive`, whose match closure puts
    // the arm out of subset) propagates; `vm_run_bc` unwinds the staged operands.
    let step = dispatch(heap, callee, argv, true, env_id)
        .map_err(|e| crate::eval::name_uncallable_local(heap, e, callee, local))?;
    // Success: drop the staged operands. The driver next truncates to `base` and rebuilds
    // the frame for the callee (reuse), so leaving them would be harmless — but truncating
    // keeps the root stack tight if the callee turned out native (`Done`).
    heap.truncate_roots(base + frame_nslots);
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
