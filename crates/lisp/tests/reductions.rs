//! `process-info`'s `:reductions` accounting (Erlang's scheduling unit). Its own test
//! binary so it runs the **real** worker pool — isolated from `preemption.rs`, which
//! drives quanta by hand with the worker pool disabled (`set_test_no_workers`); the two
//! must not share a process or the global pool config collides.

use brood::Interp;

/// `process-info`'s `:reductions` climbs for a process that does work. A CPU-bound
/// worker that grinds a long loop then parks in `receive` must report a positive
/// reduction count — the scheduler accumulates `REDUCTION_BUDGET` per preempted quantum
/// (plus the partial final one). This is the "is this process busy?" signal the observer
/// shows; a count stuck at 0 means the accumulation regressed (e.g. `preempt` refreshing
/// the budget before it's tallied).
#[test]
fn process_info_reports_reductions() {
    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def w (spawn (do
          (defn work (n acc) (if (= n 0) acc (work (- n 1) (+ acc 1))))
          (work 300000 0)
          (receive (_ :ok)))))   ;; park after grinding, so it's stable to query
        (sleep 500)
        (get (process-info w) :reductions)
    "#;
    let v = interp.eval_str(prog).expect("reductions program errored");
    let reds: i64 = interp.print(v).trim().parse().unwrap_or(-1);
    assert!(
        reds > 0,
        "CPU-bound worker should accrue reductions, got {reds} — preempt/run_one \
         reduction accounting regressed",
    );
}
