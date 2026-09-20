//! **A monitored process must not cost more to kill than an unmonitored one, however many
//! monitors the watcher holds.**
//!
//! Until 2026-09-20 the monitor table was indexed by TARGET only, and a watcher's death or a
//! `demonitor` walked the whole of it. A supervisor holding 200 000 monitored children then
//! paid 1.2 ms per child death — 200 000 entries walked per kill — against 0.7 µs for an
//! unmonitored child: 1 500×, O(n²) for the fleet, and every one of `exit :kill`'s cost on
//! the process-lifecycle probe. `process/monitor.rs` now keeps the watcher-side index Erlang
//! keeps, so both paths touch only their own entries.
//!
//! Pinned as a RATIO, not a wall time: `n` parked children killed with the parent holding
//! one monitor on each of them, against the same `n` killed unmonitored. Indexed, the two
//! differ by the `[:down …]` delivery (measured 1.5×); walked, by three orders of magnitude.
//! Sabotage-verified: the pre-index full walk in `sweep_dead_watcher` reads ~40× at this `n` (1 500× at 200k).

use brood::Interp;

#[test]
fn killing_a_monitored_process_costs_no_more_than_an_unmonitored_one() {
    let mut interp = Interp::new();
    let prog = r#"
        (def n 20000)
        (defn parked () (receive ([:go] nil)))
        (defn spawn-hold (i acc) (if (>= i n) acc (spawn-hold (+ i 1) (cons (spawn (parked)) acc))))
        (defn live () (let (s (%sched-stats)) (- (get s :spawned) (get s :exited))))
        (defn wait-dead (base) (if (> (live) base) (do (sleep 1) (wait-dead base)) nil))
        (defn kill-all (l) (if (empty? l) nil (do (exit (first l) :kill) (kill-all (rest l)))))
        (defn mon-all (l) (if (empty? l) nil (do (monitor (first l)) (mon-all (rest l)))))
        (defn timed-kill (kids base)
          (let (t0 (os/now-ns))
            (kill-all kids)
            (wait-dead base)
            (- (os/now-ns) t0)))
        (def base (live))
        ;; Interleave so drift cannot favour one arm: plain, monitored, plain, monitored.
        (def p1 (timed-kill (spawn-hold 0 nil) base))
        (def m1 (let (k (spawn-hold 0 nil)) (mon-all k) (timed-kill k base)))
        (def p2 (timed-kill (spawn-hold 0 nil) base))
        (def m2 (let (k (spawn-hold 0 nil)) (mon-all k) (timed-kill k base)))
        [(math/min p1 p2) (math/min m1 m2)]
    "#;
    let v = interp.eval_str(prog).expect("the probe ran");
    let s = interp.print(v);
    let nums: Vec<f64> = s
        .trim_matches(|c| c == '[' || c == ']')
        .split_whitespace()
        .map(|t| t.parse::<f64>().expect("an integer nanosecond count"))
        .collect();
    let (plain, monitored) = (nums[0], nums[1]);
    let ratio = monitored / plain;
    eprintln!("kill 20k: plain {plain} ns, monitored {monitored} ns, ratio {ratio:.2}");
    assert!(
        ratio < 8.0,
        "killing 20 000 monitored children cost {ratio:.1}× the unmonitored fleet — the \
         watcher-side monitor index is gone and each death walks the whole table again"
    );
}
