//! Regression for the kernel-audit #2 use-after-GC: a **tail call into a
//! different compiled arm** whose `&optional` default triggers a RUNTIME
//! compaction must register that arm as live *before* `push_frame` evaluates the
//! default — otherwise the arm's body (and its not-yet-evaluated default nodes)
//! keep handles into the region the compaction just evacuated.
//!
//! The trampoline (`vm_apply_inner`, `crates/lisp/src/eval/compile.rs`) reuses the
//! frame region on a tail call. On a switch into a *different* arm `c2`, the buggy
//! order called `push_frame(c2)` — which evaluates `c2`'s real `&optional`
//! defaults — and only *then* did `live_arm_set(slot, c2)`. A RUNTIME compaction
//! fired by that default eval (`runtime_collect` only rewrites arms in
//! `live_vm_arms`) therefore left `c2`'s compiled node tree pointing into the
//! evacuated, now-smaller region: a use-after-GC that surfaces as a corrupted
//! deref (here a spurious "parameter list" type error as a stale closure-template
//! handle is read as the wrong object) or, in release, a slab OOB / SIGSEGV.
//!
//! Lives in its own integration binary so it can drive `(runtime-collect)`
//! deterministically without interfering with other tests' process state.

use brood::Interp;

/// `f` tail-calls `g` (an arm switch). `g`'s non-nil `&optional` default forces a
/// RUNTIME compaction that reclaims ~4000 churned-away `def` versions, shrinking
/// the closures slab *under* the index of the nested-closure template literal in
/// `g`'s body. Without the fix, `g`'s arm is not yet registered when the default
/// runs, so that template handle goes stale and `g`'s body derefs the wrong /
/// out-of-bounds slot. With the fix, `g` is registered first and the compaction
/// rewrites its handles in place, so the call returns the correct result.
#[test]
fn tail_call_into_optional_default_arm_survives_runtime_compaction() {
    let mut interp = Interp::new();
    let prog = r#"
        ;; Inflate the shared RUNTIME closures slab with dead `def` versions at LOW
        ;; indices (each def supersedes the previous, so all but the last die).
        (defn churn (k) (if (= k 0) :done (do (def junk (fn () k)) (churn (- k 1)))))
        (churn 4000)
        (def marker 4242)
        ;; `g` is reached by a TAIL CALL from `f`. Its body holds a nested-closure
        ;; literal — a RUNTIME closure template minted at a HIGH index (after the
        ;; 4000 churned closures). Its non-nil `&optional` default forces a
        ;; compaction that reclaims the dead churn, shrinking the slab below that
        ;; template index.
        (defn g (a &optional (b (do (runtime-collect) 0)))
          ((fn () (str "result=" (+ a b marker)))))
        (defn f (n) (if (= n 0) (g 1) (f (- n 1))))
        (f 3)
    "#;
    let v = interp
        .eval_str(prog)
        .expect("tail-call/optional-default/compaction program errored (use-after-GC?)");
    assert_eq!(
        interp.print(v),
        "\"result=4243\"",
        "a tail call into a different arm with an &optional default that triggers a \
         RUNTIME compaction returned the wrong value — the callee arm's handles went \
         stale (kernel audit #2: live-arm must be registered before push_frame)",
    );
}
