//! The checker's budgets DECLINE, never lie (B6): a fixpoint that does not
//! settle, a specialization budget that runs out, an expression past the depth cap — each
//! reads the affected answers as unknown (sound) and says so with one `checker gave up:`
//! note. These pin that the note appears when a cap is hit, that it is the ONLY diagnostic
//! (the decline never manufactures a warning), and that a return which merely grows
//! structurally is widened to a fixpoint rather than reported.

use super::*;

fn gave_up(warnings: &[String]) -> Vec<&String> {
    warnings
        .iter()
        .filter(|w| w.starts_with("checker gave up:"))
        .collect()
}

#[test]
fn a_same_file_macro_reports_the_opaque_derivation_once() {
    // The commonest cap: a macro defined and used in the same file is unexpanded when
    // the caller-derived parameters are collected, so every derivation in the file
    // declines, and must say so — silence reads as a fully checked file.
    let warnings = file_warnings(
        "(defmodule caps-opaque)
         (defmacro my-or (a b) `(let ((v ~a)) (if v v ~b)))
         (defn- pick (x) (my-or x 1))
         (defn use-pick () (pick 2))",
    );
    let notes = gave_up(&warnings);
    assert_eq!(
        notes.len(),
        1,
        "one note for the whole file, got {warnings:?}"
    );
    assert!(
        notes[0].contains("an unexpanded macro call keeps every caller-derived parameter unknown")
    );
    assert_eq!(
        warnings.len(),
        1,
        "the decline is the only diagnostic: {warnings:?}"
    );
}

#[test]
fn a_parameter_named_like_a_macro_is_not_an_unexpanded_macro_call() {
    // The parameter list is not code: `(fn (match) …)` walked as a form was a call of the
    // `match` macro, and one such lambda anywhere made every derivation in the file decline
    // (an unrelated helper's `(inc index)` then read `number`, and `--strict` blamed it).
    // The multi-clause form's parameter lists are skipped the same way.
    let warnings = file_warnings(
        "(defmodule caps-param-named-macro)
         (defn- step (i) (inc i))
         (defn use-step () (step 1))
         (defn found (xs column)
           (seq/find xs (fn (match) (< (:start match) column))))
         (defn arms (x) ((fn ((cond) cond) ((cond when) when)) x))",
    );
    assert!(gave_up(&warnings).is_empty(), "no decline: {warnings:?}");
}

#[test]
fn a_macro_call_in_a_parameter_default_still_declines() {
    // A default is code: an unexpanded macro there is walked, and still declines.
    let warnings = file_warnings(
        "(defmodule caps-default-macro)
         (defmacro my-or (a b) `(let ((v ~a)) (if v v ~b)))
         (defn- pick (x) x)
         (defn use-pick (&optional (y (my-or nil 1))) (pick y))",
    );
    assert_eq!(gave_up(&warnings).len(), 1, "one decline: {warnings:?}");
}

#[test]
fn a_form_past_the_depth_cap_is_reported_not_silently_unknown() {
    let mut interp = crate::Interp::new();
    let x = Value::Sym(crate::core::value::intern("x"));
    let deep = nest_form(
        &mut interp,
        "inc",
        (infer::MAX_EXPR_TY_DEPTH + 64) as usize,
        x,
    );
    let defn = reader::read_one(&mut interp.heap, "(defn f (x) nil)").unwrap();
    let items = items_of(&interp, defn);
    let deep_fn = mk_list(&mut interp, &[items[0], items[1], items[2], deep]);
    let warnings: Vec<String> = check_file(&mut interp.heap, &[deep_fn])
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    let notes = gave_up(&warnings);
    assert_eq!(notes.len(), 1, "{warnings:?}");
    assert!(
        notes[0].contains("nested past the depth cap"),
        "{}",
        notes[0]
    );
}

#[test]
fn a_structurally_growing_return_widens_to_a_fixpoint_without_a_note() {
    // Kleene-from-⊥ seeding: `bn`'s self-call reads `never` in round one, then `nil`,
    // then `nil | (list any)`, then a deeper list … — unbounded unless each round is
    // widened against the last. The joint loop always widened; same-file inference (the return
    // fixpoint) did not, and 34 std/tests functions "were still moving" after 16
    // rounds the day the note was added. Sabotage: drop the widening in same-file inference and
    // this reports a note.
    let warnings = file_warnings(
        "(defmodule caps-widen)
         (defn- nest (n) (if (= n 0) :leaf (list (nest (- n 1)))))
         (defn use-nest () (nest 3))",
    );
    assert!(gave_up(&warnings).is_empty(), "{warnings:?}");
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_clean_file_reports_no_cap() {
    let warnings = file_warnings(
        "(defmodule caps-clean)
         (defn- twice (x) (* 2 x))
         (defn four () (twice 2))",
    );
    assert!(gave_up(&warnings).is_empty(), "{warnings:?}");
}

#[test]
fn a_union_widened_blindly_is_reported() {
    // Unions keep four alternatives; a fifth merges two that a check could tell apart, and a
    // finding against one of them can vanish. Four tagged arms report the `nil` reaching
    // `string/upper`; five used to go silent with no note at all.
    let four = file_warnings(
        "(defn r (k) (case k :a [:a 1] :b [:b \"x\"] :c [:c true] [:d nil]))\n\
         (defn u (k) (match (r k) ([:d v] (string/upper v)) (_ \"\")))",
    );
    assert!(four.iter().any(|w| w.contains("got nil")), "{four:?}");
    let five = file_warnings(
        "(defn r (k) (case k :a [:a 1] :b [:b \"x\"] :c [:c true] :d [:d nil] [:e \"s\"]))\n\
         (defn u (k) (match (r k) ([:d v] (string/upper v)) (_ \"\")))",
    );
    assert!(
        five.iter()
            .any(|w| w.contains("alternatives and") && w.contains("widened")),
        "{five:?}"
    );
}
