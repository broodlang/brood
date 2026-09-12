//! The fast native-to-native link (no per-call `Arc` clone) and the inline xcall path
//! (`docs/compute-frontier.md` §7.5): admission, the cold outcomes, and the fast-frame
//! dispatch that runs a linked callee in place.

use super::*;

/// The result of running a validated native fast-link ([`jit_run_fast_link`]): the call
/// completed (`Done`), raised an error parked for the arm to propagate (`Error`), or could
/// not be fast-linked after all (`Fallthrough` — the IC moved under us; the args have been
/// re-staged for the caller's slow path).
#[cfg(feature = "jit")]
pub(crate) enum FastLinkOutcome {
    /// The call completed; **the result is at the `out` pointer the caller passed in**, not
    /// carried here. Payload-less on purpose: a `Done(Value)` made this a 32-byte enum that
    /// returns through hidden-pointer memory (`sret`), so the value was copied again on a
    /// path whose whole cost was copying. See [`crate::jit::JitArmFn`].
    Done,
    Error,
    Fallthrough,
}

/// The shared body of a validated native fast-link: set up the callee frame at `stage_base`,
/// call its installed native `code`, and handle the outcome — `Done` (result boxed in
/// `roots[stage_base]`), the parked-error exit, or a deopt/preempt/tail that re-runs the
/// callee on the VM via the IC. Both [`jit_dispatch_call`] (after `vm_call_ic_fast_link`)
/// and [`jit_dispatch_fast_frame`] (the in-IR epoch-guarded path, which reads `code/nslots/
/// env` from the flat side table instead) funnel through here, so the two can never desync.
/// `epoch`/`stage_base` are the caller's already-computed values; `code` is a finalized
/// [`crate::jit::JitArmFn`]. On `Fallthrough` the `argc` args are re-staged at
/// `[stage_base, stage_base+argc)` for the caller's slow path.
///
/// `out` is where the result goes on `Done` — passed straight through to the native arm, so
/// the value is stored once by whoever produced it and never loaded back here. **Only** the
/// `Done` outcome writes it. See [`crate::jit::JitArmFn`] for why, and for the GC rule
/// (`out` is not a root; nothing may allocate between the store and the consumer).
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn jit_run_fast_link(
    heap: &mut Heap,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    code: usize,
    nslots: usize,
    callee_env: EnvId,
    callee_bases: (u32, u32),
    out: *mut Value,
) -> FastLinkOutcome {
    heap.truncate_roots(stage_base + argc);
    // DEBUG ONLY: the JIT fast path bypasses `push_frame`, so validate the staged args
    // here too — catch a corrupt arg at the earliest native frame entry (bug #2).
    #[cfg(debug_assertions)]
    {
        let args: Vec<Value> = (0..argc).map(|k| heap.root_at(stage_base + k)).collect();
        dbg_check_args(
            &args,
            &format!(
                "jit_run_fast_link site={site} loc={}",
                heap.dbg_site_loc(site)
            ),
        );
    }
    // Runtime BROOD_JIT_VERIFY: the fast-link path bypasses jit_dispatch_call's scan, so
    // scan the staged args here too (works in a plain --release build).
    if jit_verify_active() {
        jit_verify_staged(heap, stage_base, stage_base + argc, head, site, argc);
    }
    heap.extend_roots_to_nil(stage_base + nslots);
    let base = stage_base;
    // SAFETY: `code` is a finalized `extern "C" fn(*mut Heap, base)` from `jit_lower_arm`,
    // kept for the process in `GLOBAL_JIT`; the frame is at `roots[base..]`. Validated
    // current by the caller's epoch check (the IC fast-link, or the IR's flat-table guard).
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(code as *mut u8) };
    // Named `native_depth`, not `depth`: the deopt arm below binds a `depth` of its own
    // (the checkpoint's VM stack depth) that would otherwise shadow this one.
    let native_depth = heap.jit_native_depth;
    // Root callee_env via env_roots so GC tenure inside the callee forwards it.
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(callee_env);
    let saved = std::mem::replace(&mut heap.jit_call_env, env_root);
    let saved_fn = std::mem::replace(&mut heap.jit_dbg_fn, head);
    heap.jit_native_depth = native_depth + 1;
    // No stack-limit stamp here: the outermost native frame stamped it on entry
    // (`jit_tier_in_frame` / `hof_apply_native` / the scheduler's resume), the value is an
    // absolute address that a call from that frame cannot move, and re-deriving it through
    // `stacker::remaining_stack` on every depth-0 call was 6% of a call loop's samples.
    let saved_force_vm = heap.jit_force_vm;
    // KI-20: the callee's native code reads its OWN per-arm IC block through the heap
    // cursors (`vm_call_ic_put`/`vm_global_ic_put`/fast-link publishes). Install the callee's
    // bases for the call and restore the caller's around it — exactly as the cloning
    // native-link path in `jit_dispatch_call` does. Without this the callee wrote into the
    // caller's IC slots (and vice versa); never a wrong answer (every probe re-validates
    // `sym`/`argc`/`epoch`, so a crossed entry simply misses) but both arms ran permanently
    // cache-cold, and `dbg_site_loc` reported the wrong site. The bases arrive as args (they
    // rode in the `FastLink` slot), so this is two `Cell` writes, no table lookup.
    let saved_bases = heap.set_ic_bases(callee_bases);
    heap.native_gateway_seq += 1;
    let gw_seq = heap.native_gateway_seq;
    let saved_gw = std::mem::replace(&mut heap.cur_native_gateway, gw_seq);
    let outcome = f(heap as *mut Heap, base as i64, out);
    heap.cur_native_gateway = saved_gw;
    heap.set_ic_bases(saved_bases);
    heap.jit_force_vm = saved_force_vm;
    heap.jit_native_depth = native_depth;
    heap.jit_call_env = saved;
    heap.jit_dbg_fn = saved_fn;
    heap.truncate_env_roots(env_base);
    // Suspend-host latch (see `jit_latch_suspend_host`). The fast link carries no arm
    // reference, so resolve one — only on the token match, which happens at most once
    // per arm ever (cold); the steady-state cost is this one u64 compare. Resolution is
    // by the invoked code pointer against the keep-alive registry (every arm with
    // installed native code is in it, immortally — bug #2's fix), NOT the call IC: the
    // park usually spans a GC, whose epoch bump makes `vm_call_ic_probe` with the
    // pre-call `epoch` decline — measured as 13 dirty parks producing 3 latches, i.e.
    // a latch that mostly failed to hold. The probe stays as the fallback for the one
    // race the scan can miss (an inlined-upgrade swap between invoke and here).
    if heap.blocked_under_gateway == gw_seq {
        jit_latch_dirty_blocked(heap, code, site, head, argc, epoch);
    }
    // KI-11. Three of the outcome arms below re-enter the evaluator — the outcome-4
    // tail-chain follow-through (`apply_value`), and the deopt/preempt re-runs
    // (`vm_resume_deopt` / `vm_apply`). All of them run on THIS native frame, which is
    // still on the stack, so `jit_native_depth` must stay raised across them or the cap
    // stops bounding the native recursion: with it rolled back to `depth`, a chain of
    // tail-calling delegators oscillates between `depth` and `depth+1` forever while the
    // native stack grows without bound, and the process dies of a stack overflow that
    // `try`/`catch` cannot see (the VM and tree-walker both handle the same input). Each
    // re-entrant call below therefore re-raises the depth for its duration; see
    // `jit_native_reenter`. Found via JSONTestSuite's 20k-deep documents; the minimal
    // repro is a three-function cycle returning a destructured tuple.
    // Outcome 0 — the overwhelmingly common case — is handled inline; every other outcome
    // goes to a `#[cold] #[inline(never)]` helper.
    //
    // This is a code-LAYOUT change with no semantic content, and it is worth the indirection
    // because of where this function sits: `perf` puts `jit_run_fast_link` at **24% of
    // `bintree`** — as much as all of that row's native compute — and instruction-level
    // annotation showed the cost spread thin across the prologue/epilogue (register saves,
    // spills at -0x158/-0x160(%rbp)) rather than concentrated in any operation. That is the
    // signature of a large function on a hot path: the deopt/preempt/tail arms below need
    // several `SmallVec`s and many live values, so the compiler sized the frame and saved the
    // registers for them on EVERY call, including the ~all of them that just return a value.
    if outcome == 0 {
        crate::perf_bump!(jit_link_done);
        // The result is already at `*out` — the arm wrote it there. Nothing to load.
        heap.truncate_roots(stage_base);
        return FastLinkOutcome::Done;
    }
    jit_fast_link_cold_outcome(
        heap,
        outcome,
        argc,
        site,
        head,
        epoch,
        stage_base,
        base,
        nslots,
        native_depth,
        callee_env,
        out,
    )
}

/// §7.5 hot re-lowering: the largest frame the inline-blob re-compile is worth — see
/// the profitability comment at the `xcall_relower` gate in `jit_tier`.
#[cfg(feature = "jit")]
pub(super) const XCALL_RELOWER_MAX_NSLOTS: usize = 8;

/// §7.1 hot admission (`BROOD_XADMIT=1`, experiment): admit profitability-gate-refused
/// named defns at the HOT stage — deferred compile, inline call blob, frame-size cap.
#[cfg(feature = "jit")]
pub(super) fn xadmit_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_XADMIT").is_some_and(|v| v == "1"))
}

/// Name what hot admission did with this arm, under `BROOD_JIT_BAIL_TRACE=1`.
///
/// The complement of `trace_bail`: that says an arm was refused the general lowering, this
/// says whether the §7.1 experiment then picked it up. Reported per refused condition rather
/// than as one boolean, because "declined" and "admitted and still no faster" are opposite
/// findings that an A/B cannot tell apart from the outside.
#[cfg(feature = "jit")]
pub(super) fn xadmit_trace(arm: &std::sync::Arc<CompiledArm>, inlined: bool) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("BROOD_JIT_BAIL_TRACE").is_some()) {
        return;
    }
    let name = arm
        .dbg_name
        .map(crate::core::value::symbol_name_ref)
        .unwrap_or("<closure>");
    let why = if inlined {
        "declined: inlined body"
    } else if arm.inline_name.is_some() {
        "declined: has an inline variant"
    } else if arm.leaf.is_some() {
        "declined: leaf-spliced"
    } else if arm.dbg_name.is_none() {
        "declined: closure (not a named defn)"
    } else if arm.nslots > xadmit_max_nslots() {
        "declined: frame over BROOD_XADMIT_MAX_NSLOTS"
    } else {
        "admitted"
    };
    eprintln!(
        "[jit-xadmit] arm={name} nslots={} cap={} {why}",
        arm.nslots,
        xadmit_max_nslots()
    );
}

/// The frame cap hot admission applies, overridable by `BROOD_XADMIT_MAX_NSLOTS`.
///
/// **Why this is a knob and not a constant.** `BROOD_XADMIT=1` exists to answer "is admitting
/// a gate-refused named defn worth it?", and KI-109 records the answer for `mandelbrot`'s
/// `row-sum` as *noise*. That measurement could not have been about `row-sum`: its frame is
/// **nslots=14** and this cap is 8, so the arm was never admitted and both arms of the A/B ran
/// identical code. A lever that silently declines the case under test reports "no difference"
/// for the one reason that cannot be distinguished from "no effect" — the same shape as a gate
/// that passes because it scanned nothing.
///
/// The default is unchanged, so no ordinary run is affected; raising it is how the experiment
/// is actually run. It stays capped by the caller's other conditions, and an unparseable value
/// keeps the default rather than uncapping (`BROOD_L1_BUDGET`'s rule, for its reason).
#[cfg(feature = "jit")]
pub(super) fn xadmit_max_nslots() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("BROOD_XADMIT_MAX_NSLOTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(XCALL_RELOWER_MAX_NSLOTS)
    })
}

/// The suspend-host latch resolution for a fast link whose callee dirty-blocked its
/// worker (`blocked_under_gateway` came back equal to the gateway token). Shared by
/// [`jit_run_fast_link`] and the inline fast-frame path's `brood_rt_xcall_latch`
/// callback (§7.5, `BROOD_XCALL=1`) — cold by construction (at most once per arm ever).
#[cfg(feature = "jit")]
#[cold]
#[inline(never)]
pub(crate) fn jit_latch_dirty_blocked(
    heap: &mut Heap,
    code: usize,
    site: u32,
    head: Symbol,
    argc: usize,
    epoch: u64,
) {
    heap.blocked_under_gateway = 0;
    // Shed THIS site's fast link first, unconditionally: the latch stores `BAILED`
    // into `arm.jit_code`, but the FastLink mirror's hit path (and the raw load JIT'd
    // callers emit) never re-reads `jit_code` — so without this, a long-lived process
    // whose site is already populated keeps entering the latched native and parking
    // dirty forever. The IC bases were restored above, so `site` resolves against the
    // caller's block exactly as the lookup that entered here did.
    heap.vm_fast_link_clear_site(site);
    let scanned = {
        use std::sync::atomic::Ordering::Acquire;
        // Poison-tolerant like the two push sites: a codegen panic (the
        // CODEGEN-PANICKED path) may have poisoned this mutex, and the latch must
        // not turn that into a worker-thread crash.
        let reg = JIT_ARM_KEEPALIVE.lock().unwrap_or_else(|e| e.into_inner());
        reg.iter()
            .find(|a| std::ptr::eq(a.jit_code.load(Acquire), code as *mut u8))
            .cloned()
    };
    if let Some(arm) = scanned.or_else(|| {
        heap.vm_call_ic_probe(site, head, argc as u32, epoch)
            .and_then(|(_, a)| a)
            .map(|(arm, _, _)| arm.arc().clone())
    }) {
        jit_latch_suspend_host(&arm);
    }
}

/// The cold-outcome funnel for the **inline** fast-frame path (§7.5, `BROOD_XCALL=1`):
/// emitted code has already run the ceremony restores, so this is exactly
/// [`jit_fast_link_cold_outcome`] with the caller context read back off the heap.
/// The callee env is `GLOBAL` by the inline path's own guard.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
pub(crate) fn jit_xcall_cold_outcome(
    heap: &mut Heap,
    outcome: i64,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    nslots: usize,
    out: *mut Value,
) -> i64 {
    let native_depth = heap.jit_native_depth;
    match jit_fast_link_cold_outcome(
        heap,
        outcome,
        argc,
        site,
        head,
        epoch,
        stage_base,
        stage_base,
        nslots,
        native_depth,
        EnvId::GLOBAL,
        out,
    ) {
        FastLinkOutcome::Done => 0,
        FastLinkOutcome::Error => 1,
        FastLinkOutcome::Fallthrough => 2,
    }
}

/// The deopt / preempt / tail-chain / error outcomes of a native fast link — everything
/// except outcome 0. Split out of [`jit_run_fast_link`] and marked `#[cold]`/`#[inline(never)]`
/// so its frame and register pressure are not charged to the hot return path; see the comment
/// at the call site for the measurement that motivated it. Semantics are unchanged: this is the
/// same code, in the same order, with the same comments.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
#[cold]
#[inline(never)]
pub(super) fn jit_fast_link_cold_outcome(
    heap: &mut Heap,
    outcome: i64,
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    base: usize,
    nslots: usize,
    native_depth: u32,
    callee_env: EnvId,
    out: *mut Value,
) -> FastLinkOutcome {
    // These arms produce their value in Rust (a re-entered `apply_value` / `vm_resume_deopt`),
    // so they write it through `out` themselves — after all of their allocation, which is what
    // keeps the un-rooted `out` slot safe (see `crate::jit::JitArmFn`).
    let done = |v: Value| {
        // SAFETY: `out` is the caller's slot, valid for the whole call (its frame outlives
        // this one), and written exactly once on the Done path.
        unsafe { *out = v };
        FastLinkOutcome::Done
    };
    match outcome {
        3 => {
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Error
        }
        // Tail call (outcome 4): the callee JIT'd a tail call — [callee, arg0..argN] are staged
        // in roots above the callee's frame at `[base+nslots, roots_len)`. Rather than discarding
        // the staged call and re-running the callee via `vm_apply` (which would pay both JIT and
        // VM overhead for every tail-calling callee), follow the chain: dispatch the staged call
        // as if the callee had returned that value. This makes JIT-compiled thin delegators
        // (e.g. `prime?` tail-calling `divides-none?`) called in non-tail position efficient.
        4 => {
            let staged_start = base + nslots;
            let staged_end = heap.roots_len();
            if staged_end > staged_start {
                let staged_callee = heap.root_at(staged_start);
                let staged_argc = staged_end - staged_start - 1;
                let staged_args: SmallVec<[Value; 4]> = (1..=staged_argc)
                    .map(|k| heap.root_at(staged_start + k))
                    .collect();
                heap.truncate_roots(stage_base);
                let g = heap.global();
                return match jit_native_reenter(heap, native_depth, |h| {
                    apply_value(h, staged_callee, &staged_args, g)
                }) {
                    Ok(v) => done(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        FastLinkOutcome::Error
                    }
                };
            }
            // No staged call staged (shouldn't happen): fall back.
            heap.truncate_roots(stage_base);
            FastLinkOutcome::Error
        }
        // deopt (1) / preempt (2): re-run on the VM. The args survive in the param
        // slots `[base, base+argc)`. Re-probe for the arm (clones — but only on this rare
        // path) and `vm_apply`.
        _ => {
            crate::perf_bump!(jit_link_rerun);
            let mut argv2: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
            for k in 0..argc {
                argv2.push(heap.root_at(base + k));
            }
            // Resolve the callee's arm. The IC probe is only an *optimisation* for finding
            // it, so a miss must not change behaviour — but it did: the branch below used to
            // be skipped entirely on a miss, falling through to `brood_rt_call_slow`, which
            // **calls the callee again**. By then its native code has already run, so any
            // effect it performed happened twice (KI-18: both arms of a multi-arity fn were
            // entered 50 016 times instead of 50 000, exactly 16 — the deopt-bail threshold
            // — before the arm bailed and the duplication stopped). On a miss, resolve the
            // arm the slow way by name and take the same checkpoint-resume path.
            let resolved = heap
                .vm_call_ic_probe(site, head, argc as u32, epoch)
                .and_then(|(_, a)| a)
                .map(|(arm, cenv, _)| (arm, cenv))
                .or_else(|| {
                    let genv = heap.read_root_env(heap.jit_call_env);
                    match heap.env_get(genv, head) {
                        Some(Value::Fn(id)) => {
                            super::compiled_arm_for(heap, id, argc).map(|a| (a, callee_env))
                        }
                        _ => None,
                    }
                });
            if let Some((arm, cenv)) = resolved {
                // Deopt feedback (see `jit_deopt_feedback`): the fast-link hot path
                // carries no arm reference, so runs go uncounted here — only deopts.
                // Undercounted runs only make a mixed arm bail sooner (conservative).
                if outcome == 1 && arm.deopt_watch {
                    jit_deopt_feedback(&arm);
                }
                // Deopt-resume (see `CompiledArm::ckpt_slot`): resume AT the
                // checkpoint, frame intact — never re-running its side effects.
                // The shape check exists because the IC could have re-resolved to a
                // different arm than the one whose native ran; a mismatched frame can't be
                // resumed and takes the legacy re-run instead. It must be **flag-free** —
                // see [`jit_frame_shape_matches`] (KI-26).
                // `1 | 2` — deopt AND preempt — matching `vm_run_bc`'s handler (KI-18): the
                // journal, not the outcome code, decides whether there is a checkpoint to
                // resume from, and `jit_ckpt_resume` returns `None` on a zero journal. Today
                // a preempt provably observes a zero journal (`emit_self_call` resets it
                // immediately before the tick poll), but that invariant lives in the lowerer
                // while these consumers depend on it silently — and its failure mode is a
                // silently REPEATED side effect, so all four consumers now gate the same way.
                if matches!(outcome, 1 | 2) && jit_frame_shape_matches(&arm, nslots) {
                    if let Some((resume, rip, depth)) =
                        jit_ckpt_resume(heap, arm.arc(), base, nslots)
                    {
                        return match jit_native_reenter(heap, native_depth, |h| {
                            vm_resume_deopt(h, resume, base, cenv, rip, depth)
                        }) {
                            Ok(v) => done(v),
                            Err(e) => {
                                heap.jit_pending_error = Some(e);
                                FastLinkOutcome::Error
                            }
                        };
                    }
                }
                heap.truncate_roots(stage_base);
                return match jit_native_reenter(heap, native_depth, |h| {
                    vm_apply(h, arm, &argv2, cenv)
                }) {
                    Ok(v) => done(v),
                    Err(e) => {
                        heap.jit_pending_error = Some(e);
                        FastLinkOutcome::Error
                    }
                };
            }
            heap.truncate_roots(stage_base);
            // IC changed under us: restage the args so the elided slow path finds them.
            for a in &argv2 {
                heap.push_root(*a);
            }
            FastLinkOutcome::Fallthrough
        }
    }
}

/// The JIT's **in-IR** fast call path (Track B / Technique A). The arm's IR has already
/// validated this elided call site's flat-table fast-link (`site < len` && `epoch ==
/// global_epoch` && the slot's `sym`/`argc` match this site's baked head/arity — the last
/// guards against a call-site id reused across a `runtime_collect` clear, ADR-096) and read
/// `(code, nslots, env)` out of [`Heap::vm_fast_links`] with raw
/// loads — so this skips the IC probe + `RefCell` borrow that [`jit_dispatch_call`]'s fast
/// path pays (the measured 40.9%-of-`fib` cost) and runs the same frame body via
/// [`jit_run_fast_link`]. The `argc` args are the top operands on `roots`. Returns a
/// [`FastLinkOutcome`] the caller maps to a status: `Done` (result), `Error` (parked), or
/// `Fallthrough` — over the native-recursion cap, or the IC moved — which sends the IR to
/// the `brood_rt_call_slow` miss path with the args left staged.
#[cfg(feature = "jit")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn jit_dispatch_fast_frame(
    heap: &mut Heap,
    site: u32,
    head: Symbol,
    argc: usize,
    nslots: usize,
    code: usize,
    env: u64,
    callee_bases: (u32, u32),
    out: *mut Value,
) -> FastLinkOutcome {
    let n = heap.roots_len();
    let epoch = heap.global_epoch();
    // Elided (free-global) head: the args are the top `argc` operands; the frame starts there.
    let stage_base = n - argc;
    // Over the native-recursion cap → don't link (would overflow the native stack); the args
    // stay staged at `[stage_base, n)` so the slow path drains the recursion on the VM.
    if heap.jit_native_depth >= JIT_NATIVE_DEPTH_LIMIT
        || !jit_native_headroom_ok(heap.jit_native_depth)
    {
        return FastLinkOutcome::Fallthrough;
    }
    let callee_env = EnvId(env);
    // Cross-check (debug only, fires in the gate): the flat-table values the IR handed us
    // must equal what the authoritative IC fast-link resolves at this epoch — a mismatch is
    // a mirror desync and a silent-wrong-answer risk.
    #[cfg(debug_assertions)]
    {
        let auth = heap.vm_call_ic_fast_link(site, head, argc as u32, epoch);
        debug_assert!(
            matches!(auth, Some((c, ns, e, b)) if c as usize == code && ns == nslots && e == callee_env && b == callee_bases),
            "fast-link mirror desynced from the call IC (site {site}, head {head}): \
             mirror=(code={code:#x}, nslots={nslots}, env={:#x}, bases={callee_bases:?}) \
             auth={auth:?} — the IR's epoch+sym+argc guard should make this unreachable \
             (see FastLink)",
            callee_env.0
        );
    }
    jit_run_fast_link(
        heap,
        argc,
        site,
        head,
        epoch,
        stage_base,
        code,
        nslots,
        callee_env,
        callee_bases,
        out,
    )
}
