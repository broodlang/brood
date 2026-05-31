//! RUNTIME-collector exploration (read-only, no collector yet — see
//! `docs/runtime-collector-exploration.md`). Validates the **liveness model** a
//! future compacting RUNTIME collector would rest on: after redefining a global
//! many times, only the current version is reachable from the bindings, so the
//! superseded versions are *reclaimable*. `Heap::runtime_live_closure_count` marks
//! the live set by walking the shared code graph; this test confirms the gap to
//! `runtime_closure_count` (the total) tracks the redefinition churn — i.e. the
//! leak is real and would be reclaimable.

use std::sync::LazyLock;

use brood::Interp;

static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

#[test]
fn superseded_global_versions_are_reclaimable() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    // Redefine `f` 3000 times, each body structurally distinct (so the
    // unchanged-redef dedup, ADR-042, can't skip the append) — exactly the
    // hot-reload churn that leaks today.
    const N: usize = 3000;
    interp
        .eval_str(&format!(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (redef 0 {N})"
        ))
        .expect("redef loop errored");

    let total = interp.heap.runtime_closure_count();
    let live = interp.heap.runtime_live_closure_count();
    let reclaimable = total.saturating_sub(live);
    eprintln!("RUNTIME-GC estimate after {N} redefs: total={total} live={live} reclaimable={reclaimable}");

    // Only the current `f` (+ `redef` itself + a handful) is reachable from the
    // bindings; the other ~N-1 `f` versions are superseded and unreferenced.
    assert!(
        total >= N,
        "expected the {N} redefs to have promoted ≥{N} RUNTIME closures, got total={total}",
    );
    assert!(
        live < 50,
        "live RUNTIME closures should be a small constant (current f + redef + few), got {live}",
    );
    assert!(
        reclaimable >= N - 50,
        "expected ~{} reclaimable superseded versions, got {reclaimable} (total={total}, live={live})",
        N - 1,
    );
}

/// Step 2a — the out-of-place evacuation core. After churn, evacuate the live
/// RUNTIME code into a fresh `CodeSlabs` and confirm: (1) it contains *only* the
/// live closures (== the estimator's live count, ≪ total), and (2) the evacuated
/// region passes the verifier — every handle points within the new, compacted
/// region (no rewrite missed). This validates the trace→copy→forward logic safely
/// (out-of-place: the live region is untouched), the foundation before the in-place
/// swap (2b) and stop-the-world (2c).
#[test]
fn evacuation_copies_only_live_code_and_verifies() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    const N: usize = 3000;
    interp
        .eval_str(&format!(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (redef 0 {N})"
        ))
        .expect("redef loop errored");

    let (total, live, verified) = interp.heap.runtime_evacuate_check();
    eprintln!("RUNTIME-GC 2a evacuate: total={total} live={live} verified={verified}");

    assert!(verified, "evacuated region has a dangling handle (a missed rewrite)");
    assert_eq!(
        live,
        interp.heap.runtime_live_closure_count(),
        "evacuation must copy exactly the reachable closures",
    );
    assert!(total >= N, "expected ≥{N} promoted closures, got total={total}");
    assert!(live < 50, "live should be a small constant, got {live} (total {total})");

    // The program is unchanged by the (out-of-place) evacuation — `f` still works.
    // Last redef was i=N-1=2999, so f = (fn (x) (+ (* x 2999) 2999)); (f 7)=8*2999.
    let v = interp.eval_str("(f 7)").expect("f errored after evacuation");
    assert_eq!(interp.print(v), "23992");
}

/// Step 2b — the in-place compacting collect actually reclaims and preserves
/// correctness. After churn, `runtime_collect` compacts the region (gated on unique
/// `Arc` ownership — true for this single-process `Interp`), and the program keeps
/// working: the rewritten globals resolve, and freshly-defined code runs on the
/// compacted region. The LOCAL-held-handle rewrite path is covered by the
/// in-language `tests/runtime_collect_test.blsp` (a closure kept in a `let`/list
/// across a collect), which also runs under `BROOD_GC_STRESS`.
#[test]
fn in_place_collect_reclaims_and_preserves_correctness() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    const N: usize = 2000;
    interp
        .eval_str(&format!(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (redef 0 {N})"
        ))
        .expect("redef loop errored");

    let (before, after) =
        interp.heap.runtime_collect().expect("collect should run for a single-process Interp");
    eprintln!("RUNTIME-GC 2b collect: before={before} after={after} reclaimed={}", before - after);
    assert!(before >= N, "expected ≥{N} promoted, got {before}");
    assert!(before - after >= N - 50, "expected ~{} reclaimed, got {}", N - 1, before - after);

    // The rewritten current `f` still computes (last redef i=N-1=1999): 1999*7+1999.
    let v = interp.eval_str("(f 7)").expect("f errored after collect");
    assert_eq!(interp.print(v), (1999 * 7 + 1999).to_string());
    // Freshly-defined code runs on the compacted region (cleared caches rebuild).
    let v = interp.eval_str("(defn k (a) (* a a)) (k 9)").expect("new code errored after collect");
    assert_eq!(interp.print(v), "81");

    // The LOCAL-heap rewrite path: collect *while* a RUNTIME closure is held in a
    // LOCAL binding (`g`) on the live operand stack/env. The whole expression is one
    // top-level form, so the collect runs with `g` live — it must rewrite `g`'s
    // handle so `(g 3)` still calls the right (compacted) code.
    let v = interp.eval_str("(let (g f) (runtime-collect) (g 3))").expect("let-held collect errored");
    assert_eq!(interp.print(v), (1999 * 3 + 1999).to_string());

    // A second bare collect now reclaims little (steady state — nothing superseded).
    let (b2, a2) = interp.heap.runtime_collect().expect("second collect");
    assert!(b2 - a2 < 50, "steady-state collect should reclaim little, got {}", b2 - a2);
}
