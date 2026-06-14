//! Reduction-counted preemption (docs/scheduler.md stage 4 / ADR-018): a
//! CPU-bound process with no `receive` must not monopolise its worker. We pin the
//! pool to ONE worker so an infinite-loop hog and a responder must share a single
//! core — only preemption lets the responder run at all.
//!
//! This is its own test binary, so the process-wide `set_max_parallel(1)` +
//! `set_test_no_workers(true)` are isolated from the other test binaries (which run
//! the real pool at the default ≈`nproc`).
//!
//! **Deterministic, not wall-clock-bounded.** Instead of starting OS worker threads
//! and waiting (a real `(after …)` timeout flakes when load starves the process of
//! CPU), we start *no* workers and drive scheduling **quanta by hand**
//! (`test_drive_quanta`). The bound is then in *work units* — a starved responder is
//! detected as "did not run within N quanta", which load cannot perturb. With a FIFO
//! run queue + preemption: quantum 1 runs the hog (preempted after its reduction
//! budget → re-enqueued at the back), quantum 2 runs the responder (replies + exits).

use brood::{process, Interp};

#[test]
fn cpu_bound_process_does_not_starve_peers_on_one_worker() {
    process::set_max_parallel(1);
    // Drive the scheduler ourselves (no OS worker threads) so the test is bounded by
    // scheduling quanta — deterministic work units — never wall-clock.
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    // Spawn an infinite CPU hog and a responder, send :ping — but DON'T block the root
    // (no `receive` here), so the spawner returns immediately, leaving both green
    // processes queued on worker 0 and :ping in the responder's mailbox.
    let setup = r#"
        (def me (self))
        (defn hog () (hog))
        (defn responder (parent) (receive (:ping (send parent :pong))))
        (spawn (hog))
        (def r (spawn (responder me)))
        (send r :ping)
        :ok
    "#;
    interp.eval_str(setup).expect("setup errored");

    // Run a bounded number of quanta on a *separate* thread, so `run_one`'s per-quantum
    // ctx install doesn't disturb the root's ctx / scheduling TLS on this thread. 64 is
    // far more than the 2 a correct scheduler needs; a scheduler that let the hog
    // monopolise would never run the responder, so the poll below would see no :pong no
    // matter how many quanta we run.
    let ran = std::thread::spawn(|| process::test_drive_quanta(64))
        .join()
        .unwrap();
    assert!(ran > 0, "expected to run at least one quantum, ran {ran}");

    // Non-blocking poll (`after 0`, no wall-clock wait): did the responder's :pong reach
    // the root within those quanta?
    let v = interp
        .eval_str("(receive (:pong :alive) (after 0 :starved))")
        .expect("poll errored");
    process::set_test_no_workers(false);
    assert_eq!(
        interp.print(v),
        ":alive",
        "responder was starved by the CPU-bound hog — preemption / fair scheduling is not working"
    );
}
