//! End-to-end **multi-process RUNTIME collector** (ADR-091 Stage 4 auto-arming). Its own
//! test binary so the `BROOD_RT_GC_FLOOR` env `OnceLock` is read with this low value (it
//! caches on first use; a shared binary would race other tests). The collector itself is
//! always on — a shared runtime always reclaims via the generational state machine.
//!
//! Real green processes keep the runtime genuinely *shared* (so single-process compaction
//! can't run and the multi-generation state machine drives instead) while the root
//! hot-reloads a global hundreds of times. Under that load the collector must age the
//! region and migrate the live globals into the fresh generation *underneath the running
//! workers*, without ever serving freed or stale code. The invariant `(f 0) == 0` holds
//! across every redefinition (`f` is always `(fn (x) (* x i))`), so each worker's summed
//! result must be exactly 0 — any use-after-free / stale-cache miscompile would corrupt it
//! or crash.
//!
//! Note on *freeing*: whether a whole generation is fully drained and freed is inherently
//! timing-dependent — it requires every live process to become quiescent w.r.t. the
//! draining generation (a process actively looping in old code pins it, exactly Erlang's
//! purge condition). The deterministic proof that draining + freeing works lives in the
//! two-heap cycle tests in `runtime_collector.rs`; here we assert the reliably-reproducible
//! properties: the auto-collector *fires* (ages + migrates) under real concurrency, and it
//! never miscompiles.

use std::sync::LazyLock;

use brood::Interp;

static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

/// Lower the RUNTIME churn floor so aging triggers within a few dozen redefinitions
/// (the collector is always on). Must run before the first heap op (the flag caches once).
fn arm_multigen() {
    // SAFETY: set at test start, before any thread reads this env var.
    unsafe {
        std::env::set_var("BROOD_RT_GC_FLOOR", "48");
    }
}

#[test]
fn multigen_ages_and_migrates_under_live_workers_without_miscompiling() {
    arm_multigen();
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();

    // Six workers each spin a long compute loop over the hot-reloaded `f` (always
    // `(* x i)`, so `(f 0)` is invariably 0), then report their sum to the root. They
    // stay alive across the churn — keeping the runtime shared so the multi-generation
    // path drives — and their fresh `f` lookups exercise migrated code mid-flight.
    interp.eval_str("(def root (self))").expect("root pid");
    interp.eval_str("(def f (fn (x) (* x 0)))").expect("seed f");
    interp
        .eval_str("(def spin (fn (n a) (if (= n 0) a (spin (- n 1) (+ a (f 0))))))")
        .expect("define spin");
    interp
        .eval_str(
            "(def boot \
               (fn (i) (if (= i 6) :ok \
                 (do (spawn (fn () (send root (spin 120000 0)))) (boot (+ i 1))))))",
        )
        .expect("define boot");
    interp.eval_str("(boot 0)").expect("boot workers");

    // Root-driven churn: each iteration is a separate top-level evaluation (so the root
    // holds no Brood stack pinning a generation between them). Crosses the low RUNTIME-GC
    // floor repeatedly, so the collector ages + migrates while the workers run.
    for i in 0..400 {
        interp
            .eval_str(&format!("(def f (fn (x) (* x {i})))"))
            .expect("redefine f");
    }

    // Join all six workers; each sent its accumulated sum, which must be exactly 0 —
    // proving no worker ever ran freed or stale code across all the aging + migration.
    let sum = interp
        .eval_str(
            "(let (gather (fn (k a) (if (= k 0) a (gather (- k 1) (+ a (receive (m m))))))) \
               (gather 6 0))",
        )
        .expect("gather results");
    assert_eq!(
        interp.print(sum),
        "0",
        "every worker's (f 0) sum is 0 across all reloads — no freed/stale code was run",
    );

    // `f` is intact after all the churn, aging and migration.
    {
        let r = interp.eval_str("(f 7)").unwrap();
        assert_eq!(
            interp.print(r),
            (7 * 399).to_string(),
            "f computes correctly"
        );
    }

    // The auto-collector fired end-to-end: it aged (and migrated the live globals into)
    // at least one fresh generation under the running workers.
    let aged = interp.heap.runtime_aged_count();
    assert!(
        aged >= 1,
        "expected the multi-process collector to age ≥1 generation under load, got {aged}",
    );
}
