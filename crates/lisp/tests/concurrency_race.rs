//! Regression test for the KI-1 / KI-2 multi-thread scheduler race
//! (`docs/known-issues.md`, `docs/concurrency-v2.md` §6 — the acceptance bar
//! that asked for exactly this to be encoded so the race can't silently return).
//!
//! The race was a use-after-GC / stale-handle class fault that surfaced as a slab
//! out-of-bounds panic in the GC copy phase (`heap.rs` `flush_*`: a LOCAL handle
//! reachable from the GC roots whose index was past the live source slab) and,
//! on other runs, as silently wrong results. It was killed in series by three
//! changes on 2026-05-29: removing the kernel supervisor's shared rooting state
//! (`e3d3a0d`), the bump-only allocator with no slot reuse (`f90f0de`), and
//! per-worker pinned queues that end cross-thread coroutine migration (`2abf05e`).
//!
//! The reconstruction: a coordinator spawns N workers that each allocate heavily
//! (build an n-entry CHAMP map — heavy small-allocation churn that drives each
//! worker's per-process GC), `send` a result back across the per-process heap
//! boundary (deep-copy: exercises `to_message`/`from_message` + `promote`/freeze),
//! while a separate writer process rebinds a global **underfoot** (RUNTIME-region
//! churn + global-table writes racing every worker's concurrent global lookup).
//! The workers' returned count is independent of the racing global's value, so the
//! parallel total is deterministic (`k * n`) and we assert it over many trials —
//! a mismatch (silent corruption) or a panic (slab OOB) both fail the test.
//!
//! Runs on the default multi-worker scheduler (worker pool ≈ `nproc`), so this is
//! genuinely concurrent. Manually re-validated under `BROOD_GC_STRESS=1
//! BROOD_GC_VERIFY=1` (collect + live-graph verify at every safepoint) with no
//! trip; this baseline test keeps a standing guard in the normal suite.

use brood::Interp;

#[test]
fn fanout_with_concurrent_global_rebind_matches_serial() {
    let mut interp = Interp::new();
    // 120 trials × 10 workers each building a 500-entry map, with a concurrent
    // global-rebinding writer per trial. Sized to finish well under the 2-min
    // nextest cap in a debug-assertions build while still applying real pressure.
    let prog = r#"
        (def root (self))
        (def *spin* 0)

        ;; Workers read *spin* every iteration (the racing read against the writer's
        ;; `def`), but it only perturbs the map's *values* — the key set is 0..n-1,
        ;; so the count is exactly n regardless of how the rebind race interleaves.
        (defn tally (n)
          (reduce (fn (a x) (assoc a x (mod (+ x *spin*) 7))) {} (range n)))

        ;; The writer: churn the shared global table concurrently with the workers.
        (defn churn (n)
          (when (> n 0) (do (def *spin* n) (churn (- n 1)))))

        ;; Fan k workers out (each sends its map's size back), with a writer racing
        ;; them, then collect all k results.
        (defn fan (k n)
          (do
            (spawn (churn 600))
            (dotimes (b k) (spawn (send root [:r (count (tally n))])))
            (reduce (fn (acc _) (receive ([:r c] (+ acc c)))) 0 (range k))))

        ;; Each trial's parallel total must equal the serial k*n. Tail-recursive
        ;; (the test body runs in a small green-process stack).
        (defn trials (m k n)
          (cond
            (= m 0) :ok
            else (let (got (fan k n))
                   (if (= got (* k n))
                       (trials (- m 1) k n)
                       [:mismatch got (* k n)]))))

        (trials 120 10 500)
    "#;
    let v = interp.eval_str(prog).expect("fan-out race program errored");
    assert_eq!(
        interp.print(v),
        ":ok",
        "parallel fan-out total diverged from the serial k*n (silent GC corruption) \
         or a worker died — the KI-1/KI-2 scheduler race may have returned",
    );
}
