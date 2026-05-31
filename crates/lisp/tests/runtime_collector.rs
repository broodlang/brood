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
