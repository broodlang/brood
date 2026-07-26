//! JIT native code × RUNTIME compaction (ADR-091 / ADR-096 §4.C, feature `jit`).
//!
//! Fills a coverage gap identified while scoping "JIT Stage-4 — RUNTIME compaction
//! survival": there was no end-to-end test that a JIT-compiled, **RUNTIME-handle-bearing**
//! arm stays correct across an actual `(runtime-collect)` that *relocates* the handle it
//! reads.
//!
//! Why it's non-trivial: a hot arm with a quoted-list literal (`'(10 20 30)`) in its body
//! carries that list as a RUNTIME `ConstVal::Handle`. The JIT does NOT bake the handle's
//! bits into the machine code — it bakes the address of the `ConstVal` and reads the live
//! bits at each use via `brood_rt_const_load`. `(runtime-collect)` evacuates live RUNTIME
//! handles to fresh slab indices and rewrites every holder in place
//! (`rewrite_arm_handles` over `Heap::live_vm_arms`), so the arm's `ConstVal` gets the new
//! bits and the native code observes them on re-execution. This asserts that round-trip
//! produces the right value — with the arm warmed to native first.
//!
//! (Note on Stage-4 scope: today a compaction ALSO bumps `global_epoch` (`runtime.version`),
//! which lazily invalidates every installed `jit_code` on its next tier — so the arm
//! *recompiles* after the collect rather than surviving as-is. This test guards
//! *correctness* across that boundary, which holds whether the arm re-runs native-after-
//! recompile or on the VM. The "survives without recompiling" property is what Stage-4 adds;
//! this test is the correctness floor it must preserve.)
#![cfg(feature = "jit")]

use brood::Interp;

/// Evaluate `src` in a fresh interpreter on a worker-sized stack; return printed result or
/// the error message. Mirrors `tests/jit.rs`'s harness (warming loops expand deep).
fn run(src: &'static str) -> Result<String, String> {
    std::thread::Builder::new()
        .stack_size(brood::process::WORKER_STACK_BYTES)
        .spawn(move || {
            let mut interp = Interp::new();
            match interp.eval_str(src) {
                Ok(v) => Ok(interp.print(v)),
                Err(e) => Err(e.message.clone()),
            }
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked")
}

fn is(src: &'static str, want: &str) {
    assert_eq!(run(src).as_deref(), Ok(want), "diverged on:\n  {src}");
}

#[test]
fn native_arm_with_rt_handle_stays_correct_across_compaction() {
    // `prepend` carries the RUNTIME-handle literal `'(10 20 30)`. Warm it well past the
    // tiering threshold (50k activations) so the background compiler installs native code,
    // THEN `(runtime-collect)` relocates the literal, THEN call `prepend` again: the result
    // must reflect the *relocated* list, proving `brood_rt_const_load` reads the rewritten
    // bits (not stale pre-compaction bits, not a dangling slab index).
    is(
        "(defn prepend (x) (cons x '(10 20 30)))
         (defn drive (k acc) (if (< k 1) acc (drive (- k 1) (prepend k))))
         (let (warm (drive 50000 nil))
           (do (runtime-collect)
               (prepend 99)))",
        "(99 10 20 30)",
    );
}

#[test]
fn native_arm_re_reads_handle_after_repeated_compactions() {
    // The same arm across several back-to-back collects: each relocates the literal again,
    // so a stale index or a one-shot rewrite would surface here. Value is order-preserving
    // so a wrong handle is a visibly wrong list, not a silent length match.
    is(
        "(defn tag (x) (cons x '(:a :b)))
         (defn drive (k acc) (if (< k 1) acc (drive (- k 1) (tag k))))
         (let (warm (drive 50000 nil))
           (do (runtime-collect) (runtime-collect) (runtime-collect)
               (tag 7)))",
        "(7 :a :b)",
    );
}

#[test]
fn compaction_amid_churn_keeps_native_handle_reads_correct() {
    // Churn the RUNTIME region (redefine a throwaway global many times) so the collect has
    // real garbage to compact around, not just a live-only relocation. The warmed
    // handle-bearing `hold` must still read its literal correctly afterward.
    is(
        // `hold`, not `keep`: `keep` is a prelude function and reserved (ADR-166).
        "(defn hold (x) (cons x '(100 200)))
         (defn churn (k) (if (< k 1) nil (do (def junk k) (churn (- k 1)))))
         (defn drive (k acc) (if (< k 1) acc (drive (- k 1) (hold k))))
         (let (warm (drive 50000 nil))
           (do (churn 5000)
               (runtime-collect)
               (hold 1)))",
        "(1 100 200)",
    );
}
