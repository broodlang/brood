//! Fresh-only work-stealing (docs/concurrency-v2.md §3.2 🟡). An idle worker may
//! steal a process that has **never been resumed** from a backed-up peer's queue
//! and run it itself — the first resume happens on the thief with no saved
//! coroutine stack to migrate, the one migration shape proven safe in §3.1a.
//! Suspended (already-resumed) coroutines are never stolen (KI-1b: cross-thread
//! resume of a deep saved stack smashes return addresses → segfault).
//!
//! Two things to verify, with different reliability profiles:
//!
//!   * **Correctness** — every process runs exactly once and its result is
//!     uncorrupted, even when stolen across worker threads. Each fan-out worker
//!     returns a fixed map size, so the per-burst total is deterministic (`k*n`);
//!     a lost or corrupted process makes the total wrong and fails immediately.
//!     This is the standing guard (the same shape as concurrency_race.rs, now
//!     over the steal path) and must hold on *every* burst.
//!
//!   * **The steal path is live** — `(steal-count)` becomes > 0. Whether a *given*
//!     burst steals is timing-dependent (it needs one worker to empty its queue
//!     while a peer still has fresh backlog), so we don't assert it from one
//!     burst: we keep bursting until a steal is observed, bounded. A two-worker
//!     pool steals readily under fan-out (the first burst usually already does, and
//!     the count climbs every burst), so the chance of zero across the whole bound
//!     is vanishing; a run that exhausts the bound means the steal path is dead.
//!
//! Pinned to two workers (committed before the first spawn) to concentrate the
//! fan-out and make the empty-while-peer-is-busy window frequent. Its own test
//! binary because `set_max_parallel` is process-wide (mirrors
//! pool_resize_after_start.rs). NB: a CPU occupier would be *counterproductive*
//! here — it cycles in its worker's queue (re-enqueued on every preempt), so that
//! worker is never idle and never steals; plain fan-out is what exercises the path.

use brood::{process, Interp};

#[test]
fn idle_worker_steals_fresh_backlog_under_load() {
    process::set_max_parallel(2);

    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def n 40)

        ;; A fan-out worker: build an n-entry map and send its size back. The size
        ;; is exactly n regardless of scheduling, so a burst's total is deterministic.
        (defn work ()
          (send root [:r (count (reduce (fn (a x) (assoc a x x)) {} (range n)))]))

        (defn fan (k) (when (> k 0) (do (spawn (work)) (fan (- k 1)))))

        ;; `after` so a *lost* process fails the test cleanly (the total comes back
        ;; :timeout, tripping the correctness check) instead of blocking forever.
        (defn drain (k acc)
          (if (= k 0) acc
              (receive ([:r c] (drain (- k 1) (+ acc c)))
                       (after 30000 :timeout))))

        ;; Burst (spawn k workers, drain k results) until the scheduler has had to
        ;; steal at least once. Every burst verifies correctness first: a wrong
        ;; total means a process was lost or its heap corrupted while being stolen.
        ;; Bounded so a dead steal-path reports :never-stole rather than looping.
        (defn drive (tries k)
          (let (total (do (fan k) (drain k 0)))
            (if (= total (* k n))
                (if (> (steal-count) 0)
                    :stole
                    (if (= tries 0) :never-stole (drive (- tries 1) k)))
                [:corrupt total (* k n)])))

        (drive 60 500)
    "#;
    let v = interp.eval_str(prog).expect("work-stealing program errored");
    let outcome = interp.print(v);

    // Correctness holds unconditionally — even on a single-core fallback (pool of
    // 1, no peer to steal from) every process must still run exactly once.
    assert!(
        !outcome.starts_with("[:corrupt") && outcome != ":timeout",
        "a process was lost or its heap corrupted while being stolen across \
         worker threads: {outcome}",
    );

    // On a genuinely parallel pool, the steal path must be exercised within the
    // bound. (Skipped on a 1-worker fallback, where stealing is impossible.)
    if process::worker_threads() >= 2 {
        assert_eq!(
            outcome, ":stole",
            "the work-stealing path never fired across 60 bursts of 500 processes \
             (worker_threads = {}) — it appears dead",
            process::worker_threads(),
        );
    }
}
