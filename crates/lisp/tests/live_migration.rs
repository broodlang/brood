//! Live process migration under state capture (ADR-100 §7 / §8.4, the §7.6
//! acceptance test). A green process suspended **mid-computation** at `receive` is a
//! captured continuation with no native stack, so the scheduler may resume it on a
//! *different* worker than it suspended on — the migration that corosensei's
//! thread-pinned coroutines could never do (KI-1b).
//!
//! Its own test binary because `set_max_parallel` is process-wide.
//!
//! What it proves, the live-migration analogue of `work_stealing.rs`:
//!   1. **Correctness over many trials.** Each worker builds a deep **non-tail**
//!      recursion (≈`DEPTH` `BcFrame`s), blocks in `receive`, then on resume unwinds
//!      all those frames adding 1 each. The result (`DEPTH + the sent value`) is wrong
//!      or the run crashes if a migrated continuation's frame stack were mis-restored.
//!   2. **Migration actually happened.** `process::migrate_count()` counts capture-mode
//!      processes re-assigned to a different worker on wake — it must be > 0.

use brood::{process, Interp};

#[test]
fn deep_receive_continuations_resume_correctly_across_workers() {
    // A small pool so woken processes contend for workers and migrate.
    process::set_max_parallel(2);

    let mut interp = Interp::new();
    // Define the harness once. `build` is a *top-level* fn (so the spawned thunk
    // `(fn () (work))` stays VM-eligible — capture mode); its non-tail recursion is
    // what fills the frame stack that must survive a cross-worker resume.
    let setup = r#"
        (def root (self))
        (def depth 150)
        (def sent 1000)

        ;; Build `depth` non-tail frames, block at the bottom, then unwind +1 per frame.
        ;; Returns depth + the received value — a value that depends on every captured
        ;; frame being restored correctly after the migration.
        (defn build (n)
          (if (= n 0)
              (receive (v v))
              (+ 1 (build (- n 1)))))

        (defn work () (send root [:r (build depth)]))

        ;; Spawn k workers, collecting their pids (they reach `receive` and suspend with
        ;; an empty mailbox — no message yet), then wake each with a value. Suspending
        ;; before the value arrives is what forces the capture+resume (and the migration).
        (defn launch (k acc)
          (if (= k 0) acc (launch (- k 1) (cons (spawn (work)) acc))))
        (defn notify (pids)
          (when (not (empty? pids))
            (do (send (first pids) sent) (notify (rest pids)))))

        (defn drain (k acc)
          (if (= k 0) acc
              (receive ([:r c] (drain (- k 1) (+ acc c)))
                       (after 30000 :timeout))))

        ;; One burst: launch k workers, wake them, drain k results. Total is k*(depth+sent)
        ;; iff every continuation resumed correctly.
        (defn burst (k)
          (let (pids (launch k []))
            (do (notify pids) (drain k 0))))
    "#;
    interp.eval_str(setup).expect("setup errored");

    let k: i64 = 200;
    let expected = k * (150 + 1000);
    // Run bursts until at least one live migration is observed (bounded), checking
    // correctness every burst. A wrong total = a lost/corrupted continuation.
    let mut migrated = false;
    for _ in 0..40 {
        let v = interp.eval_str(&format!("(burst {})", k)).expect("burst errored");
        let got = interp.print(v);
        assert_eq!(
            got,
            expected.to_string(),
            "a migrated continuation produced the wrong result (lost/corrupted frame stack)"
        );
        if process::migrate_count() > 0 {
            migrated = true;
            break;
        }
    }
    assert!(
        migrated,
        "no live migration observed across {} bursts of {} deep-receive processes — \
         capture-mode processes never resumed on a different worker",
        40, k
    );
}
