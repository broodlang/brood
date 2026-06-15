//! Memory-bounding tests for Stage B's automatic copying GC (ADR-055). Each test
//! runs a Brood program that allocates LOCAL garbage in a long loop and asserts it
//! stays bounded — proving the collector fires at the eval safepoint *without* any
//! `(hibernate)` from the author (a depth-1 spawned-process body, where the
//! safepoint runs). Before ADR-055 these loops needed a manual `(hibernate)` flush;
//! automatic GC made that redundant and the primitive was removed.
//!
//! Driven via `Interp::eval_str` (root thread) and via `spawn` (green process), so
//! we exercise collection both at the root path and inside the coroutine
//! save/restore path (`docs/memory-model.md`).

use brood::Interp;

/// A tight tail-recursive loop in a spawned process that allocates a fresh cons
/// cell per iteration. The loop is the spawned-process body, so it runs at the
/// `gc_block_depth() == 1` safepoint where Stage B collects — the LOCAL pairs slab
/// would otherwise grow linearly with `n`. We can't inspect the green process's
/// heap from here, but completing 50 000 iterations without OOMing is the proof:
/// the bump allocator alone would grow ~hundreds of MB at this count. Small enough
/// to fit in 30 s wall in **debug** (release does millions/s).
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
                      ;; Allocate small garbage each iteration; Stage B's automatic
                      ;; GC reclaims it at the safepoint so it doesn't accumulate.
                      (cons n (cons (+ n 1) nil))
                      (spin (- n 1)))))
              (spin 50000))))
        (receive (:done :ok) (after 60000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("spin program errored");
    assert_eq!(
        interp.print(v),
        ":ok",
        "worker didn't finish — either automatic GC didn't reclaim (OOM-like memory \
         growth) or there's a regression in the receive/spawn path",
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

/// The classic gen-server pattern: a process that loops on `receive` forever,
/// allocating per-iteration garbage. The bump allocator alone would grow without
/// bound across the receive loop; the tail-recursive `loop` is the spawned-process
/// body, so Stage B collects each iteration's garbage at the safepoint (no
/// `(hibernate)` needed). Asserts the server still processes every message under
/// that automatic collection across the suspend/resume of `receive`.
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
                    ;; Build some garbage from `x` each iteration; Stage B's
                    ;; automatic GC reclaims it so it doesn't accumulate.
                    (cons x (cons state (cons :tick nil)))
                    (loop (+ state 1)))
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
        "server didn't process all messages — GC or preemption bug across receive",
    );
}

/// `(gc-stats)` observability (Tier-1; `docs/memory-review.md` §7). A tail loop
/// that churns garbage at the depth-1 eval safepoint triggers automatic Stage-B
/// collection; afterwards the per-process counters must reflect it: at least one
/// collection, more objects reclaimed than survive, and a positive threshold.
/// Proves `arena_flip` bumps the counters and `(gc-stats)` reads them back —
/// not just that the keys exist.
#[test]
fn gc_stats_counts_automatic_collections() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; Tail loop at depth 1: each iteration allocates a throwaway list and
        ;; keeps only a small counter, so the Stage-B safepoint collects.
        ;; (`map` realises 200 cons cells — a bare `(range 200)` is a lazy O(1)
        ;; Range value and would churn nothing.)
        (defn churn (n acc)
          (if (= n 0)
              acc
              (let (junk (map inc (range 200)))
                (churn (- n 1) (+ acc (count junk))))))
        (churn 2000 0)
        (gc-stats)
    "#;
    let v = interp.eval_str(prog).expect("churn program errored");
    let stats = interp.print(v);
    // The map prints unordered, so assert on the substrings rather than a fixed
    // shape. A run of 8000×200 garbage well exceeds the GC floor, so the
    // collector must have fired and the counters must have moved off zero.
    let runs = read_field(&stats, ":collections");
    let copied = read_field(&stats, ":copied");
    let reclaimed = read_field(&stats, ":reclaimed");
    let threshold = read_field(&stats, ":threshold");
    assert!(
        runs >= 1,
        "expected ≥1 automatic collection, got {runs} — gc-stats: {stats}"
    );
    assert!(
        reclaimed > 0,
        "young-death churn should reclaim garbage, got {reclaimed} — gc-stats: {stats}"
    );
    // In the **generational** collector `:copied` counts *promotions* (minor:
    // nursery→old; major: old compaction). At the normal GC floor, junk dies in
    // the nursery before a collection, so reclaim dwarfs promotion. But under
    // `BROOD_GC_STRESS=1` a minor fires at *every* safepoint, so anything live for
    // even one safepoint is tenured (premature promotion) — `copied` can then
    // exceed `reclaimed`. So only assert young-death-dominates when not stressing.
    if std::env::var_os("BROOD_GC_STRESS").is_none() {
        assert!(
            reclaimed > copied,
            "young-death churn should reclaim far more than it promotes \
             (reclaimed {reclaimed} vs copied {copied}) — gc-stats: {stats}"
        );
    }
    assert!(
        threshold > 0,
        "threshold should be a positive live-count trigger, got {threshold} — gc-stats: {stats}"
    );
}

/// `(gc-collect)` forces a collection on demand and reports the post-collection
/// stats (Tier-1 observability). Allocate a batch of garbage *without* crossing
/// the GC floor (so no automatic safepoint collection fires — `:collections` is 0),
/// then call `(gc-collect)` and assert it (a) ran exactly the collection we asked
/// for and (b) reclaimed the dead batch. This proves the forced collect is a real
/// collection (not a no-op) and is safe to invoke as a leaf builtin at depth.
#[test]
fn gc_collect_forces_a_collection() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; Build a chunk of garbage that stays under the ~64k-object GC floor, so
        ;; the automatic safepoint does NOT fire on its own here.
        (defn build (n acc)
          (if (= n 0) nil (build (- n 1) (cons n acc))))
        (build 2000 nil)
        ;; Nothing above is retained (the list is discarded), so a forced collect
        ;; should reclaim it. Returns the post-collection gc-stats map.
        (gc-collect)
    "#;
    let v = interp.eval_str(prog).expect("gc-collect program errored");
    let stats = interp.print(v);
    let runs = read_field(&stats, ":collections");
    let reclaimed = read_field(&stats, ":reclaimed");
    assert!(
        runs >= 1,
        "gc-collect should perform at least one collection, got {runs} — gc-stats: {stats}"
    );
    assert!(
        reclaimed > 0,
        "gc-collect should reclaim the discarded garbage, got {reclaimed} — gc-stats: {stats}"
    );
}

/// `(gc-trace on/off)` toggles per-collection trace logging and reports state.
/// We can't capture the child-thread stderr the trace prints to from here, so we
/// assert the *observable contract*: the query/set protocol returns the right
/// booleans (no arg = current state; truthy arg = set + return new state), and
/// the call is side-effect-safe around a real forced collection.
#[test]
fn gc_trace_toggles_and_reports_state() {
    let mut interp = Interp::new();
    let prog = r#"
        [(gc-trace)         ;; default: off
         (gc-trace true)    ;; turn on -> true
         (gc-trace)         ;; still on
         (gc-collect)       ;; a traced collection (output goes to stderr)
         (gc-trace false)   ;; turn off -> false
         (gc-trace)]        ;; still off
    "#;
    let v = interp.eval_str(prog).expect("gc-trace program errored");
    let out = interp.print(v);
    // The 4th element is the gc-stats map from gc-collect; assert on the booleans
    // around it.
    assert!(
        out.starts_with("[false true true {"),
        "gc-trace query/set protocol returned the wrong booleans: {out}"
    );
    assert!(
        out.ends_with("false false]"),
        "gc-trace should report off after being turned off: {out}"
    );
}

/// Collect at ANY eval depth (ADR-061). The same churn loop as
/// `gc_stats_counts_automatic_collections`, but run **inside a `try`** so the loop
/// body executes at eval depth ≥ 2 (the supervised-server / `(try (loop) …)`
/// shape). Before ADR-061 the safepoint only fired at the outermost eval
/// (`gc_block_depth() == 1`), so a loop this deep reported **0 collections** and
/// climbed unbounded; now the evaluator roots its transients on the operand stack
/// and collects at any depth. Asserts the collector actually fired down there.
/// A `def`'d range must survive promotion plus a local collection: `promote`
/// copies the backing `[lo hi step]` vector into RUNTIME. The bug this pins:
/// `promote_in` returning the LOCAL handle unchanged stored a stale VecId in
/// the shared global table, an OOB deref after the next arena flip (or in any
/// other process reading the global).
#[test]
fn promoted_range_survives_collection() {
    let mut interp = Interp::new();
    let prog = r#"
        (def r (range 1 10 2))
        (defn churn (n)
          (if (= n 0) nil (let (j (map inc (range 200))) (churn (- n 1)))))
        (churn 2000)
        (= r '(1 3 5 7 9))
    "#;
    let v = interp.eval_str(prog).expect("range-def program errored");
    assert_eq!(
        interp.print(v),
        "true",
        "def'd range corrupted by collection"
    );
}

#[test]
fn collects_below_the_outermost_eval() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; `map` realises 200 cons cells per iteration (a bare `(range 200)` is
        ;; a lazy O(1) Range value and would churn nothing).
        (defn churn (n acc)
          (if (= n 0)
              (gc-stats)
              (let (junk (map inc (range 200)))
                (churn (- n 1) (+ acc (count junk))))))
        ;; `try` runs `churn` via a thunk apply, so its loop body sits at eval
        ;; depth >= 2 — the case that used to never reach a GC safepoint.
        (try (churn 2000 0) (catch e e))
    "#;
    let v = interp.eval_str(prog).expect("churn-in-try program errored");
    let stats = interp.print(v);
    let runs = read_field(&stats, ":collections");
    assert!(
        runs >= 1,
        "expected ≥1 collection from a loop at depth ≥2 (ADR-061); got {runs} — \
         gc-stats: {stats}. A 0 here means the safepoint only fires at the \
         outermost eval again.",
    );
}

/// Promoting a *cyclic* local graph — a closure whose captured scope binds the
/// closure itself — must terminate, not stack-overflow. `def` (and `spawn`)
/// promote a value into the shared append-only RUNTIME region; before `promote`
/// grew a forwarding table the closure↔env back-edge recursed forever → SIGSEGV
/// (`docs/handoff-gc.md` item #2). Covers the self-referential case and the
/// realistic `letrec` mutual-recursion case, and reads the promoted cycle back
/// from a *separate* process (whose own LOCAL heap never held it) — proving the
/// shared cyclic graph is sound cross-heap, per the multi-core test rule.
#[test]
fn promotes_cyclic_local_closures_without_crashing() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; Self-referential local closure (the handoff repro): `g` captures the
        ;; `let` scope that binds `g`, so `def` promotes a closure<->env cycle.
        (def selfref (let (g (fn () g)) g))
        ;; Mutually recursive local closures via letrec, def'd: both capture the
        ;; one shared scope that binds both — a cycle through two closures.
        (def even-pred
          (letrec (even? (fn (n) (if (= n 0) true  (odd?  (- n 1))))
                   odd?  (fn (n) (if (= n 0) false (even? (- n 1)))))
            even?))
        ;; Resolve the promoted cycles from ANOTHER process: the worker reads
        ;; `selfref`/`even-pred` out of the shared RUNTIME region (its LOCAL heap
        ;; never built the cycle), so a correct answer proves the promoted graph.
        (def root (self))
        (spawn
          (let (ok (and (fn? (selfref))      ;; f returns the closure g
                        (fn? ((selfref)))    ;; g returns itself, still callable
                        (even-pred 10)       ;; 10 is even
                        (not (even-pred 7)))) ;; 7 is odd
            (send root (if ok :pass :fail))))
        (receive (:pass :pass) (:fail :fail) (after 10000 :timed-out))
    "#;
    let v = interp
        .eval_str(prog)
        .expect("cyclic-promote program errored");
    assert_eq!(
        interp.print(v),
        ":pass",
        "promoted cyclic closures didn't round-trip through a spawned process — \
         either promote regressed or the shared RUNTIME cycle reads wrong",
    );
}

/// The `send`/`spawn` twin of the promote bug: shipping a closure that *captures
/// another closure* must serialise without overflowing. `closure_to_message`
/// (`process/message.rs`) is the message-path analogue of `promote` — it copies a
/// closure's captured locals into the wire form. A router (a closure capturing a
/// map whose values are handler closures) is the realistic trigger from
/// `std/http.blsp`; before this was sound, `(spawn …)` of a thunk capturing such a
/// handler overflowed the same way `def` did. Here a worker process captures the
/// router via its spawn thunk, applies it, and sends the result back — proving the
/// closure-capturing-closure graph round-trips a per-heap message copy. Companion
/// to `promotes_cyclic_local_closures_without_crashing`, which covers the `def`
/// (promote) path; this covers the `send` (message) path.
#[test]
fn sends_closure_capturing_closure_without_crashing() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; A router: a closure capturing a map of handler closures (each value is
        ;; itself a closure). Kept LOCAL — never def'd — so shipping it exercises
        ;; closure_to_message, not promote.
        (defn router (routes)
          (fn (req) ((get routes (get req :path)) req)))
        (def root (self))
        (let (handler (router {"/" (fn (r) :ok)}))
          ;; The spawn thunk captures `handler` (a closure capturing a map of
          ;; closures) and `root`; it ships across to the worker's own heap.
          (spawn (send root (handler {:path "/"}))))
        (receive (m m) (after 10000 :timed-out))
    "#;
    let v = interp
        .eval_str(prog)
        .expect("send-closure-capturing-closure errored");
    assert_eq!(
        interp.print(v),
        ":ok",
        "a router (closure capturing a map of handler closures) didn't round-trip \
         through spawn/send — closure_to_message regressed on nested capture",
    );
}

/// A long list `def`'d to a global is promoted into the shared RUNTIME region as a
/// 100k-deep spine of RUNTIME pairs; the RUNTIME compactor must evacuate it
/// *iteratively*. `flush_rt_pair` used to recurse down the cdr spine — fine for the
/// shallow code bodies it was written for, but a pathological large quoted/list
/// literal would blow the native stack at the next `runtime_collect`. That path is
/// now reachable because RT compaction runs at eval auto-safepoints (ADR-091), so
/// the spine was made iterative (mirroring the LOCAL `flush_pair`). This forces a
/// RUNTIME collection via `(runtime-collect)` over a promoted 100k-element list and
/// asserts it survives intact — a stack overflow here is the regression. Run it
/// under `BROOD_GC_VERIFY=1` for the extra walk over the evacuated graph.
#[test]
fn runtime_collect_iterates_long_promoted_list_spine() {
    let mut interp = Interp::new();
    // 100k > any plausible native recursion budget for the old cdr-spine recursion,
    // but cheap to build (tail-recursive `range`) and to flush once.
    let prog = r#"
        ;; `def` promotes this list into the shared RUNTIME region: a 100k-deep
        ;; spine of RUNTIME pairs. (range is tail-recursive, so *building* it is
        ;; O(1) stack — the depth we're testing lives only in the flush.)
        (def big (range 100000))
        ;; Force a RUNTIME compaction now. Single-process here, so the runtime is
        ;; uniquely owned and the collect actually runs (:ran true), driving
        ;; flush_rt_pair down the whole spine. The pre-fix recursive flush
        ;; overflowed the native stack at this depth.
        (def stats (runtime-collect))
        ;; Read the evacuated list back: its length and endpoints must be intact,
        ;; proving the iterative spine rebuilt every cell and wired the cdrs right.
        (list (count big) (first big) (last big) (get stats :ran))
    "#;
    let v = interp
        .eval_str(prog)
        .expect("promoted-long-list runtime collect errored");
    assert_eq!(
        interp.print(v),
        "(100000 0 99999 true)",
        "a 100k-element list promoted to RUNTIME didn't survive a runtime_collect \
         intact — flush_rt_pair's cdr-spine evacuation regressed (recursion \
         overflow, or a mis-wired iterative rebuild)",
    );
}

/// Pull `:field N` out of a printed Brood map (`{... :field 123 ...}`). The map
/// printer separates a key from its value by one space; values here are
/// non-negative integers.
fn read_field(printed: &str, field: &str) -> i64 {
    let after = printed
        .split(field)
        .nth(1)
        .unwrap_or_else(|| panic!("field {field} not in {printed}"));
    after
        .trim_start()
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no integer after {field} in {printed}"))
}
