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
//! Vacuous (phase 1 never fires, test returns early) only in a configuration where the
//! arm never goes native at all — a no-JIT build or a lowered tier ceiling.

use brood::{process, Interp};

#[test]
fn an_arm_hosting_a_parked_receive_latches_and_later_parks_capture() {
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
