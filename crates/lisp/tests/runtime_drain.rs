//! Multi-process RUNTIME-drain edge cases (ADR-091). Its own test binary so the global
//! process `REGISTRY` holds only this test's processes — a leaked/parked worker here must
//! not pollute the `live_pids()` union another drain test in the same binary computes.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use brood::Interp;
use brood::process;

static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

/// Stage 5 — **a parked process clean of the draining generation doesn't block it**. A
/// process suspended in `receive` can't report its own liveness for a drain armed *after*
/// it parked (it never reaches a safepoint). The drain coordinator therefore inspects a
/// parked process's captured continuation directly and acks it iff it's clean. Here a worker running gen-1 code parks while a drain of
/// gen 0 is armed: it is clean of gen 0 and must not stall the drain, even though it will
/// never run again to report for itself.
#[test]
fn parked_process_clean_of_the_draining_gen_does_not_block_it() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Seed gen-0 code, then age + migrate so `f`/`root` live in gen 1 and gen 0 holds
    // only superseded (unreferenced) originals.
    interp
        .eval_str(
            r#"
            (defn f (x) (* x 2))
            (def root (self))
            "#,
        )
        .expect("seed gen-0 code");
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp.heap.migrate_live_globals(0);
    interp.heap.collect(&mut [], &mut []);
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "the root is clean of gen 0 after migration",
    );

    // Spawn a worker *after* aging — its inline thunk promotes into gen 1, so it runs
    // (and parks in `receive`) entirely on gen-1 code: clean of the draining gen 0. It
    // reports its pid in `:ready` so the test can release it afterwards (no leaked
    // permanently-parked process).
    interp
        .eval_str(
            "(spawn (fn () (do (send root [:ready (self)]) (receive (:go (send root :bye)))))) ",
        )
        .expect("spawn a gen-1 worker");
    interp
        .eval_str("(receive ([:ready p] (def worker-pid p)))")
        .expect("await :ready + stash the worker pid");

    // Arm the drain of gen 0; the root acks at its own safepoint (calling gen-1 `f`).
    interp.heap.begin_gen_drain(0);
    interp.eval_str("(f 3)").expect("root safepoint report");
    assert!(
        process::live_pids().len() >= 2,
        "the root and the parked worker are both live",
    );

    // The parked worker never reaches a safepoint to report — but it is clean of gen 0,
    // so the coordinator's parked-process inspection acks it. Without that inspection the
    // worker would pin gen 0 forever; with it, the generation drains.
    let deadline = Instant::now() + Duration::from_secs(5);
    let drained = loop {
        if process::old_gen_drained(&interp.heap) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(
        drained,
        "a parked process clean of the draining generation is inspected and acked \
         (check_process_code-style) — it must not block the drain",
    );

    // Release the worker so it exits (hygiene — no permanently-parked process left).
    interp
        .eval_str("(do (send worker-pid :go) (receive (:bye :ok)))")
        .expect("release the worker");
}

/// A **parked process whose data holds the draining generation** no longer blocks the
/// drain: the parked-process inspector copies that data out (it will not collect while it
/// waits) and acks it. The worker keeps a gen-0 closure and a gen-0 string as the argument
/// of its waiting loop; after the collector cycle it re-enters the loop through the
/// migrated global, so the code it waits in is gen 1 and only its DATA is gen 0 — the
/// shape of any long-lived server holding a callback in its state. Before, it pinned the
/// generation for as long as it waited. Woken afterwards it still has a closure that IS
/// the migrated `f`, that runs, and the text.
#[test]
fn parked_process_whose_data_pins_the_drain_copies_it_out() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    interp
        .eval_str(
            r#"
            (defn f (x) (* x 2))
            (def s "held text")
            (def root (self))
            (defn wait-loop (held)
              (receive
                (:tick (do (send root :ticked) (wait-loop held)))
                (:go (send root [(= (first held) f) ((first held) 21) (nth held 1)]))))
            (def worker (spawn (fn () (wait-loop [f s]))))
            "#,
        )
        .expect("seed gen 0 and start the worker");

    // The collector cycle: age, migrate the globals (wait-loop, f and s among them) into
    // gen 1. Then one tick, so the worker re-enters `wait-loop` through the migrated global
    // and parks in gen-1 code, still holding the gen-0 `f` and `s` in its argument.
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp.heap.migrate_live_globals(0);
    interp
        .eval_str("(do (send worker :tick) (receive (:ticked :ok)))")
        .expect("the worker re-parks in gen-1 code");
    interp.heap.collect(&mut [], &mut []);

    interp.heap.begin_gen_drain(0);
    interp.eval_str("(f 3)").expect("root safepoint report");

    let deadline = Instant::now() + Duration::from_secs(5);
    let drained = loop {
        if process::old_gen_drained(&interp.heap) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(
        drained,
        "the parked worker's gen-0 data is copied out by the inspector and it acks clean — \
         a waiting process must not hold the drain open for as long as it waits",
    );

    let answer = interp
        .eval_str("(do (send worker :go) (receive (reply reply)))")
        .expect("wake the worker");
    assert_eq!(
        interp.print(answer),
        "[true 42 \"held text\"]",
        "the worker's copy is the migrated f (`=` holds), it runs, and the text came across",
    );
}
