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
            // JIT tests exercise the VM's native tier, so pin the VM regardless of the
            // `BROOD_VM` env: the tree-walker differential job sets `BROOD_VM=0`, under
            // which these top-level programs would run on the tree-walker — testing no
            // JIT at all, and ~10× slower (timing out). Thread-local, set in-thread.
            brood::eval::compile::set_forced_ceiling(Some(brood::eval::compile::Tier::Native));
            let mut interp = Interp::new();
            match interp.eval_str(src) {
                Ok(v) => Ok(interp.print(v)),
                Err(e) => Err(e.message.clone()),
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
        // `countdown`, not `dec`: `dec` is a prelude function and reserved (ADR-166).
        "(defn countdown (i acc) (if (< i 1) acc (countdown (- i 1) (- acc 1))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (countdown 1 -9223372036854775808))))
         (run 50000 0)",
        "-9223372036854775809",
    );
}

#[test]
fn comparisons_and_maps_are_correct_under_jit() {
    // Runs in the main thread (not via `run`), so pin the VM here too — see `run`.
    brood::eval::compile::set_forced_ceiling(Some(brood::eval::compile::Tier::Native));
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
            .map_err(|e| e.message.clone());
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
fn redefining_a_sealed_operator_is_refused_after_tiering() {
    // Was `redefining_an_operator_after_tiering_is_honored`: it warmed `f` until the
    // JIT inlined `+` as a raw machine add, then redefined `+` to prove the epoch
    // guard invalidated the tiered arm. ADR-166 reserves every shipped function, so
    // that rebinding is now refused — which makes baking the add in sound *by
    // construction* rather than by guard. (Follow-up recorded in ADR-166: the guard
    // becomes removable for reserved names, which is the early-binding headroom
    // sealing was meant to buy.)
    is(
        "(defn f (x) (+ x 1))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (f 100))))
         (warm 50000 0)
         (try (do (def + (fn (a b) (* a b))) :rebound) (catch e :refused))",
        ":refused",
    );
}

#[test]
fn redefining_a_user_fn_after_tiering_is_honored() {
    // The property the sealed tests really guarded, retargeted at a name that is still
    // rebindable — and *this* is the case that matters for live editing: your own
    // function, redefined while a JIT'd caller is hot. It has to exist explicitly now,
    // because every name the previous epoch-guard tests used (`+`, `type-of`) is
    // reserved, which would otherwise have left the guard with no coverage at all.
    is(
        "(defn g (a b) (+ a b))
         (defn f (x) (g x 1))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (f 100))))
         (warm 50000 0)
         (def g (fn (a b) (* a b)))
         (f 5)", // new g: 5 * 1 = 5
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

// NOTE: the self-inliner is **default-ON** (two-stage tiering, devlog 2026-06-17), so an
// ordinary `cargo test --features jit --test jit` exercises it. The three `inlined_*` tests
// below pass either way — the result is engine-independent — so run them with
// `BROOD_NO_INLINE=1` too when the question is whether the inliner changed an answer.
// (This note read the opposite until 2026-09-04: it named a `BROOD_JIT_INLINE=1` opt-in the
// runtime stopped reading when the default flipped, i.e. it told you to arm nothing.)
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
             (if (= (math/rem n 2) 0)
               (+ 1 (cc (math/quot n 2)))
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
        "1/4", // 1 / (s 2 2) = 1/4 — exact since ADR-196; the bug this guards gave `4`
    );
}

#[test]
fn inlined_two_stage_swap_then_deopt_stays_correct() {
    // Two-stage tiering (devlog 2026-06-17): a qualifying recursive arm tiers to the SMALL
    // original native first, then the deferred *inlined* upgrade compiles and swaps in (an
    // epoch-bumped `jit_code` swap + per-engine frame growth to `inline_nslots`). This run
    // drives `fib 30` 4000× — far past both the small-arm threshold AND the deferred
    // inlined-compile latency, so the arm provably runs small-native, then inlined-native.
    // fib(30) = 832040, well within i64, so it returns via the inlined native path; the
    // result must be bit-identical to the interpreter (the swap + the bigger frame must not
    // corrupt the params or the spilled call results).
    is(
        "(defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (fib 30))))
         (run 4000 0)",
        "832040",
    );
}

#[test]
fn inlined_arm_deopts_to_bignum_after_swap() {
    // The swap→inlined→deopt path. `p` is a depth-1-inlinable non-tail recursion
    // (`(* 2 (p (- n 1)))`); warmed with `p 40` (in i64) so the small native tiers and the
    // deferred inlined upgrade swaps in. The final `(p 70)` computes 2^70, which overflows
    // i64 — so the inlined native must DEOPT mid-recursion (an inner `*` overflows), restore
    // the small frame from the param `n`, re-run on the VM, and propagate the exact bignum
    // the interpreter would. Guards that the per-engine frame transition + the swapped
    // inlined arm stay correct on the overflow→deopt boundary.
    is(
        "(defn p (n) (if (< n 1) 1 (* 2 (p (- n 1)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (p 40))))
         (do (run 50000 0) (p 70))",
        "1180591620717411303424",
    );
}

#[test]
fn integer_division_family_under_jit() {
    // rem / quot mixed with mul / add — the classic collatz step counter, fully in the
    // (now division-capable) int subset. collatz(27) takes 111 steps.
    is(
        "(defn cstep (n steps)
           (if (= n 1) steps
             (if (= (math/rem n 2) 0)
               (cstep (math/quot n 2) (+ steps 1))
               (cstep (+ (* 3 n) 1) (+ steps 1)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (cstep 27 0))))
         (run 20000 0)",
        "111",
    );
    // rem/quot on positive and negative operands.
    is(
        "(defn r (a) (math/rem a 5))
         (defn run (k last) (if (< k 1) last (run (- k 1) (r 17))))
         (list (do (run 20000 0) (r 17)) (r -17))",
        "(2 -2)",
    );
}

#[test]
fn exact_division_inlines_inexact_deopts_to_ratio() {
    // `%div` (`/`) yields an Int only on an exact quotient; a remainder means a value the
    // native can't hold, so the JIT must deopt then. Since ADR-196 that value is an exact
    // RATIO rather than a Float. Warm `(/ 24 x)`, then probe exact (4, 6) and inexact
    // (5 → 24/5, deopt → VM ratio). Matches the VM exactly.
    is(
        "(defn d (x) (/ 24 x))
         (defn run (k last) (if (< k 1) last (run (- k 1) (d 4))))
         (list (do (run 20000 0) (d 4)) (d 6) (d 5))",
        "(6 4 24/5)",
    );
}

#[test]
fn division_by_zero_deopts_to_the_same_error() {
    // A warmed division arm hitting a zero divisor must deopt and raise the VM's exact
    // error (Cranelift's srem would *trap*/abort if we hadn't guarded it).
    let err = run("(defn r (a b) (math/rem a b))
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
        "(defn q (a b) (math/quot a b))
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

#[test]
fn warmed_recursive_fib_matches_vm() {
    // The straight (non-inlined) recursive fib — the headline Track-B target. Both
    // self-calls `(fib (- n 1))` / `(fib (- n 2))` are non-tail free-global calls, so under
    // `BROOD_JIT_ICALL=1` they take the in-IR epoch-guarded fast-link path
    // (`brood_rt_fast_frame`) instead of `brood_rt_call_slow`. `fib 30` makes ~2.7M such
    // calls in one go, so `fib` tiers and the calls fast-link mid-run; the result must stay
    // bit-identical to the VM (a dispatch desync would be a silent wrong answer). With the
    // env unset this is the plain slow-dispatch path (the A/B baseline) — both must give
    // 832040.
    is(
        "(defn fib (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
         (fib 30)",
        "832040",
    );
}

#[test]
fn non_tail_mutual_recursion_fast_links_both_heads() {
    // Two DISTINCT free-global callees calling each other in non-tail position (`(+ 1 (b …))`
    // / `(+ 1 (a …))`) — two separate call sites with two different head symbols, so the
    // flat fast-link table must serve each site its own `(code, env)` (a site→sym mix-up
    // would cross-call). Warmed 100k× so both `a` and `b` tier and fast-link. `a 50`
    // bottoms out at 0 after 50 alternating steps → 50; must match the VM under both
    // `BROOD_JIT_ICALL` on and off.
    is(
        "(defn a (n) (if (< n 1) 0 (+ 1 (b (- n 1)))))
         (defn b (n) (if (< n 1) 0 (+ 1 (a (- n 1)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (a 50))))
         (run 100000 0)",
        "50",
    );
}

#[test]
fn fast_linked_callee_uses_its_own_ic_block() {
    // KI-20 regression. A JIT fast link (`jit_run_fast_link`) enters the callee's native
    // code; before the fix it left `ic_bases` pointing at the CALLER's per-arm inline-cache
    // block, so the callee resolved its own call sites / global reads against the caller's
    // slots (and vice versa). Never a wrong answer — every IC read re-validates
    // `sym`/`argc`/`epoch`, so a crossed entry just misses — but both arms ran cache-cold.
    // A three-level chain where each hot arm fast-links the next in NON-tail position and the
    // callee then makes its OWN fast-linked call: `outer`→`middle`→`inner`, each a distinct
    // free-global head, so each has a distinct IC block. If `middle` ran under `outer`'s base
    // its call to `inner` would read the wrong slot; the crossing is silent, so the guard is
    // the answer plus the debug cross-check in `jit_dispatch_fast_frame` (asserts the
    // IR-passed bases equal the authoritative IC's on every fast-frame call in debug builds).
    // Warmed past tiering so all three arms are native and fast-link. `inner 4` = 4;
    // `middle n` sums `inner 4` down the chain; `outer 8` alternates; must match the VM.
    is(
        "(defn inner (n) (if (< n 1) 0 (+ 1 (inner (- n 1)))))
         (defn middle (n) (if (< n 1) 0 (+ (inner 4) (middle (- n 1)))))
         (defn outer (n) (if (< n 1) 0 (+ (middle 3) (outer (- n 1)))))
         (defn run (k last) (if (< k 1) last (run (- k 1) (outer 8))))
         (run 100000 0)",
        "96", // outer 8 → 8×(middle 3) → 8×3×(inner 4)=8×3×4 = 96
    );
}

#[test]
fn redefining_a_fast_linked_callee_is_honored() {
    // Late binding across the fast-link: warm `f` (a non-tail caller of `g`) so the call
    // site fast-links to `g`'s native code, then `def` a new `g`. The epoch bump must
    // invalidate the flat-table slot (stale epoch → IR miss → re-resolve), so the post-`def`
    // call sees the NEW `g`. A fast-link that ignored the epoch would keep calling the old
    // code — a silent wrong answer.
    is(
        "(defn g (x) (+ x 1))
         (defn f (x) (+ 0 (g x)))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (f 10))))
         (warm 100000 0)
         (def g (fn (x) (* x 100)))
         (f 10)",
        "1000", // new g: 10*100; (+ 0 1000)
    );
}

// ===== leaf-callee inlining (default ON since 2026-07-19; BROOD_NO_LEAF_INLINE opts out) =====
//
// Leaf inlining is on by default, so these tests exercise the shipping configuration
// directly — no env flag needed. (They used to set the opt-in `BROOD_JIT_LEAF_INLINE`;
// the lever is now the opt-out `BROOD_NO_LEAF_INLINE`, cached once per process.)
// Leaf inlining must be semantics-preserving, which is exactly what every test here
// (and every other test in this file) asserts.

#[test]
fn leaf_inlined_helpers_stay_correct() {
    // The target shape: a hot fixed-arity defn whose non-tail calls all resolve to
    // small calls-free helpers. The derivation splices both callees (the residual-call
    // gate requires ALL non-tail calls gone), the arm tiers small-native, then the
    // deferred leaf upgrade swaps in with its (floored) `inline_nslots` frame. The sum
    // must be bit-identical to the interpreter across the whole small→leaf transition.
    is(
        "(defn add1 (n) (+ n 1))
         (defn sq (x) (* x x))
         (defn work (i acc) (if (>= i 200000) acc (work (+ i 1) (+ acc (sq (add1 i))))))
         (work 0 0)",
        "2666686666700000",
    );
}

#[test]
fn leaf_inlined_helper_redef_takes_effect() {
    // Hot reload across a leaf splice: warm `work` so the leaf upgrade (which baked
    // add1's OLD body into work's native code) installs, then `def` a new `add1`.
    // The def bumps the global epoch → the installed native (epoch-guarded per entry)
    // invalidates, and the re-lower refuses the stale derivation (`leaf.epoch`
    // mismatch), so the post-def call runs the NEW add1 — late binding exact.
    is(
        "(defn add1 (n) (+ n 1))
         (defn work (i acc) (if (>= i 100000) acc (work (+ i 1) (+ acc (add1 i)))))
         (work 0 0)
         (def add1 (fn (n) (+ n 1000)))
         (work 99998 0)",
        "201997", // (add1 99998) + (add1 99999) with the NEW add1 = 100998 + 100999
    );
}

#[test]
fn leaf_inline_residual_call_gate_keeps_correctness() {
    // A caller with one leaf-shaped callee AND one non-leaf callee (recursive `deep`):
    // the residual-call gate refuses the derivation (a remaining non-tail call would
    // make the checkpoint-less from-ip-0 deopt re-run unsafe), so the arm keeps its
    // small native + checkpointing. Either way the answer must match the interpreter.
    is(
        "(defn add1 (n) (+ n 1))
         (defn deep (n) (if (< n 1) 0 (+ 1 (deep (- n 1)))))
         (defn work (i acc) (if (>= i 20000) acc (work (+ i 1) (+ acc (add1 i) (deep 3)))))
         (work 0 0)",
        "200070000",
    );
}

#[test]
fn type_of_prim_covers_every_shape_hot() {
    // `type-of` is now `PrimOp1::TypeOf` (total — no deopt): a hot arm classifying a
    // mix of shapes must agree with the interpreter across compile-time-known
    // operands (int/float/bool consts) AND type-erased ones (slot reads whose tag is
    // resolved by the discriminant-byte table at runtime), including the collapsing
    // rules (Range → :pair) and an i8 comparison result boxing as :bool, not :int.
    is(
        "(defn code (x) (if (%eq (type-of x) :int) 1
                        (if (%eq (type-of x) :vector) 2
                         (if (%eq (type-of x) :pair) 3
                          (if (%eq (type-of x) :string) 4
                           (if (%eq (type-of x) :bool) 5 0))))))
         (defn work (i acc)
           (if (>= i 60000) acc
             (work (+ i 1)
               (+ acc (code (if (%eq (math/rem i 5) 0) 7
                             (if (%eq (math/rem i 5) 1) [1]
                              (if (%eq (math/rem i 5) 2) (range 3)
                               (if (%eq (math/rem i 5) 3) \"s\"
                                (< 1 2))))))))))
         (work 0 0)",
        "180000", // 12000 × (1+2+3+4+5)
    );
}

#[test]
fn type_of_prim_redef_is_refused() {
    // Was `type_of_prim_redef_falls_back`: it warmed the arm (TypeOf is epoch-guarded
    // like every PrimOp1) and then shadowed `type-of` with a user `defn`, asserting the
    // guard dispatched the new binding. A shipped prim is reserved (ADR-166), so the
    // shadowing is refused and the baked-in prim stays correct — the inlining is sound
    // because the binding is immutable, not because a guard catches it.
    is(
        "(defn probe (i acc) (if (>= i 50000) acc (probe (+ i 1) (+ acc (if (%eq (type-of i) :int) 1 0)))))
         (probe 0 0)
         (try (do (def type-of (fn (x) :shadowed)) :rebound) (catch e :refused))",
        ":refused",
    );
}

#[test]
fn type_mixed_join_edges_stay_exact() {
    // A join whose edges disagree on scalar typing — `(if c 7 (< 1 2))` merges an
    // unboxed int with an i8 comparison result — used to let the LAST-lowered edge
    // overwrite the block's `bool_param` typing, so the int edge's raw 7 was boxed
    // as `Value::Bool(7)` when staged as a call argument (or the bool edge stripped
    // to a raw truthy int, depending on lowering order). Now the first edge fixes
    // the typing and a disagreeing edge deopts to the VM. The classifier must see
    // Int 7 and Bool true — bit-identical to the interpreter.
    is(
        "(defn code (x) (if (%eq x 7) 1 (if (%eq x true) 5 0)))
         (defn work (i acc)
           (if (>= i 60000) acc
             (work (+ i 1) (+ acc (code (if (%eq (math/rem i 2) 0) 7 (< 1 2)))))))
         (work 0 0)",
        "180000", // 30000×1 + 30000×5 — verified against BROOD_VM=0
    );
}

// ===== partial leaf splicing (ADR-210; BROOD_NO_PARTIAL_LEAF opts out) =====
//
// A derivation may now keep a residual non-tail call beside the spliced leaves, because
// the leaf-spliced layout carries its own deopt checkpoint and a deopt resumes in the
// SPLICED chunk. Before this, one un-spliceable callee blocked inlining of every small
// callee beside it. `tests/jit_effect_once_test.blsp` cases 5–6 guard exactly-once
// effects; these guard the values.

#[test]
fn partially_spliced_arm_matches_vm() {
    // The shape: `mix` calls `add1` (small, calls-free → spliced) and `rec` (recursive →
    // never spliceable, so it survives as the residual call). Warmed hard so the deferred
    // leaf upgrade installs and the partially-spliced native runs the bulk of the loop.
    // mix(i) = (i+1) + 3, summed over i in 0..200000.
    is(
        "(defn add1 (n) (+ n 1))
         (defn rec (n acc) (if (< n 1) acc (rec (- n 1) (+ acc 1))))
         (defn mix (i) (+ (add1 i) (rec 3 0)))
         (defn work (i acc) (if (>= i 200000) acc (work (+ i 1) (+ acc (mix i)))))
         (work 0 0)",
        "20000700000", // sum(i+4, i=0..199999) — verified against BROOD_VM=0
    );
}

#[test]
fn partially_spliced_residual_callee_redef_is_honored() {
    // Hot reload through the RESIDUAL call, not the spliced one: warm `mix` so its
    // partially-spliced native installs (with `rec` still a real call), then `def` a new
    // `rec`. The epoch bump invalidates the installed native and the stale derivation, so
    // the post-def call must see the new `rec` — late binding exact, as for a spliced
    // callee (`leaf_inlined_helper_redef_takes_effect`).
    is(
        "(defn add1 (n) (+ n 1))
         (defn rec (n acc) (if (< n 1) acc (rec (- n 1) (+ acc 1))))
         (defn mix (i) (+ (add1 i) (rec 3 0)))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (mix 5))))
         (warm 200000 0)
         (def rec (fn (n acc) 100))
         (mix 5)",
        "106", // add1(5) = 6, plus the NEW rec = 100
    );
}

#[test]
fn partially_spliced_arm_deopting_every_activation_matches_vm() {
    // The stress case for the resume path: the arm deopts on EVERY activation (an i64
    // overflow after the residual call has completed), so nearly every iteration exercises
    // journal-write → deopt → resume-in-the-spliced-chunk. The overflow promotes to a
    // bignum, so the result also proves the resumed operand stack was intact.
    is(
        "(defn add1 (n) (+ n 1))
         (defn rec (n acc) (if (< n 1) acc (rec (- n 1) (+ acc 1))))
         (defn mix (i big) (+ (+ (add1 i) (rec 3 0)) (* big big)))
         (defn work (i acc big)
           (if (>= i 20000) acc (work (+ i 1) (+ acc (mix i big)) big)))
         (work 0 0 4000000000)",
        // sum(i+4) for i in 0..19999 = 200070000, plus 20000 * 4000000000^2 = 3.2e23.
        // Verified identical across all five configs: tree-walker, VM-no-JIT, VM+JIT,
        // BROOD_NO_PARTIAL_LEAF=1, and GC-stress+verify.
        "320000000000000200070000",
    );
}

#[test]
fn partially_spliced_spliced_callee_redef_is_honored() {
    // The other half of hot reload through a PARTIAL derivation: redefine the callee that
    // was *spliced* (`add1`), not the residual one. Its old body is baked into the
    // partially-spliced native and copied into the stored derivation, so both must be
    // invalidated — the epoch bump drops the native, and `leaf.epoch` no longer matches so
    // the re-lower refuses the stale derivation. Distinct path from
    // `leaf_inlined_helper_redef_takes_effect`, which has no residual call and therefore
    // no journal.
    is(
        "(defn add1 (n) (+ n 1))
         (defn rec (n acc) (if (< n 1) acc (rec (- n 1) (+ acc 1))))
         (defn mix (i) (+ (add1 i) (rec 3 0)))
         (defn warm (k last) (if (< k 1) last (warm (- k 1) (mix 5))))
         (warm 200000 0)
         (def add1 (fn (n) (* n 1000)))
         (mix 5)",
        "5003", // NEW add1: 5*1000 = 5000, plus rec 3 0 = 3
    );
}
