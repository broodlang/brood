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
/// call its installed native `code` ([`enter_native`]), and finish the outcome — `Done`
/// inline (the overwhelmingly common case), anything else through
/// [`fast_link_cold_outcome`]. Both [`jit_dispatch_call`] (after `vm_call_ic_fast_link`) and
/// [`jit_dispatch_fast_frame`] (the in-IR epoch-guarded path, which reads `code/nslots/env`
/// from the flat side table instead) funnel through here, so the two can never desync.
/// `epoch`/`stage_base` are the caller's already-computed values; `code` is a finalized
/// [`crate::jit::JitArmFn`]. On `Fallthrough` the `argc` args are re-staged at
/// `[stage_base, stage_base+argc)` for the caller's slow path.
///
/// `out` is where the result goes on `Done` — passed straight through to the native arm, so
/// the value is stored once by whoever produced it and never loaded back here. See
/// [`crate::jit::JitArmFn`] for why, and for the GC rule (`out` is not a root; nothing may
/// allocate between the store and the consumer).
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
    // May the callee preempt by unwinding (ADR-401)? What the calling activation may pass on.
    unwind: bool,
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
    let native_depth = heap.jit_native_depth;
    let exit = enter_native(
        heap,
        NativeEntry {
            code: code as *const u8,
            base: stage_base,
            env: EntryEnv::Bare(callee_env),
            dbg_fn: head,
            ic_bases: Some(callee_bases),
            from: EntryFrom::Native,
            unwind,
        },
        out,
    );
    // Suspend-host latch (see `jit_latch_suspend_host`). The fast link carries no arm
    // reference, so resolve one — only on the token match, which happens at most once
    // per arm ever (cold); the steady-state cost is this one u64 compare.
    if heap.blocked_under_gateway == exit.gateway {
        jit_latch_dirty_blocked(heap, code, site, head, argc, epoch);
    }
    // Outcome 0 is handled inline; every other outcome goes to a `#[cold] #[inline(never)]`
    // helper. A code-LAYOUT change with no semantic content: `perf` put this function at 24%
    // of `bintree`, spread thin across register saves and spills that the non-Done arms'
    // live values forced onto EVERY call.
    if exit.outcome == outcome::DONE {
        crate::perf_bump!(jit_link_done);
        // The result is already at `*out` — the arm wrote it there. Nothing to load.
        heap.truncate_roots(stage_base);
        return FastLinkOutcome::Done;
    }
    fast_link_cold_outcome(
        heap,
        exit.outcome,
        FastLinkSite {
            argc,
            site,
            head,
            epoch,
            stage_base,
            nslots,
            native_depth,
            callee_env: exit.env,
            code: code as *const u8,
            unwind,
        },
        out,
    )
}

/// §7.5 hot re-lowering: the largest frame the inline-blob re-compile is worth — see
/// the profitability comment at the `xcall_relower` gate in `jit_tier`.
#[cfg(feature = "jit")]
pub(super) const XCALL_RELOWER_MAX_NSLOTS: usize = 8;

/// The suspend-host latch resolution for a fast link whose callee dirty-blocked its
/// worker (`blocked_under_gateway` came back equal to the gateway token). Shared by
/// [`jit_run_fast_link`] and the inline fast-frame path's `brood_rt_xcall_latch`
/// callback — cold by construction (at most once per arm ever).
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
    blame_enclosing_gateway(heap);
    // Shed THIS site's fast link first, unconditionally: the latch stores `BAILED`
    // into `arm.jit_code`, but the FastLink mirror's hit path (and the raw load JIT'd
    // callers emit) never re-reads `jit_code` — so without this, a long-lived process
    // whose site is already populated keeps entering the latched native and parking
    // dirty forever. The IC bases were restored above, so `site` resolves against the
    // caller's block exactly as the lookup that entered here did.
    heap.vm_fast_link_clear_site(site);
    let scanned = jit_arm_for_code(code as *const u8);
    if let Some(arm) = scanned.or_else(|| {
        heap.vm_call_ic_probe(site, head, argc as u32, epoch)
            .and_then(|(_, a)| a)
            .map(|(arm, _, _)| arm.arc().clone())
    }) {
        jit_latch_suspend_host(heap, &arm);
    }
}

/// The cold-outcome funnel for the **inline** fast-frame path (§7.5): emitted code has
/// already run the ceremony restores, so this is [`fast_link_cold_outcome`] with the caller
/// context read back off the heap. The callee env is `GLOBAL` by the inline path's own
/// guard, and the code pointer does not ride out of the IR, so the callee is resolved
/// through the IC and then its name.
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
    unwind: bool,
) -> i64 {
    let native_depth = heap.jit_native_depth;
    match fast_link_cold_outcome(
        heap,
        outcome,
        FastLinkSite {
            argc,
            site,
            head,
            epoch,
            stage_base,
            nslots,
            native_depth,
            callee_env: EnvId::GLOBAL,
            code: std::ptr::null(),
            unwind,
        },
        out,
    ) {
        FastLinkOutcome::Done => 0,
        FastLinkOutcome::Error => 1,
        FastLinkOutcome::Fallthrough => 2,
    }
}

/// What a fast-linked call site knows about the call it made — enough to finish a cold
/// outcome without an arm in hand.
pub(crate) struct FastLinkSite {
    argc: usize,
    site: u32,
    head: Symbol,
    epoch: u64,
    stage_base: usize,
    nslots: usize,
    native_depth: u32,
    callee_env: EnvId,
    /// The code that ran, or null when the inline path did not carry it out.
    code: *const u8,
    unwind: bool,
}

/// Every outcome of a native fast link but 0, through [`finish_nested`]. Out of line and
/// cold so its frame and register pressure are not charged to the hot return path.
///
/// The fast link carries no arm, so the callee is resolved here: through the call IC when it
/// still names an arm of this frame's shape, else by the code pointer that ran (the keep-alive
/// registry names its arm immortally), else by name. The IC answer can be a DIFFERENT arm when
/// the epoch moved: a `def` in that window rebinds the name, and a resume would read the new
/// arm's journal out of the old arm's frame. And it must be resolved at all — falling through
/// to the slow path on a miss called the callee a second time after its native code had
/// already run (KI-18: 50 016 entries instead of 50 000). Only the IC and the code pointer
/// name the arm that RAN; a name lookup can not be told from a rebind, so an unwind never
/// records an arm found that way.
#[cfg(feature = "jit")]
#[cold]
#[inline(never)]
fn fast_link_cold_outcome(
    heap: &mut Heap,
    outcome: i64,
    call: FastLinkSite,
    out: *mut Value,
) -> FastLinkOutcome {
    let frame = NestedFrame {
        stage_base: call.stage_base,
        base: call.stage_base,
        nslots: call.nslots,
        argc: call.argc,
        native_depth: call.native_depth,
        unwind: call.unwind,
    };
    let resolve = |h: &Heap| -> Option<NestedCallee> {
        h.vm_call_ic_probe(call.site, call.head, call.argc as u32, call.epoch)
            .and_then(|(_, a)| a)
            .filter(|(arm, _, _)| jit_frame_shape_matches(arm, call.nslots))
            .map(|(arm, env, _)| (arm, env, true))
            .or_else(|| {
                (!call.code.is_null())
                    .then(|| jit_arm_for_code(call.code))
                    .flatten()
                    .map(|a| (ArmHandle::new(a), call.callee_env, true))
            })
            .or_else(|| {
                let genv = h.read_root_env(h.jit_call_env);
                match h.env_get(genv, call.head) {
                    Some(Value::Fn(id)) => super::compiled_arm_for(h, id, call.argc)
                        .map(|a| (a, call.callee_env, false)),
                    _ => None,
                }
            })
    };
    match finish_nested(heap, outcome, Value::Nil, &frame, true, resolve) {
        Nested::Done(v) => {
            // SAFETY: `out` is the caller's slot, valid for the whole call, written once —
            // after all of the re-entry's allocation, which keeps the un-rooted slot safe.
            unsafe { *out = v };
            FastLinkOutcome::Done
        }
        Nested::Raised => FastLinkOutcome::Error,
        Nested::Unresolved => FastLinkOutcome::Fallthrough,
    }
}

/// The debug cross-check behind [`jit_dispatch_fast_frame`]: the flat-mirror values the IR
/// handed us must equal what the authoritative `CallIcEntry` resolves **at the epoch the
/// mirror was published under** — a mismatch is a mirror desync and a silent-wrong-answer
/// risk. Fires in the gate (debug assertions), costs nothing in release.
///
/// At THAT epoch, not the current one: the IR validated the mirror against the global epoch
/// with a raw load, and a `def` on another worker can bump the epoch between that load and
/// this callback (`concurrency_race::fanout_with_concurrent_global_rebind_matches_serial` is
/// exactly that race). Probing at the new epoch would answer `None` for a mirror that was
/// valid when the IR read it. The release path is unaffected — `jit_run_fast_link`
/// re-validates and falls through on a move. And the ENTRY, not the probe:
/// `vm_call_ic_fast_link` reads the mirror first, so probing through it would compare the
/// mirror with itself. `tests::mirror_check_tolerates_a_
/// rebind_between_the_irs_load_and_the_callback` drives it with the race's exact state.
#[cfg(all(feature = "jit", debug_assertions))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn debug_check_fast_link_mirror(
    heap: &Heap,
    site: u32,
    head: Symbol,
    argc: usize,
    nslots: usize,
    code: usize,
    callee_env: EnvId,
    callee_bases: (u32, u32),
) {
    let Some(mirror_epoch) = heap.vm_fast_link_epoch(site) else {
        return; // cleared meanwhile: nothing to compare against
    };
    let auth = heap.vm_fast_link_authoritative(site, head, argc as u32, mirror_epoch);
    match auth {
        // The env and the callee's IC cursors must agree outright. The `(code, nslots)` pair
        // may instead be an EARLIER snapshot the arm was published with: a shared arm's
        // small → inlined swap or xcall re-lowering re-points only the swapping process's
        // links, and a peer's mirror keeps `(old code, old frame)` — self-consistent, and
        // what the old code wants (KI-174 addendum; seen as `nslots=1` against the entry's
        // `3` in two suite runs). A pair no publish ever produced is still a desync.
        Some((c, ns, e, b)) => debug_assert!(
            e == callee_env
                && b == callee_bases
                && ((c as usize == code && ns == nslots)
                    || Heap::fast_link_snapshot_was_published(code, nslots)),
            "fast-link mirror desynced from the call IC (site {site}, head {head}, epoch {mirror_epoch}): \
             mirror=(code={code:#x}, nslots={nslots}, env={:#x}, bases={callee_bases:?}) \
             auth={auth:?} — the IR's epoch+sym+argc guard should make this unreachable \
             (see FastLink)",
            callee_env.0
        ),
        // No authoritative link at the mirror's epoch: legitimate only when the entry has
        // moved on (a rebind landed after the IR's raw load), is gone (a clear), or names an
        // arm DEMOTED since the mirror was published — `BAILED` by the deopt-thrash latch,
        // `null`/`QUEUED` by ADR-372's re-lowering. A demotion leaves the mirror in place by
        // design (`vm_fast_link_clear_site`: the old native is still valid code, it only
        // deopts), so the entry refusing to link it is not a desync. Without this clause the
        // check fired deterministically on `jit::type_mixed_join_edges_stay_exact`, whose
        // `code` arm latches after sixteen deopts while `work`'s site still mirrors it.
        // Or the callee's stamp moved past the mirror's epoch: an entry that finds its code
        // still right moves the stamp to the live epoch (KI-303), so the epoch has moved
        // since the IR's raw load — the same race as above, seen through the callee.
        None => debug_assert!(
            heap.vm_call_ic_entry_epoch(site)
                .is_none_or(|e| e != mirror_epoch)
                || heap.vm_call_ic_entry_native_installed(site) != Some(true)
                || heap.global_epoch() != mirror_epoch,
            "fast-link mirror at epoch {mirror_epoch} (site {site}, head {head}) has an entry at the \
             same epoch that does not fast-link: mirror=(code={code:#x}, nslots={nslots}) — a \
             mirror published for an entry the authoritative rules refuse"
        ),
    }
}

/// The JIT's **in-IR** fast call path (Track B / Technique A). The arm's IR has already
/// validated this elided call site's flat-table fast-link (`site < len` && `epoch ==
/// global_epoch` && the slot's `sym`/`argc` match this site's baked head/arity — the last
/// guards against a call-site id reused across a `runtime_collect` clear, ADR-096) and read
/// `(code, nslots, env)` out of [`Heap::vm_fast_links`] with raw
/// loads — so this skips the IC probe + `RefCell` borrow that [`jit_dispatch_call`]'s fast
/// path pays (~40% of `fib`) and runs the same frame body via
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
    unwind: bool,
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
    // must equal what the authoritative IC fast-link resolves at the epoch the mirror was
    // published under — a mismatch is a mirror desync and a silent-wrong-answer risk.
    //
    // At THAT epoch, not the one read above: the IR validated the mirror against the
    // global epoch with a raw load, and a `def` on another worker can bump the epoch
    // between that load and this callback (`concurrency_race::fanout_with_concurrent_
    // global_rebind_matches_serial` is exactly that race). Probing at the new epoch would
    // answer `None` for a mirror that was valid when the IR read it. The release path is unaffected:
    // `jit_run_fast_link` re-validates against `epoch` and falls through on a move. A slot
    // whose mirror was cleared meanwhile has nothing to compare against and stands down.
    #[cfg(debug_assertions)]
    debug_check_fast_link_mirror(
        heap,
        site,
        head,
        argc,
        nslots,
        code,
        callee_env,
        callee_bases,
    );
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
        unwind,
    )
}

// `debug_assertions` too: every test here drives `debug_check_fast_link_mirror`, a debug-build
// cross-check that a release build does not compile — and `make asan` builds `--release`.
#[cfg(all(test, feature = "jit", debug_assertions))]
mod tests {
    use super::*;
    use crate::core::value;

    /// The `caller`/`callee` pair the mirror cross-check tests tier up.
    const THE_PAIR: &str = "(do (defn callee (x) (+ x 1)) \
         (defn caller (n acc) (if (= n 0) acc (caller (- n 1) (callee acc)))))";

    /// How many fresh interpreters [`tiered_pair`] tries before it calls the publish broken.
    const PUBLISH_ATTEMPTS: usize = 5;

    /// A fresh interpreter at ceiling 2 with `define`'s pair defined, tiered up through `run`
    /// until the caller's call site has PUBLISHED a flat mirror for `callee`, with that slot
    /// and its absolute index. The ceiling is forced (per thread) so the tests mean the same
    /// thing under every `BROOD_TIER`.
    ///
    /// Only the caller's SMALL native code publishes: both callees here are leaves, so the
    /// caller's inlined upgrade splices the callee's body in and makes no call at all. When
    /// that upgrade is installed before the small native's first call after the callee went
    /// native, nothing will ever publish — a correct state (and the faster one), which is
    /// what KI-281 was: three CI failures in this helper, the third dump showing the entry
    /// linkable and the caller's upgrade installed. That attempt is discarded and a fresh
    /// interpreter (fresh arms, fresh caches — they are per runtime) tries again. A publish
    /// that is really broken fails every attempt, so the sabotages still go red.
    fn tiered_pair(
        define: &str,
        run: &str,
        callee: value::Symbol,
    ) -> (crate::Interp, usize, crate::core::heap::FastLink) {
        for _ in 0..PUBLISH_ATTEMPTS {
            set_forced_ceiling(Some(Tier::Native));
            let mut interp = crate::Interp::new();
            interp.eval_str(define).expect("define the pair");
            if let Some((abs, mirror)) = published_mirror_running(&mut interp, run, callee) {
                return (interp, abs, mirror);
            }
        }
        panic!(
            "in {PUBLISH_ATTEMPTS} fresh interpreters the caller's inlined upgrade was installed \
             before its small native ever published a fast-link mirror for the callee — the \
             publish from an arm-less or armed IC hit is broken, not merely late"
        );
    }

    /// Does `arm` run its inlined upgrade? Read off the code pointers (the installed code IS
    /// the inlined code), the way `frame_size_for_code` tells them apart.
    fn runs_its_inlined_upgrade(arm: &CompiledArm) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let inlined = arm.inline_code.load(Relaxed);
        !inlined.is_null()
            && inlined != crate::jit::QUEUED
            && inlined != crate::jit::BAILED
            && arm.jit_code.load(Relaxed) == inlined
    }

    /// Run `run` until a mirror for `callee` is published (`Some`), or until some arm runs its
    /// inlined upgrade with nothing published (`None`: this attempt lost the race
    /// [`tiered_pair`] describes). FAILS past a generous deadline: a setup that never
    /// publishes would otherwise pass every test using it vacuously.
    fn published_mirror_running(
        interp: &mut crate::Interp,
        run: &str,
        callee: value::Symbol,
    ) -> Option<(usize, crate::core::heap::FastLink)> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        let mut runs = 0u64;
        while std::time::Instant::now() < deadline {
            interp.eval_str(run).expect("run the pair");
            runs += 1;
            if let Some(hit) = interp
                .heap
                .vm_fast_links_published()
                .into_iter()
                .find(|(_, fl)| fl.sym == callee)
            {
                return Some(hit);
            }
            if interp
                .heap
                .dbg_compiled_arms()
                .iter()
                .any(|arm| runs_its_inlined_upgrade(arm))
            {
                return None;
            }
        }
        // Report what the JIT made of every arm, so a failure says whether an arm never
        // compiled, latched BAILED, or kept deopting.
        use std::sync::atomic::Ordering::Relaxed;
        let arms: Vec<String> = interp
            .heap
            .dbg_compiled_arms()
            .iter()
            .map(|arm| {
                let code = arm.jit_code.load(Relaxed);
                let state = if code.is_null() {
                    "uncompiled"
                } else if code == crate::jit::BAILED {
                    "BAILED"
                } else {
                    "native"
                };
                format!(
                    "{} uid {} nsites {} {state} compile-epoch {} stale {} calls {} deopts {} \
                     (total {}) entry-deopts {} runs-inlined-upgrade {}",
                    arm.dbg_label(),
                    arm.uid,
                    arm.nsites,
                    arm.compile_epoch.load(Relaxed),
                    arm.stale_bindings.load(Relaxed),
                    arm.jit_calls.load(Relaxed),
                    arm.jit_deopts.load(Relaxed),
                    arm.jit_deopts_total.load(Relaxed),
                    arm.jit_entry_deopts.load(Relaxed),
                    runs_its_inlined_upgrade(arm),
                )
            })
            .collect();
        // Each hypothesis left after 2026-10-22 reads off these: the caller's IC entry for the
        // callee (arm-less? a stale or never-native arm? an epoch behind?) and the cell beside
        // it (a builtin cell where a Brood link belongs), against the global epoch.
        let sites = interp.heap.dbg_call_ics_for(callee);
        panic!(
            "the pair never published a fast-link mirror — the state under test was never \
             reached ({runs} runs of {run} in 90 s; published: {:?})\nglobal epoch {}\n\
             call ICs for the callee:\n  {}\narms:\n  {}",
            interp
                .heap
                .vm_fast_links_published()
                .iter()
                .map(|(_, fl)| crate::core::value::symbol_name(fl.sym))
                .collect::<Vec<_>>(),
            interp.heap.global_epoch(),
            sites.join("\n  "),
            arms.join("\n  ")
        );
    }

    #[test]
    fn mirror_check_tolerates_a_rebind_between_the_irs_load_and_the_callback() {
        // The race `concurrency_race::fanout_with_concurrent_global_rebind_matches_serial`
        // once hit, rebuilt deterministically: a mirror published at epoch E (the IR's raw
        // load validated it), a `def` of the callee bumps the epoch, and THEN the callback's
        // cross-check runs with the values the IR already holds. It must not fire — the
        // mirror was valid when read, and the release path re-validates and falls through.
        // Sabotage: pass `heap.global_epoch()` to `vm_fast_link_authoritative` in
        // `debug_check_fast_link_mirror` and this reds.
        let callee = value::intern("callee");
        let (mut interp, abs, mirror) = tiered_pair(THE_PAIR, "(caller 20000 0)", callee);
        let epoch_before = interp.heap.global_epoch();
        assert_eq!(
            mirror.epoch, epoch_before,
            "the mirror is at the current epoch before the rebind"
        );
        // The race's second half: a rebind lands after the IR's raw load.
        interp
            .eval_str("(def callee (fn (x) (+ x 2)))")
            .expect("rebind the callee");
        assert!(
            interp.heap.global_epoch() > epoch_before,
            "the rebind did not move the epoch — the state under test is not the race's"
        );
        // Address the slot the way the IR does: site-relative to the current IC base.
        let old_bases = interp
            .heap
            .set_ic_bases((abs as u32, interp.heap.ic_bases().1));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            debug_check_fast_link_mirror(
                &interp.heap,
                0,
                callee,
                mirror.argc as usize,
                mirror.nslots as usize,
                mirror.code as usize,
                EnvId(mirror.env),
                (mirror.callee_ic_base, mirror.callee_gic_base),
            );
        }));
        interp.heap.set_ic_bases(old_bases);
        assert!(
            outcome.is_ok(),
            "the cross-check fired on a mirror that was valid when the IR read it — a rebind between \
             the IR's epoch load and the callback is not a desync"
        );
    }

    #[test]
    fn mirror_check_still_fires_on_a_real_desync() {
        // The check must remain a check: a mirror whose values disagree with the entry at the
        // SAME epoch is the silent-wrong-answer case it exists for.
        let callee = value::intern("callee");
        let (interp, abs, mirror) = tiered_pair(THE_PAIR, "(caller 20000 0)", callee);
        let old_bases = interp
            .heap
            .set_ic_bases((abs as u32, interp.heap.ic_bases().1));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            debug_check_fast_link_mirror(
                &interp.heap,
                0,
                callee,
                mirror.argc as usize,
                mirror.nslots as usize + 1, // a frame size the entry does not resolve to
                mirror.code as usize,
                EnvId(mirror.env),
                (mirror.callee_ic_base, mirror.callee_gic_base),
            );
        }));
        interp.heap.set_ic_bases(old_bases);
        assert!(
            outcome.is_err(),
            "a mirror disagreeing with its entry at the same epoch must fire"
        );
    }

    /// A thin wrapper called BY NAME from native code links like any callee. The interpreter
    /// caches such a callee on the call IC without its arm (it runs the wrapper through the
    /// dispatcher's redirect, ADR-069), and the caller runs interpreted until it tiers, so
    /// the native caller's first calls find that arm-less entry. Taking it for a builtin's
    /// left every later call on the slow path — the link is published only from an entry
    /// that carries the arm — at ~2 100 instructions a call where a linked one is ~400.
    /// Sabotage: return `None` for a closure in `jit_dispatch_call`'s arm-less-hit arm and
    /// this never publishes.
    #[test]
    fn a_thin_wrapper_called_by_name_publishes_its_fast_link() {
        let (mut interp, _, mirror) = tiered_pair(
            "(do (defn wrapped (a x) (+ a x)) \
                 (defn wcaller (n acc) (if (= n 0) acc (wcaller (- n 1) (wrapped acc 1)))))",
            "(wcaller 20000 0)",
            value::intern("wrapped"),
        );
        assert_eq!(mirror.argc, 2);
        assert_eq!(
            interp.eval_str("(wcaller 20000 5)").expect("run").as_int(),
            Some(20005)
        );
    }
}
