//! The suspend-host latch: a JIT-native arm that encloses a PARKING `receive` must latch
//! itself off the native tier.
//!
//! Why: a process suspended at `receive` under a native frame cannot be state-captured —
//! the mailbox's capture path needs a clean all-VM stack — so it dirty-blocks its whole
//! OS worker thread (§7.4) and can never migrate. The `%receive` fence keeps a chunk that
//! calls `%receive` DIRECTLY out of the JIT subset, but an arm whose receive sits one call
//! down (`(fn (x) (+ x (inner)))` where `inner` receives) is in the subset and — as a
//! closure arm — exempt from the profitability gate, so it lowers. That shape is every
//! worker whose helper blocks: found during the §7.1 step 2 experiment, where
//! `live_migration`'s 12-way load harness measured 28/36 liveness failures without the
//! latch and 0/36 with; the latch stays because the closure-exempt class lowers today.
//!
//! The observable is [`process::dirty_receive_block_count`]: a park under a native frame
//! increments it; a captured park does not. Phase 1 drives rounds until one dirty block
//! is seen (proof the arm ran native and the flag path fired); phase 2 asserts the count
//! then stays (near-)flat — the latch put the arm back on the VM, so later parks capture.
//!
//! **Currently vacuous by default (2026-08-30), and each test says so on stderr.** On
//! today's tree the receive-hosting shapes these tests build do not lower: the
//! spill-reserve rule gives a single-non-tail-call arm no slots (measured as load-bearing
//! — see `jit_spill_reserve`), and a `def`-named closure is gate-bailed like any named
//! defn. The latch itself was validated when the §7.1 step 2 experiment admitted these
//! shapes (dirty park → `suspend-latched` → later parks captured; `live_migration`'s
//! 12-way harness 28/36 liveness failures without it, 0/36 with) and stays as protection
//! for any future admission — partial lowering, or a wider subset. If phase 1 fires again
//! under a future tree, these tests arm themselves and the phase-2 assertions bite; the
//! `42` round-trip assertions check parked-receive correctness either way.

use brood::eval::compile::{tier_ceiling, Tier};
use brood::{process, Interp};

/// True when this run can never observe a dirty block, so the phase-1 hunt is pointless.
///
/// A dirty block is a park under a NATIVE frame; with the ceiling below [`Tier::Native`]
/// (`BROOD_TIER=0|1`, `BROOD_VM=0`, `BROOD_NO_JIT=1`) no arm ever lowers, so phase 1's
/// forty rounds — a 150 ms sleep plus a 50 000-iteration re-heat each — can only walk to
/// the vacuous exit the long way. On the tree-walker that walk is the whole 120 s nextest
/// budget: the `BROOD_VM=0` differential job timed out on this file from 2026-08-30 06:57Z
/// until this guard, while the same test passed vacuously in the default job. Asks the
/// runtime rather than the environment (the ceiling has three spellings — see
/// `tier_ceiling`).
fn ceiling_below_native() -> bool {
    if tier_ceiling() < Tier::Native {
        eprintln!(
            "jit_suspend_latch: tier ceiling is {:?} — nothing can lower; vacuous",
            tier_ceiling()
        );
        return true;
    }
    false
}

#[test]
fn an_arm_hosting_a_parked_receive_latches_and_later_parks_capture() {
    if ceiling_below_native() {
        return;
    }
    let mut interp = Interp::new();
    let setup = r#"
        (def root (self))
        ;; `inner` receives (its own chunk is fenced off the JIT); `host` is a CLOSURE
        ;; called directly — closures are exempt from the profitability gate and its
        ;; receive is one call down (past the `%receive` fence), so it tiers native.
        ;; NOT a `reduce`/HOF shape: a Rust builtin HOF nests a `vm_apply` driver under
        ;; which a receive can never capture (it dirty-blocks with NO gateway, token 0),
        ;; so that shape would keep the count climbing with the latch working perfectly.
        (defn inner () (receive (v v)))
        (def host (fn (x) (+ x (inner))))
        (defn hot (i acc) (if (= i 0) acc (hot (- i 1) (+ acc (host 0)))))
        (defn feed (k) (when (> k 0) (do (send (self) 1) (feed (- k 1)))))
        ;; One round: a worker parks inside host's receive (the sleep gives it time to
        ;; reach the park), then is woken. 42 iff the park/resume round-tripped.
        (defn round ()
          (let (w (spawn (send root [:r (host 41)])))
            (do (sleep 150)
                (send w 1)
                (receive ([:r n] n) (after 30000 :timeout)))))
    "#;
    interp.eval_str(setup).expect("setup errored");
    // Tier `host` hot with pre-filled messages, so its receives never park here.
    interp.eval_str("(feed 200000)").expect("feed errored");
    let v = interp.eval_str("(hot 200000 0)").expect("hot errored");
    assert_eq!(interp.print(v), "200000");

    // Phase 1: rounds until one park dirty-blocks — i.e. until the step is native and a
    // worker parks under it. Tiering is backgrounded, so keep the arm hot between tries.
    let mut saw_dirty = false;
    let dirty = process::dirty_receive_block_count();
    for _ in 0..40 {
        let v = interp.eval_str("(round)").expect("round errored");
        assert_eq!(interp.print(v), "42", "a parked worker resumed wrongly");
        if process::dirty_receive_block_count() > dirty {
            saw_dirty = true;
            break;
        }
        interp
            .eval_str("(do (feed 50000) (hot 50000 0))")
            .expect("re-heat errored");
    }
    if !saw_dirty {
        // The step never went native (no-JIT build, BROOD_NO_JIT/BROOD_TIER<2, or the
        // machine never tiered it): every park captured, which is the healthy baseline —
        // there is nothing to latch. The latch behaviour is covered by the default-config
        // suite run.
        eprintln!("jit_suspend_latch: no dirty block observed — nothing tiered; vacuous");
        return;
    }
    // Settling: a park under a native→native chain latches ONE arm per occurrence — the
    // innermost-alive gateway's — and converges over successive parks (the spawn thunk
    // and `host` each take a round). Drive the convergence out of the asserted window so
    // phase 2 measures the steady state, not the walk to it.
    for _ in 0..6 {
        let v = interp.eval_str("(round)").expect("settling round errored");
        assert_eq!(interp.print(v), "42", "a parked worker resumed wrongly");
    }
    // Phase 2: the dirty blocks latched every hosting arm onto the VM, so later parks
    // capture. A small tolerance absorbs the self-limiting races (an in-flight background
    // compile or inline upgrade landing over the latch is re-latched by the next park).
    let before = process::dirty_receive_block_count();
    for _ in 0..12 {
        let v = interp.eval_str("(round)").expect("round errored");
        assert_eq!(interp.print(v), "42", "a parked worker resumed wrongly");
    }
    let extra = process::dirty_receive_block_count() - before;
    assert!(
        extra <= 3,
        "the suspend latch is not holding: {extra} of 12 post-latch parks still \
         dirty-blocked their worker (a native frame kept enclosing the receive)"
    );
}

/// The long-lived-process variant — the gen-server shape the latch exists for. A fresh
/// process (the test above) respects a latch because its first dispatch re-reads
/// `jit_code` and sees `BAILED`; a LONG-LIVED process that already holds a populated
/// [`FastLink`] to the arm keeps entering the latched native through the mirror's hit
/// path (and the raw load in JIT'd callers), which never consults `jit_code` — so without
/// `vm_fast_link_clear_site` in the fast-link gateway's latch path, every later park in
/// that process dirty-blocks forever. Latent on today's tree for the same reason the
/// module doc gives (the shapes here no longer lower, so this runs vacuously and says so);
/// under the §7.1 step 2 window, the un-shed fast link measured 12 of 12 post-latch parks
/// still dirty.
#[test]
fn a_long_lived_process_sheds_its_stale_fast_link_after_the_latch() {
    if ceiling_below_native() {
        return;
    }
    let mut interp = Interp::new();
    let setup = r#"
        (def root (self))
        (defn inner () (receive (v v)))
        (def host (fn (x) (+ x (inner))))
        (defn hot (i acc) (if (= i 0) acc (hot (- i 1) (+ acc (host 0)))))
        (defn feed (k) (when (> k 0) (do (send (self) 1) (feed (- k 1)))))
        ;; ONE worker serves every round, so its fast link at the [:go] call site
        ;; survives between rounds — the state the latch must shed.
        (defn wloop ()
          (receive
            ([:heat k] (do (feed k) (hot k 0) (send root [:heated]) (wloop)))
            ([:go]     (do (send root [:r (host 41)]) (wloop)))
            ([:stop]   :done)))
        (def w (spawn (wloop)))
        (defn round ()
          (do (send w [:go])
              (sleep 150)
              (send w 1)
              (receive ([:r n] n) (after 30000 :timeout))))
    "#;
    interp.eval_str(setup).expect("setup errored");
    // Tier `host` inside the worker itself, with its receives pre-fed so none park.
    interp
        .eval_str("(send w [:heat 200000])")
        .expect("heat send errored");
    let v = interp
        .eval_str("(receive ([:heated] :ok) (after 60000 :timeout))")
        .expect("heat wait errored");
    assert_eq!(interp.print(v), ":ok", "worker never finished heating");

    // Phase 1: rounds until one park dirty-blocks (host native, worker parked under it).
    let mut saw_dirty = false;
    let dirty = process::dirty_receive_block_count();
    for _ in 0..40 {
        let v = interp.eval_str("(round)").expect("round errored");
        assert_eq!(interp.print(v), "42", "the parked worker resumed wrongly");
        if process::dirty_receive_block_count() > dirty {
            saw_dirty = true;
            break;
        }
        interp
            .eval_str("(send w [:heat 50000])")
            .expect("re-heat send errored");
        interp
            .eval_str("(receive ([:heated] :ok) (after 60000 :timeout))")
            .expect("re-heat wait errored");
    }
    if !saw_dirty {
        eprintln!("jit_suspend_latch: no dirty block observed — nothing tiered; vacuous");
        interp.eval_str("(send w [:stop])").ok();
        return;
    }
    // Settling: the latch converges one arm per park; give the chain a few rounds.
    for _ in 0..6 {
        let v = interp.eval_str("(round)").expect("settling round errored");
        assert_eq!(interp.print(v), "42", "the parked worker resumed wrongly");
    }
    // Phase 2: the SAME worker's later parks must capture — the latch stored BAILED and
    // the gateway shed the site's fast link, so nothing re-enters the latched native.
    let before = process::dirty_receive_block_count();
    for _ in 0..12 {
        let v = interp.eval_str("(round)").expect("round errored");
        assert_eq!(interp.print(v), "42", "the parked worker resumed wrongly");
    }
    let extra = process::dirty_receive_block_count() - before;
    interp.eval_str("(send w [:stop])").ok();
    assert!(
        extra <= 3,
        "the stale fast link is not being shed: {extra} of 12 post-latch parks in the \
         same long-lived process still dirty-blocked its worker"
    );
}
