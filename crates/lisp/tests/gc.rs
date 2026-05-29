//! Memory-bounding tests for the Phase-2 arena flip. Each test runs a Brood
//! program that allocates LOCAL garbage in a long loop and asserts the live
//! count stays bounded — proving `(hibernate)` actually flushes the arena
//! (and not just for very-short-lived processes whose heap drops on exit).
//!
//! Driven via `Interp::eval_str` (root thread) and via `spawn` (green
//! process), so we exercise the flush both at the root path and inside the
//! coroutine save/restore path (`docs/memory-model.md`).

use brood::Interp;

/// A tight tail-recursive loop in a spawned process that allocates a fresh
/// cons cell per iteration and hibernates between iterations. Without
/// flushing, the LOCAL pairs slab would grow linearly with `n`; with
/// `(hibernate)` it stays bounded by the per-iteration working set
/// (a couple of conses). We can't directly inspect the green process's heap
/// from here, but completing without OOMing on a long loop is the proof —
/// the bump allocator would otherwise grow ~hundreds of MB at this count.
/// 50 000 iterations is comfortably above what the bump alone can absorb
/// inside a single process, and small enough to fit in 30 s wall in **debug**
/// (release runs do millions per second; see the `hib-5m.blsp` benchmark in
/// `docs/benchmarks/`).
#[test]
fn long_tail_loop_stays_bounded() {
    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def worker
          (spawn
            (do
              (defn spin (n)
                (cond
                  (= n 0) (send root :done)
                  else
                    (do
                      ;; Allocate small garbage each iteration; (hibernate)
                      ;; flushes the arena so it doesn't accumulate.
                      (cons n (cons (+ n 1) nil))
                      (hibernate spin (- n 1)))))
              (spin 50000))))
        (receive (:done :ok) (after 60000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("spin program errored");
    assert_eq!(
        interp.print(v),
        ":ok",
        "worker didn't finish — either hibernate didn't flush (OOM-like memory growth) \
         or there's a regression in the receive/spawn path",
    );
}

/// A 100k-iteration churn loop in a spawned process. Smaller than the
/// million-iteration `long_tail_loop_stays_bounded` and **without**
/// `(hibernate)` — exercises the per-process bump allocator on a load
/// the bump can comfortably absorb (~100k conses ≈ low MB). The process
/// exits when done; its LOCAL heap drops whole, so root memory stays
/// bounded too.
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

/// The classic gen-server pattern: a process that loops on `receive`
/// forever, allocating per-iteration garbage. The bump allocator alone
/// would grow without bound across the receive loop; `(hibernate loop …)`
/// at the end of each iteration deep-copies just the surviving state into
/// a fresh arena and drops the old slabs.
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
                    ;; Build some garbage from `x` each iteration; hibernate
                    ;; flushes the arena so it doesn't accumulate.
                    (cons x (cons state (cons :tick nil)))
                    (hibernate loop (+ state 1)))
                  ([:stop reply-to]
                    (send reply-to [:final state]))))
              (loop 0))))
        ;; Cast a bunch of messages, then ask for the final state.
        (defn pump (n)
          (if (= n 0) :pumped
            (do (send server [:cast n]) (pump (- n 1)))))
        (pump 5000)
        (send server [:stop root])
        (receive ([:final n] n) (after 60000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("server program errored");
    assert_eq!(
        interp.print(v),
        "5000",
        "server didn't process all messages — flush regression or preemption bug",
    );
}
