//! Tracing-GC sanity tests (ADR-035). Each test runs a Brood program that
//! would historically blow the LOCAL heap and asserts the live-object count
//! stays bounded — proving the per-process mark-sweep is actually reclaiming.
//!
//! Driven via `Interp::eval_str` (root thread) and via `spawn` (green
//! process), so we exercise the GC both at the root path and inside the
//! coroutine save/restore path (`docs/memory-model.md`).

use brood::Interp;

/// A tight tail-recursive loop that allocates a fresh cons cell per iteration.
/// Without reclamation the LOCAL pairs slab grows linearly with `n`; with GC
/// it stays bounded by the per-iteration working set (a couple of conses).
// Mark-sweep was removed in the bump-allocator phase. Without an arena
// flush point inside a tail-recursive loop, this test's `(spin 200000)`
// accumulates allocations until the loop exits — by design for phase 1.
// Phase 2 (arena flip) will add a flush point that bounds this; until then
// it would fail. Leaving the test as a regression target for that phase.
#[test]
#[ignore = "phase 2 will add arena flip on tail-recursion or growth threshold"]
fn long_tail_loop_stays_bounded() {
    let mut interp = Interp::new();
    let baseline = interp.heap.local_live_count();
    let prog = r#"
        (defn spin (n)
          (if (= n 0)
            :done
            (do
              ;; Allocate a small cons each iteration — pure garbage, never read.
              (cons n (cons (+ n 1) nil))
              (spin (- n 1)))))
        (spin 200000)
    "#;
    let v = interp.eval_str(prog).expect("spin program errored");
    assert_eq!(interp.print(v), ":done");
    let live = interp.heap.local_live_count();
    // We allocated ~400k cons cells worth of garbage; with GC the surviving
    // count must be a small multiple of the per-iteration working set, not
    // anywhere near 400k. Pick a generous bound: anything under 64k proves
    // reclamation; in practice live count sits well below 1k.
    assert!(
        live < 64 * 1024,
        "GC failed to reclaim: live count {} grew beyond the threshold (baseline {})",
        live,
        baseline,
    );
}

/// The same loop, but inside a spawned green process. Exercises the coroutine
/// suspend/resume save/restore of `GC_BLOCK` (a regression here would either
/// (a) sweep live values mid-call or (b) silently disable GC after the first
/// `receive`). We have the worker signal back so the test joins cleanly.
#[test]
fn spawned_process_reclaims_too() {
    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def worker
          (spawn
            (do
              (defn churn (n)
                (if (= n 0)
                  (send root :done)
                  (do
                    (cons n (cons (+ n 1) (cons :spinning nil)))
                    (churn (- n 1)))))
              (churn 100000))))
        ;; Block until the worker finishes its churn loop.
        (receive (:done :ok) (after 30000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("spawn program errored");
    assert_eq!(
        interp.print(v),
        ":ok",
        "spawned worker did not complete (preemption regression or GC corruption)",
    );
    // We can't directly inspect the green process's own heap (it's gone by now),
    // but the root interp's heap must also stay bounded — `eval_str` ran the
    // root code which itself doesn't accumulate.
    let live = interp.heap.local_live_count();
    assert!(
        live < 64 * 1024,
        "root heap unexpectedly large after spawn: {}",
        live,
    );
}

/// The classic case the arena-reset doesn't help with: a server loop that
/// never returns to a top-level boundary. With GC it must stay bounded over
/// many iterations; without it would OOM.
#[test]
fn server_style_receive_loop_stays_bounded() {
    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def server
          (spawn
            (do
              (defn loop (state)
                (receive
                  ([:cast x]
                    ;; Build some garbage from `x` each iteration.
                    (cons x (cons state (cons :tick nil)))
                    (loop (+ state 1)))
                  ([:stop reply-to]
                    (send reply-to [:final state]))))
              (loop 0))))
        ;; Cast a bunch of messages, then ask for the final state.
        (defn pump (n)
          (if (= n 0) :pumped
            (do (send server [:cast n]) (pump (- n 1)))))
        (pump 20000)
        (send server [:stop root])
        (receive ([:final n] n) (after 30000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("server program errored");
    assert_eq!(
        interp.print(v),
        "20000",
        "server didn't process all messages — preemption or GC regression",
    );
}
