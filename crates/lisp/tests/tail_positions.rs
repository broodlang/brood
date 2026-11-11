//! **Proper tail calls are load-bearing** (CLAUDE.md, "Conventions & invariants"): a form
//! with a body or branches hands its LAST form back to the evaluator loop instead of
//! evaluating it in a nested call, so a loop written as tail recursion runs in O(1) frames.
//!
//! `basic.rs::tail_calls_do_not_overflow` covered one form (`if`). This covers every name in
//! `builtins::SPECIAL_FORMS`: each is either listed in [`TAIL_SHAPES`] with a loop whose
//! recursive call sits in that form's tail position, or in [`NO_TAIL_POSITION`] with the
//! reason it has none. A special form added to the list without being classified here fails
//! [`every_special_form_is_classified`], which is the point: the rule is checked when the
//! form lands, not when someone notices a stack overflow.
//!
//! The loops run deeper than either engine's non-tail frame limit (the tree-walker's
//! `MAX_TW_FRAMES` is 2^19, the VM's 2^20), so a form that stops being tail-safe fails with
//! `recursion too deep` rather than passing on a shallow loop. They run on the tree-walker,
//! where the continuation discipline lives, and on the default tier.

mod support;

use brood::builtins::SPECIAL_FORMS;
use brood::eval::compile::Tier;
use support::eval_on;

/// Deeper than both engines' non-tail frame limits (2^19 tree-walked, 2^20 in the VM).
const DEPTH: u32 = 1_100_000;

/// Each form with a tail position, and a loop body whose recursive `(spin …)` call sits in
/// it. `spin` is `(defn spin (n) BODY)`; it must answer `:done` once `n` reaches 0.
const TAIL_SHAPES: &[(&str, &str)] = &[
    ("if", "(if (= n 0) :done (spin (dec n)))"),
    ("do", "(if (= n 0) :done (do 1 (spin (dec n))))"),
    ("let", "(if (= n 0) :done (let (m (dec n)) (spin m)))"),
    (
        "letrec",
        "(if (= n 0) :done (letrec (k (fn () 1)) (spin (dec n))))",
    ),
    ("when", "(if (= n 0) :done (when true (spin (dec n))))"),
    ("cond", "(cond (= n 0) :done else (spin (dec n)))"),
    ("and", "(if (= n 0) :done (and true (spin (dec n))))"),
    ("or", "(if (= n 0) :done (or false (spin (dec n))))"),
    ("match", "(match n (0 :done) (m (spin (dec m))))"),
    ("case", "(case n 0 :done (spin (dec n)))"),
    ("->", "(if (= n 0) :done (-> n dec spin))"),
];

/// Special forms with no tail position to test, each with the reason.
const NO_TAIL_POSITION: &[(&str, &str)] = &[
    ("quote", "returns its argument unevaluated"),
    (
        "quasiquote",
        "builds data; its holes are not in tail position",
    ),
    (
        "fn",
        "a function's body is the tail position every shape above already runs through",
    ),
    (
        "def",
        "binds a global; the value is not returned in tail position",
    ),
    ("defmacro", "a definition"),
    ("defn", "a definition"),
    ("defdyn", "a definition"),
    ("defrecord", "a definition"),
    ("defability", "a definition"),
    ("impl", "a definition"),
    ("defmodule", "a module header"),
    ("comment", "ignores its body"),
    (
        "try",
        "its body runs inside a handler frame, by design not a tail position",
    ),
    ("catch", "a clause of try"),
    ("throw", "does not return"),
    (
        "receive",
        "waits for a message; tested by the process suites",
    ),
    (
        "binding",
        "restores the dynamic binding after the body, so the body is not in tail position",
    ),
    ("doseq", "a loop that returns nil"),
    ("for", "a comprehension that builds a collection"),
    ("error", "raises"),
    (
        "with-out-str",
        "captures output around its body, then returns the captured string",
    ),
    (
        "with-err-str",
        "captures output around its body, then returns the captured string",
    ),
];

fn classified(name: &str) -> bool {
    TAIL_SHAPES.iter().any(|(form, _)| *form == name)
        || NO_TAIL_POSITION.iter().any(|(form, _)| *form == name)
}

#[test]
fn every_special_form_is_classified() {
    // SPECIAL_FORMS also names macro keywords the highlighter colours (`spawn`, …); those
    // past the evaluator's own forms are listed in NO_TAIL_POSITION only when they are
    // forms a body runs through. Everything not classified is reported at once.
    let unclassified: Vec<&str> = SPECIAL_FORMS
        .iter()
        .copied()
        .filter(|name| !classified(name) && !HIGHLIGHT_ONLY.contains(name))
        .collect();
    assert!(
        unclassified.is_empty(),
        "special forms with no tail-position verdict: {unclassified:?}\n\
         add each to TAIL_SHAPES (with a loop through its tail position) or to \
         NO_TAIL_POSITION (with the reason it has none) in {}",
        file!()
    );
    for (form, _) in TAIL_SHAPES.iter().chain(NO_TAIL_POSITION) {
        assert!(
            SPECIAL_FORMS.contains(form),
            "{form} is classified here but is no longer in SPECIAL_FORMS — drop it"
        );
    }
}

/// Names in `SPECIAL_FORMS` that are there for highlighting only and are ordinary macros or
/// functions over a body tested elsewhere (spawning runs the body in another process).
const HIGHLIGHT_ONLY: &[&str] = &["spawn", "spawn-link"];

fn run_loop(body: &str, tier: Tier) -> Result<String, String> {
    eval_on(&format!("(defn spin (n) {body}) (spin {DEPTH})"), tier)
}

#[test]
fn every_tail_position_runs_in_constant_frames_on_the_tree_walker() {
    for (form, body) in TAIL_SHAPES {
        assert_eq!(
            run_loop(body, Tier::TreeWalk),
            Ok(":done".to_string()),
            "`{form}` is not tail-safe on the tree-walker: {body}"
        );
    }
}

#[test]
fn every_tail_position_runs_in_constant_frames_on_the_default_tier() {
    for (form, body) in TAIL_SHAPES {
        assert_eq!(
            run_loop(body, Tier::Native),
            Ok(":done".to_string()),
            "`{form}` is not tail-safe on the default tier: {body}"
        );
    }
}

/// The depth is load-bearing: a loop with its call OUT of tail position must fail at it, on
/// both tiers, or the shapes above would pass whether or not their forms are tail-safe.
#[test]
fn a_call_out_of_tail_position_fails_at_this_depth() {
    let not_tail = "(if (= n 0) 0 (+ 1 (spin (dec n))))";
    for tier in [Tier::TreeWalk, Tier::Native] {
        let result = run_loop(not_tail, tier);
        assert!(
            matches!(&result, Err(message) if message.contains("recursion too deep")),
            "a non-tail loop {DEPTH} deep did not hit the frame limit on {tier:?}: {result:?}"
        );
    }
}
