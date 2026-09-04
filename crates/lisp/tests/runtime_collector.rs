//! RUNTIME-collector exploration (read-only, no collector yet — see
//! `docs/runtime-collector-exploration.md`). Validates the **liveness model** a
//! future compacting RUNTIME collector would rest on: after redefining a global
//! many times, only the current version is reachable from the bindings, so the
//! superseded versions are *reclaimable*. `Heap::runtime_live_closure_count` marks
//! the live set by walking the shared code graph; this test confirms the gap to
//! `runtime_closure_count` (the total) tracks the redefinition churn — i.e. the
//! leak is real and would be reclaimable.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use brood::core::heap::Heap;
use brood::process;
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
    // Define the driver and force the modules its body names to load BEFORE the baseline is
    // taken. `reflect/eval` is a QUALIFIED reference, so it auto-loads `std/reflect.blsp`,
    // and every `def` in that module promotes into the RUNTIME region. Those closures are
    // legitimately live and have nothing to do with the churn under test — counting them
    // turned this assertion into a measure of how much of `std/` the driver happens to pull
    // in, which is why moving `eval` under `reflect/` broke it at 111 with no leak involved.
    interp
        .eval_str(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (reflect/eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (reflect/eval (quote 1))",
        )
        .expect("driver setup errored");
    // Everything reachable before any churn: the prelude's promotions, the driver, and
    // whatever `std/` the driver loaded. The churn is measured against this.
    let baseline = interp.heap.runtime_live_closure_count();
    interp
        .eval_str(&format!("(redef 0 {N})"))
        .expect("redef loop errored");

    let total = interp.heap.runtime_closure_count();
    let live = interp.heap.runtime_live_closure_count();
    let reclaimable = total.saturating_sub(live);
    eprintln!(
        "RUNTIME-GC estimate after {N} redefs: total={total} live={live} \
         (baseline={baseline}, churn-live={}) reclaimable={reclaimable}",
        live.saturating_sub(baseline)
    );

    // Only the current `f` (+ `redef` itself + a handful) is reachable from the
    // bindings; the other ~N-1 `f` versions are superseded and unreferenced.
    assert!(
        total >= N,
        "expected the {N} redefs to have promoted ≥{N} RUNTIME closures, got total={total}",
    );
    assert!(
        live.saturating_sub(baseline) < 50,
        "the churn should leave a small constant live (current f + a few), got {live} \
         against a pre-churn baseline of {baseline}",
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
    // Define the driver and force the modules its body names to load BEFORE the baseline is
    // taken. `reflect/eval` is a QUALIFIED reference, so it auto-loads `std/reflect.blsp`,
    // and every `def` in that module promotes into the RUNTIME region. Those closures are
    // legitimately live and have nothing to do with the churn under test — counting them
    // turned this assertion into a measure of how much of `std/` the driver happens to pull
    // in, which is why moving `eval` under `reflect/` broke it at 111 with no leak involved.
    interp
        .eval_str(
            "(defn redef (i n) \
               (if (= i n) :done \
                 (do (reflect/eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
                     (redef (+ i 1) n)))) \
             (reflect/eval (quote 1))",
        )
        .expect("driver setup errored");
    // Everything reachable before any churn: the prelude's promotions, the driver, and
    // whatever `std/` the driver loaded. The churn is measured against this.
    let baseline = interp.heap.runtime_live_closure_count();
    interp
        .eval_str(&format!("(redef 0 {N})"))
        .expect("redef loop errored");

    let (total, live, verified) = interp.heap.runtime_evacuate_check();
    eprintln!(
        "RUNTIME-GC 2a evacuate: total={total} live={live} (baseline={baseline}, \
         churn-live={}) verified={verified}",
        live.saturating_sub(baseline)
    );

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
        live.saturating_sub(baseline) < 50,
        "the churn should leave a small constant live, got {live} against a pre-churn \
         baseline of {baseline} (total {total})"
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
                 (do (reflect/eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
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
        .eval_str("(let (g f) (dev/runtime-collect) (g 3))")
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
/// explicit `(dev/runtime-collect)`. A single-process `Interp` uniquely owns the
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
                 (do (reflect/eval (list 'def 'f (list 'fn '(x) (list '+ (list '* 'x i) i)))) \
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
    // mid-thunk (the bug's precondition) regardless of build profile. Set on THIS heap,
    // not through `BROOD_RT_GC_FLOOR`: the env var is read once per process, so under
    // plain `cargo test` (one process, parallel threads) it leaked into whichever tests
    // ran after it — the promotion-count tests then read a 128 floor and failed (KI-86).
    let mut interp = Interp::new();
    interp.heap.set_rt_gc_floor(128);
    // A 0-arg global defined BEFORE the isolate; its resolution must survive intact.
    interp
        .eval_str(
            "(defn probe () 42) \
             (defn defmany (i n) \
               (if (= i n) :done \
                 (do (reflect/eval (list 'def (symbol (str \"z-\" i)) (list 'fn '(x) 'x))) \
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
    let mut interp = Interp::new();
    interp.heap.set_rt_gc_floor(256);
    interp
        .eval_str(
            "(defn defmany (fi i n) \
               (if (= i n) :done \
                 (do (reflect/eval (list 'def (symbol (str \"z-\" fi \"-\" i)) (list 'fn '(x) (list '+ 'x i)))) \
                     (defmany fi (+ i 1) n)))) \
             (defn loopf (f i n) (if (= i n) :done (do (f i) (loopf f (+ i 1) n))))",
        )
        .expect("setup errored");
    // 300 "files", each defining 200 DISTINCT globals inside its own %isolate.
    interp
        .eval_str("(loopf (fn (fi) (%isolate (fn () (defmany fi 0 200)))) 0 300)")
        .expect("scoped loop errored");
    interp
        .eval_str("(dev/runtime-collect)")
        .expect("collect errored");
    let count = interp.heap.runtime_closure_count();
    assert!(
        count < 500,
        "per-%isolate scoping should bound the RUNTIME region — got {count} promoted closures \
         after 300×200 distinct defs (unscoped accumulates ~60000, the leak)",
    );
}

/// Regression (bug #2 sibling): `declared_sigs` holds promoted RUNTIME type-expression
/// handles off the graph. Before the fix it was NOT in the set `runtime_collect_with`
/// rewrites, so a compaction relocated the type-expr out from under the stored handle and
/// the checker read a garbage form (`(int -> int)` → `(i 1)`). `runtime_collect_with` now
/// evacuates `declared_sigs` alongside `globals`.
#[test]
fn declared_sigs_survive_a_runtime_compaction() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    let sym = brood::core::value::intern("my-fn");
    let ty = interp.eval_str("'(int -> int)").expect("type expr");
    interp.heap.set_declared_sig(sym, ty);
    // Churn distinct defs, then compact the RUNTIME region (relocates handles).
    interp
        .eval_str(
            "(defn redef (i n) (if (= i n) :done (do (reflect/eval (list 'def (symbol (str \"z-\" i)) (list 'fn '(x) i))) (redef (+ i 1) n)))) (redef 0 3000)",
        )
        .expect("churn");
    interp.heap.set_rt_auto_collect(true);
    interp.heap.runtime_collect();
    let got = interp.heap.declared_sig_value(sym).expect("sig vanished");
    assert_eq!(
        interp.print(got),
        "(int -> int)",
        "declared sig corrupted by RUNTIME compaction"
    );
}

/// Regression for the KI-6 hardening: a globals snapshot now suppresses RUNTIME
/// compaction at the `runtime_collect_with` choke point — covering BOTH the auto
/// safepoint path AND an explicit `(dev/runtime-collect)`. So a manual collect *inside* an
/// `%isolate` (which the original KI-6 fix left out of scope) is a no-op rather than a
/// snapshot-stranding corruption. A pre-isolate global must survive it.
#[test]
fn manual_runtime_collect_inside_isolate_is_a_noop() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.eval_str("(defn probe () 42)").expect("probe def");
    interp
        .eval_str(
            "(%isolate (fn () \
               (defn dm (i n) (if (= i n) :done (do (reflect/eval (list 'def (symbol (str \"z-\" i)) (list 'fn '(x) 'x))) (dm (+ i 1) n)))) \
               (dm 0 300) \
               (dev/runtime-collect)))",
        )
        .expect("isolate thunk");
    let v = interp
        .eval_str("(probe)")
        .expect("probe call after isolate");
    assert_eq!(
        interp.print(v),
        "42",
        "an explicit (dev/runtime-collect) inside %isolate stranded the globals snapshot",
    );
}

/// Hardening for unpaired snapshot/restore (KI-6). The `GlobalsSnapshot` newtype makes
/// restore-without-snapshot unforgeable and double-restore a move error (both compile-time);
/// `#[must_use]` flags a dropped snapshot. This exercises the one runtime-checkable part:
/// nested snapshots each suppress compaction, restore is LIFO (the debug-only depth assert
/// must not fire on valid nesting), and releasing all of them re-enables compaction.
#[test]
fn nested_globals_snapshots_suppress_then_re_enable_compaction() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    let outer = interp.heap.snapshot_globals();
    let inner = interp.heap.snapshot_globals();
    assert!(
        interp.heap.runtime_collect().is_none(),
        "compaction must be a no-op while globals snapshots are outstanding",
    );
    interp.heap.restore_globals(inner); // LIFO: inner first — the depth assert must hold
    assert!(
        interp.heap.runtime_collect().is_none(),
        "still suppressed while the outer snapshot is outstanding",
    );
    interp.heap.restore_globals(outer);
    assert!(
        interp.heap.runtime_collect().is_some(),
        "compaction must be re-enabled once every snapshot is restored",
    );
}

/// Hot-reload soundness for the checker's namespace caches (`module_public_exports` /
/// `known_ns_prefixes`, count-keyed): adding an export to a module (the hot-reload case)
/// must be reflected — the cache invalidates because the global COUNT moved. (A rebind
/// keeps the count AND the name-set, so the cache stays correct then; `%isolate` rollback
/// restores the exact prior set, so a recurring count means the same set — no stale read.)
#[test]
fn checker_ns_caches_reflect_hot_reload_adds() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp
        .eval_str("(defmodule mlibZ) (defn aaa () 1)")
        .expect("load module");
    let bare_names = |v: Vec<(brood::core::value::Symbol, brood::core::value::Symbol)>| {
        v.into_iter()
            .map(|(b, _)| brood::core::value::symbol_name(b))
            .collect::<Vec<_>>()
    };
    let before = bare_names(interp.heap.module_public_exports("mlibZ/"));
    assert!(before.iter().any(|n| n == "aaa"), "aaa should be exported");
    assert!(!before.iter().any(|n| n == "bbb"), "bbb not defined yet");
    assert!(interp.heap.known_ns_prefixes().contains("mlibZ/"));
    // Hot-reload: the module gains a new export → global count moves → caches must rebuild.
    interp.eval_str("(def mlibZ/bbb 2)").expect("add export");
    let after = bare_names(interp.heap.module_public_exports("mlibZ/"));
    assert!(
        after.iter().any(|n| n == "bbb"),
        "hot-reload add not reflected — stale checker cache (got {after:?})",
    );
    assert!(after.iter().any(|n| n == "aaa"), "aaa still exported");
}

/// Step 2 — **aging** (the Erlang-style 2-generation core). `age_runtime` flips
/// the RUNTIME code region to a fresh generation slot; code promoted *before* the
/// flip keeps executing (read from its own `code_gen` slot), and code promoted
/// *after* lands in the new slot — both live simultaneously with no rewrite. This
/// is the read-path correctness that the whole 2-generation model rests on: a
/// handle carries its generation, so the two slabs coexist.
#[test]
fn aging_flips_generation_and_both_gens_execute() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Generation 0: define `g` and confirm it runs.
    assert_eq!(interp.heap.runtime_cur_gen(), 0, "starts in gen 0");
    interp.eval_str("(defn g (x) (* x 10))").expect("define g");
    let g0 = interp.eval_str("(g 3)").unwrap();
    assert_eq!(interp.print(g0), "30");

    // Age: target slot 1 is empty, so the flip succeeds.
    assert!(interp.heap.age_runtime(), "should age (slot 1 empty)");
    assert_eq!(interp.heap.runtime_cur_gen(), 1, "now in gen 1");

    // Generation 1: define `h`. It lands in slot 1 while `g` still lives in slot 0.
    interp.eval_str("(defn h (x) (+ x 100))").expect("define h");

    // BOTH generations execute correctly — the two-slab read path is sound.
    let g1 = interp.eval_str("(g 3)").unwrap();
    assert_eq!(
        interp.print(g1),
        "30",
        "gen-0 code still executes after aging"
    );
    let h1 = interp.eval_str("(h 3)").unwrap();
    assert_eq!(interp.print(h1), "103", "gen-1 code executes");
    // A closure that closes over both (calls g and h) — crosses generations in one call.
    interp
        .eval_str("(defn gh (x) (+ (g x) (h x)))")
        .expect("define gh");
    let gh1 = interp.eval_str("(gh 3)").unwrap();
    assert_eq!(
        interp.print(gh1),
        "133",
        "a gen-1 closure calling a gen-0 global works",
    );

    // Cannot age again: slot 0 is still occupied (g lives there) → 2-versions-max.
    assert!(
        !interp.heap.age_runtime(),
        "second age must fail — target slot 0 not empty (g still live)",
    );
    assert_eq!(interp.heap.runtime_cur_gen(), 1, "stays in gen 1");
}

/// Stage 3 — the **cooperative liveness probe** (`runtime_gen_referenced`). It is
/// the per-process half of the union that decides when an aged-out old generation
/// can be freed (Stage 4): the generation is dead only when every live process (and
/// the shared globals) reports it unreferenced. Here, single-process, the probe
/// sees everything, so its answer is exact — we watch a generation go from
/// referenced (its code still bound in the globals) to dead (every binding
/// superseded into the new generation).
#[test]
fn old_generation_liveness_probe() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Generation 0: define `f`. It is referenced (bound in the globals); the empty
    // gen-1 slot is trivially unreferenced (the fast path).
    interp.eval_str("(defn f (x) (* x 2))").expect("define f");
    assert!(
        interp.heap.runtime_gen_referenced(0),
        "gen 0 is referenced — `f` is bound there",
    );
    assert!(
        !interp.heap.runtime_gen_referenced(1),
        "gen 1 slot is empty → trivially unreferenced",
    );

    // Age to gen 1. Aging alone moves NO bindings, so `f` still lives in gen 0 and
    // gen 0 stays referenced; gen 1 is still empty until something is promoted there.
    assert!(interp.heap.age_runtime(), "should age (slot 1 empty)");
    assert_eq!(interp.heap.runtime_cur_gen(), 1);
    assert!(
        interp.heap.runtime_gen_referenced(0),
        "gen 0 still referenced after aging — `f`'s binding hasn't moved",
    );
    assert!(
        !interp.heap.runtime_gen_referenced(1),
        "gen 1 still empty right after aging",
    );

    // Promote fresh code into gen 1: `h` lands there and is referenced; gen 0 (still
    // holding `f`) stays referenced too — both generations live at once.
    interp.eval_str("(defn h (x) (+ x 100))").expect("define h");
    assert!(
        interp.heap.runtime_gen_referenced(1),
        "gen 1 now referenced — `h` is bound there",
    );
    assert!(
        interp.heap.runtime_gen_referenced(0),
        "gen 0 still referenced — `f` hasn't been superseded yet",
    );

    // Drain gen 0: redefine `f` (structurally different, so ADR-042 dedup can't skip
    // the promote) — its binding now points into gen 1. Nothing live references the
    // superseded gen-0 `f` any more; a LOCAL collect drops any transient remnant.
    interp
        .eval_str("(defn f (x) (+ (* x 3) 7))")
        .expect("redefine f");
    interp.heap.collect(&mut [], &mut []);
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "gen 0 is now dead — every binding it held has been superseded into gen 1",
    );
    // Gen 1 carries the current `f` + `h` and remains referenced.
    assert!(
        interp.heap.runtime_gen_referenced(1),
        "gen 1 holds the live code",
    );
}

/// Stage 3b — the **cross-process drain union**. A generation is dead only once
/// *every* live process has reported clean for the current drain epoch. Driven here
/// with two real heaps sharing one runtime `Arc` (as `spawn` builds a child), so the
/// union is exercised deterministically without leaning on scheduler timing: one
/// process superseded its reference, the other still pins the old generation via a
/// captured closure handle — the union must stay `false` until that pin is released.
#[test]
fn cross_process_drain_union() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Gen 0: define `f` and capture its (gen-0) closure handle before anything moves.
    interp.eval_str("(defn f (x) (* x 2))").expect("define f");
    let f_gen0 = interp.eval_str("f").expect("resolve f");

    // Age, then supersede `f` into gen 1, so the shared globals no longer point at
    // gen 0 — the precondition that makes a clean ack stay clean.
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp
        .eval_str("(defn f (x) (+ (* x 3) 7))")
        .expect("redefine f");
    interp.heap.collect(&mut [], &mut []);
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "main no longer references gen 0 (globals moved to gen 1)",
    );

    // A second process of the SAME runtime that captured the gen-0 `f` in a local
    // root — exactly the private-state pin the shared-globals view can't see.
    let mut child = Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    child.push_root(f_gen0);
    assert!(
        child.runtime_gen_referenced(0),
        "child pins gen 0 via the captured handle",
    );

    // Arm the drain of gen 0. Both heaps observe it through the shared runtime.
    const MAIN: u64 = 1;
    const CHILD: u64 = 2;
    let epoch = interp.heap.begin_gen_drain(0);
    assert_eq!(epoch, 1, "first drain is epoch 1");
    assert!(
        interp.heap.drain_active() && child.drain_active(),
        "the drain is visible to every process of the runtime",
    );

    // Each process reports. Main is clean; child still pins → the union is NOT drained.
    interp.heap.report_gen_liveness(MAIN);
    child.report_gen_liveness(CHILD);
    assert!(
        !interp.heap.gen_drained(&[MAIN, CHILD]),
        "gen 0 is not dead — the child still references it",
    );
    // The pinning child is exactly what a single-process view would miss.
    assert!(
        interp.heap.gen_drained(&[MAIN]),
        "main considered alone is clean",
    );

    // Child releases the captured handle and re-reports → generation now drained.
    child.truncate_roots(0);
    assert!(
        !child.runtime_gen_referenced(0),
        "child released its only gen-0 reference",
    );
    child.report_gen_liveness(CHILD);
    assert!(
        interp.heap.gen_drained(&[MAIN, CHILD]),
        "every live process reported clean → gen 0 is dead",
    );

    // A fresh drain bumps the epoch, so the stale clean acks no longer count.
    let epoch2 = interp.heap.begin_gen_drain(0);
    assert_eq!(epoch2, 2, "the drain epoch is strictly monotonic");
    assert!(
        !interp.heap.gen_drained(&[MAIN, CHILD]),
        "a new drain epoch invalidates prior acks — every process must re-report",
    );

    // Ending the drain makes the reporting path inert again.
    interp.heap.end_gen_drain();
    assert!(!interp.heap.drain_active());
    assert!(
        !interp.heap.gen_drained(&[MAIN]),
        "no drain armed → the union answer is false",
    );
}

/// Stage 3c — the drain report **wired through the live scheduler**. A real spawned
/// process running under the worker pool participates in the union via the eval / VM
/// safepoint (its report) and the scheduler registry (the live set). A worker that
/// captured the old generation's code and parked pins it — `old_gen_drained` stays
/// `false` until the worker releases the reference and exits, at which point the root
/// (having reported clean at its own safepoint) is the only pinner left and the
/// generation is dead. Genuinely concurrent (default worker pool); the outcome is
/// deterministic because it hinges on process *liveness*, not scheduling order.
#[test]
fn drain_report_wires_through_the_scheduler() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Define `f` (gen 0). Spawn a worker that captures the gen-0 `f` in a closure and
    // parks on `receive` holding it — a live process pinning gen 0 that the root's
    // single-process view can't see. It reports its pid in `:ready` (so the root can
    // release it later) and only replies `:released` after dropping the reference.
    interp
        .eval_str(
            r#"
            (defn f (x) (* x 2))
            (def root (self))
            (defn worker (held)
              (do
                (send root [:ready (self)])
                (receive (:release (send root :released)))))
            (spawn (fn () (worker f)))
            (receive ([:ready wp] (def worker-pid wp)))
            "#,
        )
        .expect("spawn a worker that parks holding the gen-0 closure");

    // Age, then supersede EVERY gen-0 global (`f` and `worker`) into gen 1, so the
    // shared globals no longer point at gen 0. The still-running worker process keeps
    // its own captured gen-0 copies — that is the pin the drain must respect — but the
    // root itself is now clean of gen 0.
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp
        .eval_str(
            "(defn f (x) (+ (* x 3) 7)) \
             (defn worker (held) (do (send root :v2) :ignored))",
        )
        .expect("redefine the gen-0 globals into gen 1");
    interp.heap.collect(&mut [], &mut []);
    // Redefining f/worker moves *those* globals to gen 1, but the real reclaim cycle
    // (`advance_runtime_multigen`) also runs `migrate_live_globals(old)` before arming the
    // drain — the load-bearing "globals point into the current generation before anyone
    // reports" step. Any *permanent* live runtime global still resident in gen 0 (e.g. the
    // boot-defined `*features-loading*` load tracker, whose empty-map value is a gen-0
    // runtime handle) is moved by migrate, not by the redefinition, so the test must run it
    // too — otherwise which slot boot happens to leave that global in leaks into the result.
    interp.heap.migrate_live_globals(0);
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "the root itself no longer references gen 0",
    );

    // Arm the drain of gen 0, then make the root report at its own safepoint (calling
    // gen-1 `f` drives the VM trampoline). Root is clean → it acks the epoch.
    interp.heap.begin_gen_drain(0);
    interp.eval_str("(f 3)").expect("root safepoint report");

    // The worker (parked, holding the gen-0 closure) is in the live set but has no
    // ack for this epoch, so the generation is NOT drained — the whole point of the
    // cross-process union.
    assert!(
        process::live_pids().len() >= 2,
        "root and the parked worker are both live",
    );
    assert!(
        !process::old_gen_drained(&interp.heap),
        "the parked worker still pins gen 0",
    );

    // Release the worker: it drops the captured gen-0 closure, replies, and exits.
    interp
        .eval_str("(do (send worker-pid :release) (receive (:released :ok)))")
        .expect("release the worker and await its acknowledgement");

    // Once the worker has left the registry, the root is the only live process and it
    // already acked clean → gen 0 is drained. Poll briefly for the worker's exit to
    // finish (its `:released` send races a hair ahead of deregistration).
    // 5 s is generous for an ordinary build and NOT for an instrumented one: under ASAN
    // every allocation is intercepted and the whole runtime moves at roughly a tenth speed,
    // so the worker's exit-and-deregister loses the race with this deadline and the test
    // reports "gen 0 is dead" — a scheduler conclusion drawn from a stopwatch. Found by the
    // nightly's first ASAN run (2026-09-02); it is not reachable locally because the `jit`
    // binary ahead of it exhausts any sane local timeout first.
    #[cfg(brood_asan)]
    let budget = Duration::from_secs(60);
    #[cfg(not(brood_asan))]
    let budget = Duration::from_secs(5);
    let deadline = Instant::now() + budget;
    let drained = loop {
        if process::old_gen_drained(&interp.heap) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(
        drained,
        "after the worker exits, every live process has reported clean → gen 0 is dead",
    );
}

/// Stage 4 — **freeing a drained generation reclaims its slot**. Once the Stage-3
/// union confirms no live process references the old generation, `free_runtime_gen`
/// stores a fresh empty slab into its ArcSwap slot. Driven with two heaps sharing one
/// runtime: the free is refused while a peer still pins the generation, and succeeds —
/// emptying the slot so aging can reuse it — once the pin is released.
#[test]
fn free_reclaims_after_cross_process_drain() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    interp.eval_str("(defn f (x) (* x 2))").expect("define f");
    let f_gen0 = interp.eval_str("f").expect("resolve f");
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp
        .eval_str("(defn f (x) (+ (* x 3) 7))")
        .expect("redefine f");
    interp.heap.collect(&mut [], &mut []);
    assert!(!interp.heap.runtime_gen_referenced(0));

    let mut child = Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    child.push_root(f_gen0);
    assert!(child.runtime_gen_referenced(0), "child pins gen 0");

    const MAIN: u64 = 1;
    const CHILD: u64 = 2;
    interp.heap.begin_gen_drain(0);
    interp.heap.report_gen_liveness(MAIN);
    child.report_gen_liveness(CHILD);

    // The child still pins gen 0 → the drain gate says NOT drained, so a caller
    // (`free_drained_gen`) would not free. Aging back into slot 0 is refused too — it
    // is non-empty (the 2-versions rule).
    assert!(!interp.heap.gen_drained(&[MAIN, CHILD]), "child pins gen 0");
    assert!(
        !interp.heap.age_runtime(),
        "cannot age into non-empty slot 0"
    );

    // Release the pin; now the generation is drained and safe to free.
    child.truncate_roots(0);
    child.report_gen_liveness(CHILD);
    assert!(
        interp.heap.gen_drained(&[MAIN, CHILD]),
        "gen 0 is now drained"
    );

    // Free it: the slot empties, the drain ends, and aging can reclaim slot 0.
    assert!(
        interp
            .heap
            .free_runtime_gen(0, interp.heap.drain_identity().0),
        "free the drained gen 0"
    );
    assert!(!interp.heap.drain_active(), "the drain ended");
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "gen 0 slot is empty after the free",
    );
    assert!(
        interp.heap.age_runtime(),
        "slot 0 is now empty → aging can reuse it",
    );
    assert_eq!(interp.heap.runtime_cur_gen(), 0);
}

/// Stage 4 — **a reused slot never serves stale compiled code**. Freeing a generation
/// and aging back into its slot mints handles with bit-identical `(gen, index)` to the
/// freed ones. `vm_cache` keys on those bits (not `version`), so without the
/// `free_epoch` lazy-clear a process could execute the *old* closure's compiled body
/// for the *new* closure. This drives a real define → call (populating `vm_cache`) →
/// free → reuse → call and asserts the new code runs.
#[test]
fn reused_slot_runs_new_code_not_stale_cache() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // Gen 0: define `f` and CALL it, so its compiled body is cached under its
    // (gen 0, index) handle.
    interp.eval_str("(defn f (x) (* x 2))").expect("define f");
    {
        let r = interp.eval_str("(f 5)").unwrap();
        assert_eq!(interp.print(r), "10");
    }

    // Age, supersede `f` into gen 1, drop the gen-0 remnant. Gen 0 is now unreferenced
    // (its old `f` lingers only in the excluded-from-liveness `vm_cache`).
    assert!(interp.heap.age_runtime());
    interp
        .eval_str("(defn f (x) (+ x 100))")
        .expect("redefine f into gen 1");
    interp.heap.collect(&mut [], &mut []);
    assert!(!interp.heap.runtime_gen_referenced(0));

    // Free gen 0 (bumps `free_epoch`), then age back into the freed slot 0 and define a
    // NEW `f` there — its handle can reuse the old `f`'s `(gen 0, index)` bits.
    assert!(
        interp
            .heap
            .free_runtime_gen(0, interp.heap.drain_identity().0),
        "free drained gen 0"
    );
    assert!(interp.heap.age_runtime(), "age back into the freed slot 0");
    assert_eq!(interp.heap.runtime_cur_gen(), 0);
    interp
        .eval_str("(defn f (x) (- x 1))")
        .expect("define a new f in the reused slot");

    // The new `f` must run — NOT the stale gen-0 `(* x 2)` a bit-identical `vm_cache`
    // key would otherwise serve.
    {
        let r = interp.eval_str("(f 5)").unwrap();
        assert_eq!(
            interp.print(r),
            "4",
            "the reused slot runs the new f (5-1), not the stale cached (* 5 2)=10"
        );
    }
}

/// Stage 4 — **migration lets a generation with a stable global still be freed**. This
/// is the case that *stalls* without live-globals migration: a global defined once and
/// never redefined stays in its birth generation, pinning it forever (Brood's `def` is
/// per-global, unlike an Erlang module reload that re-exports every function). After
/// `migrate_live_globals` re-exports the live globals into the current generation, the
/// aged-out one is unreferenced and freeable — while every binding keeps working.
#[test]
fn migration_re_exports_globals_so_a_stable_global_does_not_pin() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // A stable global (defined once) and a churny one (redefined a few times), all in
    // generation 0.
    interp
        .eval_str("(defn stable (x) (+ x 1000))")
        .expect("stable");
    for k in 0..5 {
        interp
            .eval_str(&format!("(defn f (x) (+ x {k}))"))
            .expect("define f");
    }
    assert_eq!(interp.heap.runtime_cur_gen(), 0);

    // Age into generation 1. Nothing has moved yet — every global still lives in gen 0,
    // so without migration gen 0 would stay pinned by `stable` forever.
    assert!(interp.heap.age_runtime(), "age to gen 1");
    let migrated = interp.heap.migrate_live_globals(0);
    assert!(
        migrated >= 2,
        "expected ≥2 live globals (stable + f + …) migrated, got {migrated}",
    );

    // Both globals still compute — now resolving to their gen-1 copies.
    {
        let r = interp.eval_str("(stable 5)").unwrap();
        assert_eq!(interp.print(r), "1005");
    }
    {
        let r = interp.eval_str("(f 5)").unwrap();
        assert_eq!(interp.print(r), "9", "f is the last redef (+ x 4)");
    }

    // A LOCAL collect first: the eval results from the `def`s left gen-0 closure handles
    // as garbage in this process's LOCAL heap (the liveness walk conservatively counts
    // every LOCAL cell, so unreclaimed garbage still pins the generation). A real process
    // reclaims this at its safepoint before reporting clean — do the same here.
    interp.heap.collect(&mut [], &mut []);

    // THE point: with the live globals migrated off it, generation 0 is unreferenced —
    // so it can be freed (without migration, `stable` would keep this true forever).
    assert!(
        !interp.heap.runtime_gen_referenced(0),
        "gen 0 is unreferenced after migration — a stable global no longer pins it",
    );
    assert!(
        interp
            .heap
            .free_runtime_gen(0, interp.heap.drain_identity().0),
        "free the drained gen 0"
    );

    // Everything still works after the free, and fresh code runs on the compacted region.
    {
        let r = interp.eval_str("(stable 7)").unwrap();
        assert_eq!(interp.print(r), "1007", "stable survives the free");
    }
    {
        let r = interp.eval_str("(f 7)").unwrap();
        assert_eq!(interp.print(r), "11");
    }
    {
        let r = interp.eval_str("(defn g (a) (* a a)) (g 6)").unwrap();
        assert_eq!(interp.print(r), "36", "fresh code runs post-free");
    }
}

/// Stage 4 — **migration never clobbers a binding redefined after aging**. The reconcile
/// installs a migrated handle only where the global still resides in the aged-out
/// generation; a redefinition (which lands in the *current* generation) is left alone.
/// This drives the ordering deterministically: age, redefine `g` into the new
/// generation, then migrate — `g` must keep the redefinition, not revert to the copy of
/// its old-generation body.
#[test]
fn migration_preserves_a_post_aging_redefinition() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    interp.eval_str("(defn g (x) (* x 10))").expect("g gen 0");
    interp
        // `keeper`, not `keep`: `keep` is a prelude function and reserved (ADR-166).
        .eval_str("(defn keeper (x) (- x 1))")
        .expect("keep gen 0");
    assert!(interp.heap.age_runtime(), "age to gen 1");

    // A redefinition after aging lands in generation 1 (the current generation).
    interp
        .eval_str("(defn g (x) (+ x 1))")
        .expect("redefine g into gen 1");

    // Migrate the still-in-gen-0 globals (`keep`). `g` is already in gen 1, so it is not
    // a migration candidate and the reconcile leaves it untouched.
    let migrated = interp.heap.migrate_live_globals(0);
    assert!(migrated >= 1, "keep should migrate, got {migrated}");

    {
        let r = interp.eval_str("(g 5)").unwrap();
        assert_eq!(
            interp.print(r),
            "6",
            "g keeps its post-aging redefinition (+ 5 1), not the migrated (* 5 10)",
        );
    }
    {
        let r = interp.eval_str("(keeper 5)").unwrap();
        assert_eq!(interp.print(r), "4", "keep was migrated intact");
    }
}

/// Stage 4 — **age → migrate → drain → free cycles repeatedly and stays bounded**. Two
/// heaps share one runtime; across several cycles the region is churned, aged, its live
/// globals migrated, the vacated generation drained (both processes report clean) and
/// freed. The current-generation closure count must stay bounded across cycles — the
/// proof that whole-generation reclamation actually converges, not just runs once.
#[test]
fn migration_drain_free_cycles_and_stays_bounded() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);
    interp.eval_str("(defn f (x) (+ x 0))").expect("seed f");

    // A peer heap sharing the runtime — it holds no old-generation references, so it
    // reports clean every cycle (its role is to keep the runtime genuinely shared).
    let child = Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());

    const MAIN: u64 = 1;
    const CHILD: u64 = 2;
    let mut counts = Vec::new();
    for cycle in 0..4 {
        // Churn: redefine `f` several times, superseding versions in the current gen.
        for k in 0..8 {
            interp
                .eval_str(&format!("(defn f (x) (+ x {})) ", cycle * 10 + k))
                .expect("redef f");
        }
        let old = interp.heap.runtime_cur_gen();
        assert!(interp.heap.age_runtime(), "cycle {cycle}: age");
        interp.heap.migrate_live_globals(old);
        // Reclaim this process's LOCAL gen-`old` garbage (the churn's eval results) so it
        // reports clean — mirrors a real process's safepoint LOCAL GC before it reports.
        interp.heap.collect(&mut [], &mut []);
        interp.heap.begin_gen_drain(old);

        // Both processes report clean (neither holds an old-gen handle after migration).
        interp.heap.report_gen_liveness(MAIN);
        child.report_gen_liveness(CHILD);
        assert!(
            interp.heap.gen_drained(&[MAIN, CHILD]),
            "cycle {cycle}: gen {old} drained",
        );
        assert!(
            interp
                .heap
                .free_runtime_gen(old, interp.heap.drain_identity().0),
            "cycle {cycle}: free gen {old}",
        );

        // `f` still computes the latest redefinition on the freshly-migrated generation.
        let r = interp.eval_str("(f 100)").unwrap();
        let want = 100 + cycle * 10 + 7;
        assert_eq!(interp.print(r), want.to_string(), "cycle {cycle}: f works");
        counts.push(interp.heap.runtime_closure_count());
    }

    // Convergence: the current-generation live count doesn't grow cycle over cycle —
    // each freed generation reclaims the prior cycle's churn.
    let max = *counts.iter().max().unwrap();
    assert!(
        max < 64,
        "current-gen closures stayed bounded across cycles ({counts:?}), got max {max}",
    );
}

/// Stage 5 soundness (ADR-091) — a `def` of a value resident in the *draining*
/// generation must be re-homed into the current generation. Otherwise it stores a
/// stale handle into the shared globals table (an un-walked drain root), re-pinning
/// a generation a process already acked clean; freeing that generation then leaves
/// the global dangling. Here a straggler binds a gen-0 handle *after* migration moved
/// the live globals off gen 0; with the re-home it lands in gen 1 and survives the
/// free, without it it dangles into the emptied gen-0 slab.
#[test]
fn a_def_of_an_old_gen_value_is_rehomed_off_the_freed_generation() {
    LazyLock::force(&MEM_GUARD);
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);

    // f is born in gen 0. Capture its gen-0 handle before aging (a process could
    // likewise still hold such a value across the flip).
    interp.eval_str("(def f (fn () 42))").expect("def f");
    let g_env = interp.heap.global();
    let f_gen0 = interp
        .heap
        .env_get(g_env, brood::core::value::intern("f"))
        .expect("f is bound");

    // Age to gen 1 and migrate the live globals off gen 0. gen 0 now holds only the
    // superseded original — unreferenced by any *global*, but we still hold `f_gen0`.
    assert!(interp.heap.age_runtime(), "age to gen 1");
    interp.heap.migrate_live_globals(0);
    interp.heap.collect(&mut [], &mut []);

    // The straggler: bind a NEW global `g` to the gen-0 handle (what `(def g <old>)`
    // does after a departed process handed the value on).
    let g_sym = brood::core::value::intern("g");
    interp.heap.env_define(g_env, g_sym, f_gen0);
    interp.heap.collect(&mut [], &mut []);

    // Free gen 0. With the re-home fix `g` was copied into gen 1, so this is safe;
    // without it `g` still points into the now-empty gen-0 slab.
    assert!(
        interp
            .heap
            .free_runtime_gen(0, interp.heap.drain_identity().0),
        "gen 0 freed"
    );

    // `g` must still resolve + run — proving it was re-homed off the freed generation.
    let r = interp.eval_str("(g)").expect("call g after gen 0 freed");
    assert_eq!(
        interp.print(r),
        "42",
        "g was re-homed into the live generation"
    );
}

/// KI-86's actual mechanism, pinned. A heap that switched the runtime collector OFF must
/// stay off across a bulk-promote re-baseline (`rt_gc_rebaseline_all_live`, which the stdlib
/// image install calls after materialising a module): before the fix that call replaced the
/// `usize::MAX` opt-out with `max(floor, 2 * live)`, so a process-wide `BROOD_RT_GC_FLOOR=128`
/// made `superseded_global_versions_are_reclaimable` compact mid-count (`total=213`,
/// deterministically, with zero scheduler activity). Sabotage: drop the `usize::MAX` early
/// return in `rt_gc_rebaseline_all_live` and this fails.
#[test]
fn an_opted_out_heap_stays_opted_out_across_a_rebaseline() {
    let mut interp = Interp::new();
    interp.heap.set_rt_auto_collect(false);
    assert_eq!(
        interp.heap.rt_gc_threshold(),
        usize::MAX,
        "opt-out sets the sentinel"
    );
    interp
        .eval_str("(defn a (x) x) (defn b (x) (a x)) (defn c (x) (b x))")
        .expect("a few promotions");
    interp.heap.rt_gc_rebaseline_all_live();
    assert_eq!(
        interp.heap.rt_gc_threshold(),
        usize::MAX,
        "a re-baseline must not re-arm a collector the caller switched off (KI-86)"
    );
    // The complement: an ARMED heap is re-baselined (the ADR-218 behaviour is intact).
    interp.heap.set_rt_auto_collect(true);
    interp.heap.rt_gc_rebaseline_all_live();
    assert!(
        interp.heap.rt_gc_threshold() < usize::MAX,
        "an armed heap takes the finite max(floor, 2 * live) threshold"
    );
}
