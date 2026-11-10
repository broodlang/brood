//! Preemption reaches EVERY shape of running code, not just a top-level loop (ADR-400).
//!
//! `preemption.rs` proves a CPU-bound process cannot starve a peer on one worker. That hog is
//! a plain self-tail loop at the bottom of its process — the one shape the bytecode driver
//! always preempted. A body inside `try` used to run as a nested evaluation the driver could
//! not capture, so it ran to completion in one quantum however long it took: every
//! `http/serve` handler (std/net/http.blsp runs each request inside `try`) was
//! non-preemptible. Each case here puts the hog in one such shape.
//!
//! **Why a FINITE hog, raced against a responder.** One worker, quanta driven by hand
//! (`test_drive_quanta`, so load cannot perturb it), FIFO run queue. The responder parks in
//! `receive`; the hog pings it and starts its loop at once, sending `:hog-done` when the loop
//! finishes. If the hog is preempted, the responder runs next and `:pong` reaches the root
//! first; if it is not, the hog's quantum finishes the whole loop and `:hog-done` arrives
//! first. So a regression is a clean assertion failure naming the shape — not a hang — and
//! the loop is sized at many quanta' worth of reductions (2000 each), so a hog that IS
//! preempted cannot finish within its first quantum by accident.
//!
//! One `#[test]` drives every shape in sequence: the scheduler knobs are process-wide, and
//! the shapes must not share a run queue with each other.

use brood::{Interp, process};

/// Race `hog_form` (a form that loops for many quanta, then evaluates to anything) against a
/// responder on one hand-driven worker; returns which message reached the root first.
fn first_message(interp: &mut Interp, hog_form: &str) -> String {
    // The HOG sends the responder its `:ping`, immediately before the long work: the
    // responder (spawned first, parked in `receive`) becomes runnable only once the hog is
    // committed to the shape under test. Pinged from the root instead, the responder could
    // answer during whatever the hog does BEFORE reaching that shape (its first calls, a
    // tier-up), which is no evidence about the shape at all — that version passed with the
    // native unwind sabotaged.
    let setup = format!(
        r#"
        (def me (self))
        (defn responder (parent) (receive (:ping (send parent :pong))))
        (def r (spawn (responder me)))
        (spawn (do (send r :ping) {hog_form} (send me :hog-done)))
        :ok
    "#
    );
    interp.eval_str(&setup).expect("setup errored");
    let ran = std::thread::spawn(|| process::test_drive_quanta(100_000))
        .join()
        .unwrap();
    assert!(ran > 0, "expected to run at least one quantum, ran {ran}");
    let v = interp
        .eval_str("(receive (:pong :pong) (:hog-done :hog-done) (after 0 :nothing))")
        .expect("poll errored");
    let first = interp.print(v);
    // Drain the other message so the next shape starts from an empty mailbox.
    interp
        .eval_str("(receive (:pong :pong) (:hog-done :hog-done) (after 0 :nothing))")
        .expect("drain errored");
    first
}

#[test]
fn every_shape_of_running_code_is_preemptible() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    interp
        .eval_str(
            r#"
            (defn spin (k j acc) (if (>= j k) acc (spin k (+ j 1) (+ acc j))))
            (defdyn *shape-x* 0)
            ;; A set literal, a ratio and a decimal in the CALLER: each used to put the
            ;; whole function on the tree-walker, whose calls run nested and cannot yield.
            (defn set-hog (k) (let (s #{(+ k 1)}) (+ (count s) (spin k 0 0))))
            (defn ratio-hog (k) (let (r 1/3) (if r (spin k 0 0) 0)))
            (defn decimal-hog (k) (let (d 1.5M) (if d (spin k 0 0) 0)))
            :ok
        "#,
        )
        .expect("prelude errored");

    let shapes: &[(&str, &str)] = &[
        // The baseline every scheduler already got right: a bare loop.
        ("a bare self-tail loop", "(spin 200000 0 0)"),
        // ADR-400: the body of an inline `try` runs in the driver's own loop.
        ("a loop inside try", "(try (spin 200000 0 0) (catch e nil))"),
        // …and the handler, which runs in the same frame once the body has thrown.
        (
            "a loop inside a try handler",
            "(try (throw :x) (catch e (spin 200000 0 0)))",
        ),
        // A `try` with `finally` expands to a `%try` around a nested `try`.
        (
            "a loop inside try/finally",
            "(try (spin 200000 0 0) (finally nil))",
        ),
        // A body that calls (non-tail) into a function whose loop is the hog.
        (
            "a loop called from inside try",
            "(try (+ 1 (spin 200000 0 0)) (catch e nil))",
        ),
        // ADR-402: a `binding` body runs in the driver too (it was a thunk the `%binding`
        // native evaluated nested).
        (
            "a loop inside binding",
            "(binding (*shape-x* 1) (spin 200000 0 0))",
        ),
        // KI-237: a set, ratio or decimal literal no longer sends its function to the
        // tree-walker.
        (
            "a loop under a function with a set literal",
            "(set-hog 200000)",
        ),
        (
            "a loop under a function with a ratio literal",
            "(ratio-hog 200000)",
        ),
        (
            "a loop under a function with a decimal literal",
            "(decimal-hog 200000)",
        ),
        // ADR-403: `fold` walks a vector or range in Brood — per element for a reducer it
        // calls, per native span for a primitive one — so it yields like any loop.
        (
            "a vector fold with a slow reducer",
            "(reduce (into [] (range 20)) 0 (fn (acc x) (+ acc (spin 20000 0 0))))",
        ),
        (
            "a range fold with a slow reducer",
            "(reduce (range 20) 0 (fn (acc x) (+ acc (spin 20000 0 0))))",
        ),
        (
            "a long vector fold with a primitive reducer",
            "(reduce (into [] (range 400000)) 0 +)",
        ),
        (
            "a long range fold with a primitive reducer",
            "(reduce (range 3000000) 0 +)",
        ),
    ];
    let mut starved = Vec::new();
    for (name, form) in shapes {
        let first = first_message(&mut interp, form);
        if first != ":pong" {
            starved.push(format!("{name}: first message was {first}"));
        }
    }
    process::set_test_no_workers(false);
    assert!(
        starved.is_empty(),
        "a CPU-bound process starved its peer — these shapes were not preempted:\n  {}",
        starved.join("\n  ")
    );
}

/// A `receive` inside a `try` body parks the process like any other `receive` — it used to
/// block its worker thread outright (the nested evaluation could not be captured), so on one
/// worker nothing else could run until a message arrived. Here the parked process's message
/// is sent only AFTER the responder answers, so a worker blocked by the `try` would never get
/// to the responder at all: the poll would read `:nothing`.
#[test]
fn a_receive_inside_try_parks_instead_of_blocking_the_worker() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    let setup = r#"
        (def me (self))
        (def waiter (spawn (send me (try (receive (:go :received)) (catch e :caught)))))
        (defn responder (parent) (receive (:ping (send parent :pong))))
        (def r (spawn (responder me)))
        (send r :ping)
        :ok
    "#;
    interp.eval_str(setup).expect("setup errored");
    let ran = std::thread::spawn(|| process::test_drive_quanta(64))
        .join()
        .unwrap();
    assert!(ran > 0, "expected to run at least one quantum, ran {ran}");
    let v = interp
        .eval_str("(receive (:pong :pong) (after 0 :nothing))")
        .expect("poll errored");
    assert_eq!(
        interp.print(v),
        ":pong",
        "the try-wrapped receive blocked the only worker"
    );
    // The parked waiter still gets its message and resumes inside its `try`.
    interp.eval_str("(send waiter :go)").expect("send errored");
    std::thread::spawn(|| process::test_drive_quanta(64))
        .join()
        .unwrap();
    let v = interp
        .eval_str("(receive (:received :received) (:caught :caught) (after 0 :nothing))")
        .expect("poll errored");
    process::set_test_no_workers(false);
    assert_eq!(interp.print(v), ":received");
}

/// Drive `warm` on the root until every `(name argc)` arm in `arms` has settled on native
/// code, and fail if one bailed or never tiered — a shape that silently ran interpreted
/// would pass for the wrong reason. Under a tier ceiling below native (`BROOD_TIER=0|1`, the
/// interpreter CI jobs) every arm reads `:no-jit`, and the shape is still worth running.
fn settle_native(interp: &mut Interp, warm: &str, arms: &[(&str, usize)]) {
    let probe = arms
        .iter()
        .map(|(f, argc)| format!("(get (%jit-arm-state {f} {argc}) :state)"))
        .collect::<Vec<_>>()
        .join(" ");
    for _ in 0..5_000 {
        interp.eval_str(warm).expect("warm-up errored");
        let v = interp
            .eval_str(&format!("[{probe}]"))
            .expect("probe errored");
        let states = interp.print(v);
        if states
            .split_whitespace()
            .all(|s| s.contains(":native") || s.contains(":no-jit"))
        {
            return;
        }
        assert!(
            !states.contains(":bailed"),
            "an arm the shape needs native bailed: {states}"
        );
    }
    panic!("the shape's arms never tiered to native code");
}

/// The callers are loops: a caller whose only work is the call is refused by the JIT's
/// profitability gate (`call-mediated-boxed`) and would run interpreted, which
/// [`settle_native`] would flag.
const NATIVE_CHAIN: &str = r#"
    (defn n-spin (k j acc) (if (>= j k) acc (n-spin k (+ j 1) (+ acc j))))
    (defn n-caller (n i acc) (if (= i 0) acc (n-caller n (- i 1) (+ acc (n-spin n 0 0)))))
    (defn n-outer (n i acc) (if (= i 0) acc (n-outer n (- i 1) (+ acc (n-caller n 1 0)))))
    :ok
"#;

/// ADR-401: a native loop called from native code is preempted by unwinding the native
/// activations into interpreter frames. It used to deopt into a nested interpreter that
/// could not be preempted either, so a warm request handler calling a warm loop ran it to
/// the end in one quantum — the latency row's 1–2 ms fat handlers.
///
/// **One shape per test, each in its own process (nextest).** A shape's run can trigger a
/// hot re-lowering of the callers, and an activation that lands on the swap runs on the
/// interpreter — so a second shape in the same process can run its callers interpreted and
/// pass through the always-preemptible VM→native path instead of the native chain it names
/// (it did, with the unwind sabotaged). First after the warm-up, a shape runs the chain.
fn native_shape_is_preemptible(name: &str, hog: &str) {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    interp.eval_str(NATIVE_CHAIN).expect("prelude errored");
    settle_native(
        &mut interp,
        "(doseq (i (range 200)) (n-outer 40 2 0))",
        &[("n-spin", 3), ("n-caller", 3), ("n-outer", 3)],
    );
    let first = first_message(&mut interp, hog);
    process::set_test_no_workers(false);
    assert_eq!(
        first, ":pong",
        "{name} was not preempted — it starved its peer"
    );
}

#[test]
fn a_native_loop_under_one_native_caller_is_preemptible() {
    native_shape_is_preemptible(
        "a native loop under one native caller",
        "(n-caller 200000 1 0)",
    );
}

#[test]
fn a_native_loop_under_two_native_callers_is_preemptible() {
    native_shape_is_preemptible(
        "a native loop under two native callers",
        "(n-outer 200000 1 0)",
    );
}

#[test]
fn a_native_chain_inside_try_is_preemptible() {
    native_shape_is_preemptible(
        "a native chain inside try",
        "(try (n-outer 200000 1 0) (catch e nil))",
    );
}

/// The chain's value survives the round trip through interpreter frames: every caller the
/// preempt rebuilt resumes with its operand stack and adds the callee's value to it.
#[test]
fn an_unwound_native_chain_returns_the_same_value() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    interp.eval_str(NATIVE_CHAIN).expect("prelude errored");
    settle_native(
        &mut interp,
        "(doseq (i (range 200)) (n-outer 40 2 0))",
        &[("n-spin", 3), ("n-caller", 3), ("n-outer", 3)],
    );
    // A companion hog keeps a process waiting for the worker throughout: with none, a spent
    // budget does not unwind (nothing would run instead), and this test is about the value
    // an unwound and rebuilt chain returns.
    interp
        .eval_str(
            "(do (def me (self))
                 (spawn (send me (+ 7 (n-outer 200000 3 0))))
                 (spawn (n-spin 3000000 0 0))
                 :ok)",
        )
        .expect("spawn errored");
    let ran = std::thread::spawn(|| process::test_drive_quanta(100_000))
        .join()
        .unwrap();
    let got = interp
        .eval_str("(receive (v v) (after 0 :nothing))")
        .expect("poll errored");
    let got = interp.print(got);
    process::set_test_no_workers(false);
    assert!(
        ran > 3,
        "the chain should have taken many quanta, took {ran}"
    );
    // 3 × (sum 0..199999) + 7.
    assert_eq!(
        got, "59999700007",
        "the unwound chain returned a different value"
    );
}

/// ADR-401: a native RECURSION — no back edge anywhere — is preempted at the entry
/// reduction poll and unwound through every level. It used to run to the end in one
/// quantum: a tree walk, a nested encode, any recursive function that tiered.
#[test]
fn a_native_recursion_is_preemptible() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    interp
        .eval_str("(defn n-make (d) (if (= d 0) [nil nil] [(n-make (- d 1)) (n-make (- d 1))]))")
        .expect("prelude errored");
    settle_native(
        &mut interp,
        "(doseq (i (range 50)) (n-make 6))",
        &[("n-make", 1)],
    );
    let first = first_message(&mut interp, "(n-make 17)");
    process::set_test_no_workers(false);
    assert_eq!(
        first, ":pong",
        "a native recursion was not preempted — it starved its peer"
    );
}

/// ADR-401: a pure self-recursive arm journals nothing in its frame — no area, no reset on
/// its hot path — so the operand beneath its second call (the first call's result) survives
/// an unwind only through the side record its error path writes at the top of `roots`.
/// Unwound and rebuilt many times over, the tree count must come out exact.
#[test]
fn an_unwound_native_recursion_returns_the_same_value() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    interp
        .eval_str(
            // A tree walk over vectors (`bintree`'s shape): an int-only fib-shaped arm would
            // take the unboxed register worker instead, which is not preemptible at all.
            "(defn n-make (d) (if (= d 0) [nil nil] [(n-make (- d 1)) (n-make (- d 1))]))
             (defn n-count (node)
               (if (nil? node) 1 (+ 1 (+ (n-count (nth node 0)) (n-count (nth node 1))))))
             (def n-tree (n-make 16))
             (defn n-spin (k j acc) (if (>= j k) acc (n-spin k (+ j 1) (+ acc j))))",
        )
        .expect("prelude errored");
    settle_native(
        &mut interp,
        "(doseq (i (range 50)) (n-count (n-make 6)))",
        &[("n-count", 1)],
    );
    // The companion keeps a process waiting, so spent budgets unwind (see above).
    interp
        .eval_str(
            "(do (def me (self))
                 (spawn (send me (n-count n-tree)))
                 (spawn (n-spin 3000000 0 0))
                 :ok)",
        )
        .expect("spawn errored");
    let ran = std::thread::spawn(|| process::test_drive_quanta(100_000))
        .join()
        .unwrap();
    let got = interp
        .eval_str("(receive (v v) (after 0 :nothing))")
        .expect("poll errored");
    let got = interp.print(got);
    process::set_test_no_workers(false);
    assert!(
        ran > 3,
        "the recursion should have taken many quanta, took {ran}"
    );
    // 2^(16+2) - 1: every node and every nil leaf counted once.
    assert_eq!(
        got, "262143",
        "the unwound recursion returned a different value"
    );
}

/// ADR-402: a `receive` inside a `binding` body parks, and resumes with the binding still in
/// force; it used to block the worker like a `try`-wrapped one.
#[test]
fn a_receive_inside_binding_parks_and_keeps_its_binding() {
    process::set_max_parallel(1);
    process::set_test_no_workers(true);
    let mut interp = Interp::new();
    let setup = r#"
        (defdyn *parked-x* :outside)
        (def me (self))
        (def waiter (spawn (send me (binding (*parked-x* :inside) (receive (:go *parked-x*))))))
        (defn responder (parent) (receive (:ping (send parent :pong))))
        (def r (spawn (responder me)))
        (send r :ping)
        :ok
    "#;
    interp.eval_str(setup).expect("setup errored");
    std::thread::spawn(|| process::test_drive_quanta(64))
        .join()
        .unwrap();
    let v = interp
        .eval_str("(receive (:pong :pong) (after 0 :nothing))")
        .expect("poll errored");
    assert_eq!(
        interp.print(v),
        ":pong",
        "the binding-wrapped receive blocked the only worker"
    );
    interp.eval_str("(send waiter :go)").expect("send errored");
    std::thread::spawn(|| process::test_drive_quanta(64))
        .join()
        .unwrap();
    let v = interp
        .eval_str("(receive (:inside :inside) (:outside :outside) (after 0 :nothing))")
        .expect("poll errored");
    process::set_test_no_workers(false);
    assert_eq!(
        interp.print(v),
        ":inside",
        "the binding did not survive the park"
    );
}
