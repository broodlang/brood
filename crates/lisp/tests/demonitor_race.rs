//! KI-213: a `demonitor` racing the death path's push. The death path took a target's
//! watchers under `MONITORS`, released it, then pushed each `[:down …]`; a `demonitor` plus
//! a zero-timeout flush in that gap found nothing and the down landed after — 31 leaks in
//! 80 000 rounds of this shape, 169 with the gap widened on purpose. The local pushes now
//! happen under the same hold of the lock as the take.
//!
//! This is the root-process form of `tests/robustness_limits_test.blsp` "demonitor and a
//! down in flight": inside the test RUNNER the same 80 000 rounds never saw the race (the
//! runner's own load hides the window), while a script reproduced it every time, so the
//! guard lives here, where it reds under sabotage.

use brood::Interp;

const STRESS: &str = r#"
(defn round (n leaks)
  (if (= n 0)
    leaks
    (let (me (self)
          t (spawn (receive ([:next from] (send from [:val 1]))))
          m (monitor t))
      (send t [:next me])
      (receive ([:val _] nil) ([:down ^m _ _] nil))
      (demonitor m)
      (receive ([:down ^m _ _] nil) (after 0 nil))
      (sleep 0)
      (round (- n 1) (+ leaks (receive ([:down ^m _ _] 1) (after 0 0)))))))
(defn worker (parent n) (send parent [:leaks (round n 0)]))
(let (me (self) w 40 n 2000)
  (map (range 0 w) (fn (i) (spawn (worker me n))))
  (fold (range 0 w) 0 (fn (acc i) (receive ([:leaks k] (+ acc k))))))
"#;

#[test]
fn a_demonitor_then_flush_never_leaves_a_down_behind() {
    let mut interp = Interp::new();
    let leaked = interp.eval_str(STRESS).expect("the stress evaluates");
    assert_eq!(
        interp.print(leaked),
        "0",
        "downs left behind after demonitor + flush across 80 000 rounds"
    );
}
