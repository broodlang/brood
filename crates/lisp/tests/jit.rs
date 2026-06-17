//! End-to-end JIT tests (ADR-101, feature `jit`). Each program is run through a real
//! `Interp` — so with `--features jit` the hot arms tier to native code via the
//! background compiler — and its result is asserted against the value the tree-walker /
//! bytecode VM produces. This is the differential guard for the JIT *as it actually
//! fires on compiled code*: the unit tests in `eval/compile.rs` prove the lowering
//! mechanism; these prove a warmed program stays bit-identical to the interpreter.
//!
//! Every program **warms** its hot function past the tiering threshold (8 activations)
//! by calling it from a driver loop tens of thousands of times, which also gives the
//! async background compiler ample time to install native code. Correctness holds
//! whether or not a given run has tiered yet (so these never flake), while in practice
//! the arm is native for the overwhelming majority of the iterations.
//!
//! The whole file is gated on `--features jit`: without it there is nothing JIT-specific
//! to test (the same programs run on the VM are covered by `tests/differential.rs`).
#![cfg(feature = "jit")]

use brood::Interp;

/// Evaluate `src` in a fresh interpreter on a large stack (some helpers expand to deep
/// nested-`if` trees) and return `Ok(printed)` or `Err(message)`.
fn run(src: &'static str) -> Result<String, String> {
    std::thread::Builder::new()
        .stack_size(brood::process::WORKER_STACK_BYTES)
        .spawn(move || {
            let mut interp = Interp::new();
            match interp.eval_str(src) {
                Ok(v) => Ok(interp.print(v)),
                Err(e) => Err(e.message),
            }
        })
        .expect("spawn jit test thread")
        .join()
        .expect("jit test thread panicked")
}

/// Assert a warmed program yields exactly `want`.
fn is(src: &'static str, want: &str) {
    assert_eq!(
        run(src).as_deref(),
        Ok(want),
        "JIT result diverged on:\n  {src}"
    );
}

#[test]
fn fused_int_loop_sums_correctly() {
    // `(- i 1)` → Prim2SlotInt, `(+ acc i)` → Prim2SlotSlot, `(< i 1)` → Prim2SlotInt:
    // the real fused shape. Warmed via `run` (50k activations) → native.
    is(
        "(defn sumto (i acc) (if (< i 1) acc (sumto (- i 1) (+ acc i))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (sumto 1000 0))))
         (run 50000 0)",
        "500500",
    );
}

#[test]
fn overflow_promotes_to_bignum_under_jit() {
    // An accumulating product overflows i64; the JIT must deopt on overflow so the
    // result matches the VM's BigInt promotion (a wrapping native op would diverge).
    is(
        "(defn prod (i acc) (if (< i 1) acc (prod (- i 1) (* acc i))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (prod 30 1))))
         (run 50000 0)",
        "265252859812191058636308480000000", // 30!
    );
    // Subtraction underflow → BigInt too (i64::MIN - 1).
    is(
        "(defn dec (i acc) (if (< i 1) acc (dec (- i 1) (- acc 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (dec 1 -9223372036854775808))))
         (run 50000 0)",
        "-9223372036854775809",
    );
}

#[test]
fn comparisons_and_maps_are_correct_under_jit() {
    // Each comparison lives inside an `if` so the arm tiers; `>`/`>=` lower to `%lt`/`%le`
    // with a swapped arg-map, which the JIT must apply. Warm each, probe both sides of 5.
    let cmp = |op: &str| {
        // returns "[<5> <=5> >5> >=5>]" style via a single classify per call, summed.
        format!(
            "(defn p (x) (if ({op} x 5) 1 0))
             (defn run (k a) (if (< k 1) a (run (- k 1) (p a))))
             (list (do (run 30000 3) (p 3)) (do (run 30000 5) (p 5)) (do (run 30000 9) (p 9)))"
        )
    };
    // We can't pass a String to `is` (it takes &'static str), so assert inline.
    for (op, want) in [
        ("<", "(1 0 0)"),  // 3<5,5<5,9<5
        ("<=", "(1 1 0)"), // 3<=5,5<=5,9<=5
        (">", "(0 0 1)"),  // 3>5,5>5,9>5    (map [1,0])
        (">=", "(0 1 1)"), // 3>=5,5>=5,9>=5 (map [1,0])
        ("=", "(0 1 0)"),  // 3=5,5=5,9=5
    ] {
        let src = cmp(op);
        let mut interp = Interp::new();
        let got = interp
            .eval_str(&src)
            .map(|v| interp.print(v))
            .map_err(|e| e.message);
        assert_eq!(
            got.as_deref(),
            Ok(want),
            "comparison `{op}` diverged under JIT"
        );
    }
}

#[test]
fn negative_numbers_and_mixed_signs() {
    // Mul/Sub/compare with negatives — sign handling in the native ops.
    is(
        "(defn f (i acc) (if (< i 1) acc (f (- i 1) (- acc 3))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 10 0))))
         (run 50000 0)",
        "-30",
    );
    is(
        "(defn g (i acc) (if (< i 1) acc (g (- i 1) (* acc -2))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (g 5 1))))
         (run 50000 0)",
        "-32", // (-2)^5
    );
}

#[test]
fn deopt_on_non_int_operand_matches_vm() {
    // A loop whose accumulator becomes a non-int (a float) mid-stream forces the JIT's
    // tag-check deopt; the VM then carries the float. The result must match the VM.
    is(
        "(defn f (i acc) (if (< i 1) acc (f (- i 1) (+ acc 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 5 0.5))))
         (run 50000 0)",
        "5.5",
    );
}

#[test]
fn redefining_an_operator_after_tiering_is_honored() {
    // THE epoch-guard regression: warm `f` so it tiers (inlining `+` as a raw machine
    // add), then redefine `+`. A tiered arm that ignored the redefinition would still
    // add; the guard must invalidate it so `f` dispatches to the new `+` (here, `*`).
    is(
        "(defn f (x) (+ x 1))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (f 100))))
         (warm 50000 0)
         (def + (fn (a b) (* a b)))
         (f 5)", // new +: 5 * 1 = 5
        "5",
    );
}

#[test]
fn unrelated_def_after_tiering_self_heals() {
    // A `def` of an *unrelated* global bumps the global epoch, invalidating the JIT'd
    // arm; it must re-validate (`+` is still native) and recompile — not bail forever —
    // and stay correct throughout.
    is(
        "(defn f (x) (+ x 1))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (f 10))))
         (warm 50000 0)
         (def unrelated 99)
         (warm 50000 0)", // still (f 10) = 11
        "11",
    );
}

#[test]
fn nested_ifs_and_multiple_args_under_jit() {
    // A 3-way classify (nested `if`, comparisons with two different constants) inside a
    // tiering arm, plus a 3-arg loop, exercise the CFG + frame-slot handling.
    is(
        "(defn sign (x) (if (< x 0) -1 (if (= x 0) 0 1)))
         (defn run (k a) (if (< k 1) a (run (- k 1) (+ (sign -7) (+ (sign 0) (sign 12))))))
         (run 50000 0)",
        "0", // -1 + 0 + 1
    );
    is(
        "(defn f (i j acc) (if (< i 1) acc (f (- i 1) (+ j 1) (+ acc j))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 5 0 0))))
         (run 50000 0)",
        "10", // 0+1+2+3+4
    );
}

#[test]
fn deep_handle_spill_under_jit() {
    // Multi-slot handle spill (`docs/jit-optimizing-tier.md` §6b prerequisite). A
    // right-nested `(+ (g a) (+ (g b) (+ (g c) (g d))))` keeps THREE call-result handles
    // live across later call safepoints at once, so the arm needs 3 spill slots. Before
    // the liveness-driven `jit_spill_reserve` (it was a hardcoded `1`) this bailed to the
    // VM at the second spill; now it lowers natively. Warmed 50k× so it tiers; the result
    // must stay bit-identical to the interpreter. (`g` is `(* x 2)` so the answer is
    // deterministic and independent of evaluation order.)
    is(
        "(defn g (x) (* x 2))
         (defn h (n) (+ (g n) (+ (g (+ n 1)) (+ (g (+ n 2)) (g (+ n 3))))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (h 10))))
         (run 50000 0)",
        "92", // 2*(10 + 11 + 12 + 13) = 2*46
    );
}

#[test]
fn inlined_recursive_fib_under_jit() {
    // Recursive self-inlining (`docs/jit-optimizing-tier.md` §6b, Phase B). The non-tail
    // self-calls in `fib`'s body are spliced depth-1 into the arm's own frame (shifted
    // slot ranges), so the inlined arm has 4 leaf `Call`s + 3 live call-result handles
    // spilled across safepoints. A missed slot-shift in `shift_slots` is a silent wrong
    // result, so the warmed JIT result must stay bit-identical to the interpreter.
    // fib(20) = 6765; driven 50k× so the inlined arm tiers to native.
    is(
        "(defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (fib 20))))
         (run 50000 0)",
        "6765",
    );
}

#[test]
fn inlined_recursive_collatz_under_jit() {
    // Self-inlining over a body with two non-tail recursive call shapes (the two `if`
    // arms), exercising `shift_slots` across nested `if`/`Prim2` and the integer-division
    // family. collatz-count(27) = 111. Warmed 50k×.
    is(
        "(defn cc (n)
           (if (= n 1) 0
             (if (= (rem n 2) 0)
               (+ 1 (cc (quot n 2)))
               (+ 1 (cc (+ (* 3 n) 1))))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (cc 27))))
         (run 50000 0)",
        "111",
    );
}

#[test]
fn inlined_self_call_with_tail_helper_does_not_drop_wrapper() {
    // Regression for the tail-flag bug in self-inlining (`shift_slots` must demote spliced
    // `Call`s to non-tail). `s`'s body has a non-tail self-call `(/ 1 (s b (- e)))` AND, in
    // the `else` branch, a call to a *different* fn `(r2acc …)` that sits in `s`'s TAIL
    // position (so it compiled as `tail: true`). When the self-call is inlined, that helper
    // call is spliced into the `(/ 1 …)` operand (non-tail) position; if it stayed
    // `tail: true` it would return from the whole frame and **drop the `(/ 1 …)`** — `s 2
    // -2` came back `4` (= 2^2) instead of `0.25`, failing 32 stdlib tests (`pow`, `sort`,
    // `assoc-in`, …). Warmed 50k× so the inlined arm tiers; the wrapped reciprocal must hold.
    is(
        "(defn r2acc (b e acc) (if (= e 0) acc (r2acc b (- e 1) (* acc b))))
         (defn s (b e) (cond (< e 0) (/ 1 (s b (- e))) else (r2acc b e 1)))
         (defn run (k last) (if (< k 1) last (run (- k 1) (s 2 -2))))
         (run 50000 0)",
        "0.25", // 1 / (s 2 2) = 1 / 4
    );
}

#[test]
fn integer_division_family_under_jit() {
    // rem / quot mixed with mul / add — the classic collatz step counter, fully in the
    // (now division-capable) int subset. collatz(27) takes 111 steps.
    is(
        "(defn cstep (n steps)
           (if (= n 1) steps
             (if (= (rem n 2) 0)
               (cstep (quot n 2) (+ steps 1))
               (cstep (+ (* 3 n) 1) (+ steps 1)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (cstep 27 0))))
         (run 20000 0)",
        "111",
    );
    // rem/quot on positive and negative operands.
    is(
        "(defn r (a) (rem a 5))
         (defn run (k last) (if (< k 1) last (run (- k 1) (r 17))))
         (list (do (run 20000 0) (r 17)) (r -17))",
        "(2 -2)",
    );
}

#[test]
fn exact_division_inlines_inexact_deopts_to_float() {
    // `%div` (`/`) yields an Int only on an exact quotient; a remainder means a Float the
    // native builds, so the JIT must deopt then. Warm `(/ 24 x)`, then probe exact (4, 6)
    // and inexact (5 → 4.8, deopt → VM Float). Matches the VM exactly.
    is(
        "(defn d (x) (/ 24 x))
         (defn run (k last) (if (< k 1) last (run (- k 1) (d 4))))
         (list (do (run 20000 0) (d 4)) (d 6) (d 5))",
        "(6 4 4.8)",
    );
}

#[test]
fn division_by_zero_deopts_to_the_same_error() {
    // A warmed division arm hitting a zero divisor must deopt and raise the VM's exact
    // error (Cranelift's srem would *trap*/abort if we hadn't guarded it).
    let err = run("(defn r (a b) (rem a b))
         (defn run (k last) (if (< k 1) last (run (- k 1) (r 10 2))))
         (run 20000 0)
         (r 10 0)")
    .expect_err("division by zero must error, not return");
    assert!(
        err.contains("division by zero"),
        "expected a division-by-zero error, got: {err:?}"
    );
}

#[test]
fn quot_min_over_neg1_deopts_to_bignum() {
    // `quot i64::MIN -1` overflows i64 (Cranelift sdiv would trap); the guard deopts and
    // the VM promotes to a BigInt. Warm `quot`, then hit the overflow edge.
    is(
        "(defn q (a b) (quot a b))
         (defn run (k last) (if (< k 1) last (run (- k 1) (q 100 5))))
         (do (run 20000 0) (q -9223372036854775808 -1))",
        "9223372036854775808", // 2^63, a BigInt
    );
}

#[test]
fn let_bindings_compile_and_round_trip_through_slots() {
    // A `let` binder inside a hot loop: `d` is stored into a frame slot (SetLocal) and
    // read back (Local) within the recursion. acc → acc + 2*acc = 3*acc each step.
    is(
        "(defn f (i acc) (if (< i 1) acc (let (d (* acc 2)) (f (- i 1) (+ acc d)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 10 1))))
         (run 50000 0)",
        "59049", // 3^10
    );
    // Multiple binders in one `let` + a deopt-safe re-run: an overflowing binder must
    // still produce the VM's BigInt (the slot is recomputed on the VM re-run).
    is(
        "(defn f (i acc) (if (< i 1) acc (let (a (+ acc 1) b (* acc 3)) (f (- i 1) (+ a b)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 8 1))))
         (run 50000 0)",
        "87381",
    );
    // `let` whose binder overflows mid-loop → deopt → VM recomputes the binding as BigInt.
    is(
        "(defn f (i acc) (if (< i 1) acc (let (sq (* acc acc)) (f (- i 1) sq))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 6 2))))
         (run 50000 0)",
        "18446744073709551616", // 2^64 by repeated squaring of 2, six times (overflows i64 → BigInt)
    );
}

#[test]
fn do_sequencing_under_jit() {
    // A `do` with non-final forms (Pop) inside a tiering arm. The non-final arithmetic is
    // pure so it's discarded; the loop still computes correctly.
    is(
        "(defn f (i acc) (if (< i 1) acc (f (- i 1) (do (+ acc 0) (* acc 1) (+ acc 2)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f 5 0))))
         (run 50000 0)",
        "10", // each step keeps only the last `do` form, acc -> acc+2, five steps from 0
    );
}

#[test]
fn handle_locals_carry_and_return_through_the_jit() {
    // The hybrid operand model: a *handle* (a list) lives in a frame slot and rides
    // through the loop (slot-copy on the self-call) and back out (slot → roots return).
    // Before this, `(Local xs)` eagerly tag-checked Int and deopted on a list, so any
    // handle-touching arm bailed; now it stays native. Result must match the VM.
    is(
        "(defn carry (xs n) (if (< n 1) xs (carry xs (- n 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (carry (list 1 2 3) 20))))
         (run 50000 nil)",
        "(1 2 3)",
    );
    // Returning one of two handle arguments (a Slot return, no arithmetic on the handle).
    is(
        "(defn pick3 (c x y) (if (< c 0) x y))
         (defn run (k last) (if (< k 1) last (run (- k 1) (pick3 5 (list :a) (list :b :c)))))
         (run 50000 nil)",
        "(:b :c)",
    );
    // A handle bound by `let` and returned (SetLocal copies the handle verbatim).
    is(
        "(defn f (xs n) (if (< n 1) (let (keep xs) keep) (f xs (- n 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (f (list 7 8) 10))))
         (run 50000 nil)",
        "(7 8)",
    );
}

#[test]
fn cons_builds_lists_under_jit() {
    // A cons in a hot loop allocates a pair per iteration — exercising the handle's
    // out-pointer ABI, `Op::Handle`, and the back-edge gc_safepoint that bounds the
    // nursery. `(cons n acc)` fuses to Prim2SlotSlot{Cons}; the result must match the VM.
    is(
        "(defn build (n acc) (if (< n 1) acc (build (- n 1) (cons n acc))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (build 5 nil))))
         (run 100000 nil)",
        "(1 2 3 4 5)",
    );
    // cons of a *computed* car (generic Prim2{Cons}), longer list = more GC pressure.
    is(
        "(defn sq-down (n acc) (if (< n 1) acc (sq-down (- n 1) (cons (* n n) acc))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (sq-down 6 nil))))
         (run 100000 nil)",
        "(1 4 9 16 25 36)",
    );
}

#[test]
fn car_cdr_traverse_lists_under_jit() {
    // `first`/`rest` via the handle ops, with a tag-check → deopt on a non-pair. nth
    // element of a list, traversed with rest, returned with first.
    is(
        "(defn elt (xs n) (if (< n 1) (first xs) (elt (rest xs) (- n 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (elt (list 10 20 30 40 50) 3))))
         (run 100000 0)",
        "40",
    );
    // first/rest then arithmetic on the element (a Handle used as an int → tag-check).
    is(
        "(defn sum2 (xs) (+ (first xs) (first (rest xs))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (sum2 (list 11 22 33)))))
         (run 100000 0)",
        "33",
    );
    // Walk a list down to its empty tail with `rest` — the cdr of the last pair is nil,
    // returned through the JIT; matches the VM (nil prints as `nil`).
    is(
        "(defn walk (xs n) (if (< n 1) xs (walk (rest xs) (- n 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (walk (list 1 2) 2))))
         (run 100000 nil)",
        "nil",
    );
}

#[test]
fn cons_then_traverse_round_trips_under_jit() {
    // Build a list with cons, then read it back with first/rest — both halves native.
    is(
        "(defn build (n acc) (if (< n 1) acc (build (- n 1) (cons n acc))))
         (defn elt (xs n) (if (< n 1) (first xs) (elt (rest xs) (- n 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (elt (build 8 nil) 5))))
         (run 100000 0)",
        "6", // build 8 → (1..8); elt …5 → the 6th element = 6
    );
}

#[test]
fn jit_result_matches_a_known_fib_style_accumulator() {
    // A two-accumulator tail loop (the classic iterative fib), fully in the int subset.
    is(
        "(defn fib (n a b) (if (< n 1) a (fib (- n 1) b (+ a b))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (fib 50 0 1))))
         (run 50000 0)",
        "12586269025", // fib(50)
    );
}
