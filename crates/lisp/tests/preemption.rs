//! Reduction-counted preemption (docs/scheduler.md stage 4 / ADR-018): a
//! CPU-bound process with no `receive` must not monopolise its worker. We pin the
//! pool to ONE worker so an infinite-loop hog and a responder must share a single
//! core — only preemption lets the responder run at all.
//!
//! This is its own test binary, so the process-wide `set_max_parallel(1)` is
//! isolated from the other test binaries (which run with the default ≈`nproc`).
//!
//! The test is bounded by a `receive` timeout: if preemption regresses, the
//! responder is starved and the root's `(after 3000 …)` fires, so the assertion
//! fails with `:starved` instead of hanging CI.

use brood::{process, Interp};

#[test]
fn cpu_bound_process_does_not_starve_peers_on_one_worker() {
    process::set_max_parallel(1);
    let mut interp = Interp::new();
    // On one worker: an infinite hog (never returns, never receives) plus a
    // responder. Without preemption the worker is captured by `hog` forever and
    // `responder` never runs; with preemption `hog` yields every reduction budget,
    // so `responder` gets the worker, replies, and the root sees `:pong`.
    let prog = r#"
        (def me (self))
        (defn hog () (hog))
        (defn responder (parent) (receive (:ping (send parent :pong))))
        (spawn (hog))
        (def r (spawn (responder me)))
        (send r :ping)
        (receive (:pong :alive) (after 3000 :starved))
    "#;
    let v = interp.eval_str(prog).expect("program errored");
    assert_eq!(
        interp.print(v),
        ":alive",
        "responder was starved by the CPU-bound hog — preemption is not working"
    );
}
