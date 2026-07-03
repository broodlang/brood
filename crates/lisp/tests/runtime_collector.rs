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
    // Measure the manual path in isolation — keep churn from being auto-collected.
    interp.heap.set_rt_auto_collect(false);
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
    eprintln!(
        "RUNTIME-GC estimate after {N} redefs: total={total} live={live} reclaimable={reclaimable}"
    );

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
    // Measure the manual path in isolation — keep churn from being auto-collected.
    interp.heap.set_rt_auto_collect(false);
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

    assert!(
        verified,
        "evacuated region has a dangling handle (a missed rewrite)"
    );
    assert_eq!(
        live,
        interp.heap.runtime_live_closure_count(),
        "evacuation must copy exactly the reachable closures",
    );
    assert!(
        total >= N,
        "expected ≥{N} promoted closures, got total={total}"
    );
    assert!(
        live < 50,
        "live should be a small constant, got {live} (total {total})"
    );

    // The program is unchanged by the (out-of-place) evacuation — `f` still works.
    // Last redef was i=N-1=2999, so f = (fn (x) (+ (* x 2999) 2999)); (f 7)=8*2999.
    let v = interp
        .eval_str("(f 7)")
        .expect("f errored after evacuation");
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
    // Measure the manual path in isolation — keep churn from being auto-collected.
    interp.heap.set_rt_auto_collect(false);
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

    let (before, after) = interp
        .heap
        .runtime_collect()
        .expect("collect should run for a single-process Interp");
    eprintln!(
        "RUNTIME-GC 2b collect: before={before} after={after} reclaimed={}",
        before - after
    );
    assert!(before >= N, "expected ≥{N} promoted, got {before}");
    assert!(
        before - after >= N - 50,
        "expected ~{} reclaimed, got {}",
        N - 1,
        before - after
    );

    // The rewritten current `f` still computes (last redef i=N-1=1999): 1999*7+1999.
    let v = interp.eval_str("(f 7)").expect("f errored after collect");
    assert_eq!(interp.print(v), (1999 * 7 + 1999).to_string());
    // Freshly-defined code runs on the compacted region (cleared caches rebuild).
    let v = interp
        .eval_str("(defn k (a) (* a a)) (k 9)")
        .expect("new code errored after collect");
    assert_eq!(interp.print(v), "81");

    // The LOCAL-heap rewrite path: collect *while* a RUNTIME closure is held in a
    // LOCAL binding (`g`) on the live operand stack/env. The whole expression is one
    // top-level form, so the collect runs with `g` live — it must rewrite `g`'s
    // handle so `(g 3)` still calls the right (compacted) code.
    let v = interp
        .eval_str("(let (g f) (runtime-collect) (g 3))")
        .expect("let-held collect errored");
    assert_eq!(interp.print(v), (1999 * 3 + 1999).to_string());

    // A second bare collect now reclaims little (steady state — nothing superseded).
    let (b2, a2) = interp.heap.runtime_collect().expect("second collect");
    assert!(
        b2 - a2 < 50,
        "steady-state collect should reclaim little, got {}",
        b2 - a2
    );
}

/// The **automatic** safepoint trigger bounds the RUNTIME region with *no*
/// explicit `(runtime-collect)`. A single-process `Interp` uniquely owns the
/// runtime `Arc`, so the eval safepoint auto-compacts once churn crosses
/// `rt_gc_floor()` (default 4096). Redefining far past that must leave the live
/// region a small fraction of the promotions — i.e. it didn't grow unbounded.
#[test]
fn auto_safepoint_collect_bounds_runtime_region() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    // 6000 distinct redefs — well past the 4096 default floor, so the safepoint
    // must have auto-collected at least once mid-loop.
    const N: usize = 6000;
    interp
        .eval_str(&format!(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (redef 0 {N})"
        ))
        .expect("redef loop errored");

    // Without auto-collection the region would hold ≥N promoted closures. Bounded,
    // it sits near the floor (live set is a small constant; threshold ≈ floor).
    let count = interp.heap.runtime_closure_count();
    eprintln!("auto-collect: RUNTIME closures after {N} redefs = {count}");
    assert!(
        count < N,
        "auto safepoint collection should bound the region well below {N} promotions, got {count}",
    );

    // And the program is still correct after all those mid-loop compactions: the
    // current `f` (last redef i=N-1=5999) resolves on the compacted region.
    let v = interp
        .eval_str("(f 7)")
        .expect("f errored after auto-collect");
    assert_eq!(interp.print(v), (5999i64 * 7 + 5999).to_string());
}

// The end-to-end corruption (a live compiled arm's `Const` stranded by a mid-call
// compaction) is a slab-packing-sensitive race that only reproduces reliably with a
// large, structurally varied churn file (e.g. `load`ing a real module under
// `BROOD_GC_STRESS=1`) — see `docs/known-issues.md` for the manual repro. The
// *mechanism* the fix rests on is unit-tested deterministically in
// `crate::eval::compile::tests` (`const_handle_round_trips`,
// `rewrite_arm_handles_*`): that `runtime_collect`'s `rewrite_arm_handles` rewrites
// every movable handle a live arm's node tree embeds.

/// Regression: `%isolate` must be safe against a RUNTIME compaction firing *inside*
/// the isolated thunk. `%isolate` snapshots the global table — an off-graph
/// `SymbolMap<Value>` of raw RUNTIME handles — runs a thunk, then restores it. Before
/// the fix, a compaction while the thunk ran (its `def`s crossing the RT floor)
/// relocated those handles, so `restore_globals` reinstalled stale handles that now
/// aliased *other* closures — an unrelated pre-isolate global silently misdispatched
/// (resolved to a 1-arg closure defined inside the rolled-back isolate → arity error).
/// The fix suppresses RUNTIME auto-compaction across snapshot→restore
/// (`Heap::rt_collect_block`). See `docs/devlog.md` (2026-07-03).
#[test]
fn isolate_is_safe_against_a_runtime_compaction_inside_the_thunk() {
    LazyLock::force(&MEM_GUARD);
    // A low RT floor so the ~500 defs inside the isolate reliably trip a compaction
    // mid-thunk (the bug's precondition) regardless of build profile. Per-test process
    // under nextest, so this env set doesn't leak to other tests.
    std::env::set_var("BROOD_RT_GC_FLOOR", "128");
    let mut interp = Interp::new();
    // A 0-arg global defined BEFORE the isolate; its resolution must survive intact.
    interp
        .eval_str(
            "(defn probe () 42) \
             (defn defmany (i n) \
               (if (= i n) :done \
                 (do (eval (list 'def (symbol (str \"z-\" i)) (list 'fn '(x) 'x))) \
                     (defmany (+ i 1) n))))",
        )
        .expect("setup defs errored");
    // Isolate a thunk that defs 500 distinct 1-arg globals — enough to cross the floor
    // and (pre-fix) trigger a compaction that relocated the snapshot's handles.
    interp
        .eval_str("(%isolate (fn () (defmany 0 500)))")
        .expect("isolate thunk errored");
    // After the isolate, `probe` must still resolve to its own 0-arg body → 42.
    // Pre-fix this raised an arity error (probe aliased a rolled-back z-* 1-arg fn).
    let v = interp
        .eval_str("(probe)")
        .expect("probe call errored after isolate (global misdispatch)");
    assert_eq!(
        interp.print(v),
        "42",
        "a pre-isolate global misdispatched after %isolate — a RUNTIME compaction relocated the snapshot's handles",
    );
    std::env::remove_var("BROOD_RT_GC_FLOOR");
}

/// Regression for the test-runner leak fix (docs/devlog.md 2026-07-03): running each
/// "file" inside its own `%isolate` (as `test/run-tests-scoped` does) rolls back that
/// file's top-level `def`s and lets the next safepoint reclaim the promoted code — so a
/// large suite's shared RUNTIME region stays bounded instead of accumulating every file's
/// distinct code at once. Without scoping, 300 "files" × 200 distinct defs promote ~60k
/// closures that stay rooted (the leak); with per-`%isolate` scoping the region collapses
/// back to a small constant. (Relies on the `%isolate` compaction-safety fix — otherwise a
/// mid-isolate compaction would corrupt the snapshot.)
#[test]
fn per_isolate_scoping_bounds_runtime_region_growth() {
    LazyLock::force(&MEM_GUARD);
    std::env::set_var("BROOD_RT_GC_FLOOR", "256");
    let mut interp = Interp::new();
    interp
        .eval_str(
            "(defn defmany (fi i n) \
               (if (= i n) :done \
                 (do (eval (list 'def (symbol (str \"z-\" fi \"-\" i)) (list 'fn '(x) (list '+ 'x i)))) \
                     (defmany fi (+ i 1) n)))) \
             (defn loopf (f i n) (if (= i n) :done (do (f i) (loopf f (+ i 1) n))))",
        )
        .expect("setup errored");
    // 300 "files", each defining 200 DISTINCT globals inside its own %isolate.
    interp
        .eval_str("(loopf (fn (fi) (%isolate (fn () (defmany fi 0 200)))) 0 300)")
        .expect("scoped loop errored");
    interp.eval_str("(runtime-collect)").expect("collect errored");
    let count = interp.heap.runtime_closure_count();
    assert!(
        count < 500,
        "per-%isolate scoping should bound the RUNTIME region — got {count} promoted closures \
         after 300×200 distinct defs (unscoped accumulates ~60000, the leak)",
    );
    std::env::remove_var("BROOD_RT_GC_FLOOR");
}
