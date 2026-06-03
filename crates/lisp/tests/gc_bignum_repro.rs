//! TEMP repro for the GC "slab out of bounds" panic surfaced by the Life demo's
//! whole-board bignum step. Run in DEBUG with the stress collector + verifier:
//!
//!   BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 BROOD_GC_TENURE=1 BROOD_GC_MAJOR=1 \
//!     cargo test -p brood --test gc_bignum_repro -- --nocapture --test-threads=1
//!
//! The crashes were in `nest mcp` / `bench` (the SAME long-lived image re-run many
//! times), so these drive many evals on ONE interp, and the spawned-process
//! (coroutine save/restore) path the existing gc tests exercise.

use brood::Interp;

/// Many evals of a bignum-churning fold on ONE interp — bench `:iterations` shape.
#[test]
fn reduce_add_repeated() {
    let mut interp = Interp::new();
    for i in 0..20 {
        let v = interp
            .eval_str("(reduce (fn (a x) (+ a x)) 0 (range 50000))")
            .unwrap_or_else(|e| panic!("iter {i}: {e:?}"));
        assert_eq!(interp.print(v), "1249975000");
    }
    println!("reduce_add_repeated ok");
}

/// Bignum bit-op churn re-run on one interp, with a persistent wide global so old
/// gen accumulates across iterations (the `bitboard/step` shape).
#[test]
fn bignum_bitop_repeated() {
    let mut interp = Interp::new();
    interp.eval_str("(def mask (- (bit-shift-left 1 4000) 1))").unwrap();
    let prog = r#"
        (defn spin (n acc)
          (if (= n 0) acc
            (spin (- n 1)
              (bit-and (bit-xor (bit-shift-left acc 1) (bit-shift-right acc 7)) mask))))
        (spin 2000 mask)
    "#;
    for i in 0..15 {
        interp.eval_str(prog).unwrap_or_else(|e| panic!("iter {i}: {e:?}"));
    }
    println!("bignum_bitop_repeated ok");
}

/// Inside a SPAWNED process (the depth-1 coroutine save/restore safepoint the gc
/// tests target) — sustained bignum churn through a `receive`-driven loop.
#[test]
fn bignum_churn_in_spawn() {
    let mut interp = Interp::new();
    let prog = r#"
        (def root (self))
        (def mask (- (bit-shift-left 1 4000) 1))
        (def worker
          (spawn
            (do
              (defn spin (n acc)
                (if (= n 0) (send root :done)
                  (spin (- n 1)
                    (bit-and (bit-xor (bit-shift-left acc 1) (bit-shift-right acc 7)) mask))))
              (spin 20000 mask))))
        (receive (:done :ok) (after 60000 :timed-out))
    "#;
    let v = interp.eval_str(prog).expect("spawn churn errored");
    assert_eq!(interp.print(v), ":ok");
    println!("bignum_churn_in_spawn ok");
}
