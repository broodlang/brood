//! Regressions from the 2026-10-02 type-system review: ability/protocol checks, sealed
//! exhaustiveness, the check cache, recursion and guard-effect lints, `try` bodies.

use super::*;

fn count_containing(warnings: &[String], needle: &str) -> usize {
    warnings.iter().filter(|w| w.contains(needle)).count()
}

// ---- one dead clause, one warning, one category ----------------------------------------

#[test]
fn a_dead_cond_clause_is_reported_once_naming_the_tested_expression() {
    let w = file_warnings("(sig f (int -> int)) (defn f (n) (cond (string? n) 0 else n))");
    assert_eq!(
        count_containing(&w, "unreachable clause: n is int"),
        1,
        "the dead clause is reported: {w:?}"
    );
    assert_eq!(
        count_containing(&w, "can never be true"),
        0,
        "the same dead clause must not be reported a second time: {w:?}"
    );
}

#[test]
fn unreachable_clause_silences_a_dead_clause_and_only_that_category_does() {
    let src = |category: &str| {
        format!("(sig f (int -> int)) ({category} (defn f (n) (cond (string? n) 0 else n)))")
    };
    let allowed = file_warnings(&src("check-allow :unreachable-clause"));
    assert!(
        allowed.is_empty(),
        "`:unreachable-clause` is the documented category of a dead clause: {allowed:?}"
    );
    let other = file_warnings(&src("check-allow :type-mismatch"));
    assert_eq!(
        count_containing(&other, "unreachable clause"),
        1,
        "a dead clause has ONE category, and `:type-mismatch` is not it: {other:?}"
    );
}

#[test]
fn a_dead_clause_on_a_precise_let_local_is_silenced_by_its_category() {
    // `docs/type-annotations.md`'s own use case for the directive.
    let w = file_warnings(
        "(check-allow :unreachable-clause (defn a () (let (port 8080) (cond (string? port) 0 else port))))",
    );
    assert!(w.is_empty(), "{w:?}");
}

#[test]
fn a_dead_match_guard_is_reported_once_naming_its_own_binder() {
    let w = file_warnings(
        "(sig g (int -> int)) (defn g (n) (match n (s :when (string? s) 0) (_ (+ n 1))))",
    );
    assert_eq!(
        w.len(),
        1,
        "one dead clause, one warning — not one per binding that aliases it: {w:?}"
    );
    assert!(w[0].contains("unreachable clause: s is int"), "{w:?}");
    let allowed = file_warnings(
        "(sig g (int -> int)) (defn g (n) (check-allow :unreachable-clause (match n (s :when (string? s) 0) (_ (+ n 1)))))",
    );
    assert!(allowed.is_empty(), "{allowed:?}");
}

#[test]
fn a_dead_literal_match_clause_without_a_predicate_is_still_reported() {
    // A literal pattern on a precisely typed binding: no predicate is involved, and the
    // dead clause is reported under its one category all the same.
    let w = file_warnings("(sig h (int -> int)) (defn h (n) (match n (\"a\" 0) (_ n)))");
    assert_eq!(count_containing(&w, "unreachable clause"), 1, "{w:?}");
    let allowed = file_warnings(
        "(sig h (int -> int)) (defn h (n) (check-allow :unreachable-clause (match n (\"a\" 0) (_ n))))",
    );
    assert!(allowed.is_empty(), "{allowed:?}");
}

// ---- sealed-match exhaustiveness forgets a rebound name ---------------------------------

const SEALED_PRELUDE: &str = "(defmodule u) (defrecord c (x)) (defrecord r (y)) \
     (defability S :sealed [c r] (op [self] :-> int)) \
     (impl S u/c (op [s] 1)) (impl S u/r (op [s] 2)) ";

fn sealed_warnings(body: &str) -> Vec<String> {
    let w = file_warnings(&format!("{SEALED_PRELUDE}{body}"));
    w.into_iter()
        .filter(|m| m.contains("no clause handles"))
        .collect()
}

#[test]
fn sealed_match_still_reports_a_missing_member_of_the_parameter() {
    let w = sealed_warnings("(sig f (S -> int)) (defn f (s) (match s ((record c {:x x}) 1)))");
    assert_eq!(w.len(), 1, "the guard must still fire: {w:?}");
}

#[test]
fn sealed_match_forgets_a_name_a_match_clause_rebinds() {
    let w = sealed_warnings(
        "(sig f (S -> int)) (defn f (s) (match s ((record c {:x s}) (match s ((record c {:x _}) 10))) ((record r {:y y}) 2)))",
    );
    assert!(
        w.is_empty(),
        "the inner `s` is the FIELD, not the sealed param: {w:?}"
    );
}

#[test]
fn sealed_match_forgets_a_name_rebound_by_if_let_for_destructuring_fn_and_catch() {
    for body in [
        "(sig f (S -> int)) (defn f (s) (if-let (s (get s :x)) (match s ((record c {:x _}) 10)) 0))",
        "(sig g (S -> int)) (defn g (s) (for (s [(c 1)]) (match s ((record c {:x _}) 10))))",
        "(sig f (S -> int)) (defn f (s) (let ([s t] [(c 1) 2]) (match s ((record c {}) 10))))",
        "(sig g (S -> int)) (defn g (s) ((fn ([s]) (match s ((record c {}) 10))) [(c 1)]))",
        "(sig h (S -> int)) (defn h (s) (try (match (c 1) ((record c {}) 10)) (catch s (match s ((record c {}) 10)))))",
    ] {
        let w = sealed_warnings(body);
        assert!(w.is_empty(), "a rebinding keeps no sealed type — {body}: {w:?}");
    }
}

// ---- impl / behaviour op shapes ----------------------------------------------------------

#[test]
fn an_impl_op_with_a_list_parameter_form_is_read() {
    let w = file_warnings(
        "(defmodule u) (defrecord m (n)) (defability Eq (eqv [self other] :-> bool)) (impl Eq u/m (eqv (a b) true))",
    );
    assert_eq!(
        count_containing(&w, "eqv"),
        0,
        "`(eqv (a b) …)` is an impl of eqv: {w:?}"
    );
}

#[test]
fn a_variadic_behaviour_op_accepts_any_provider_arity() {
    let w = file_warnings(
        "(defmodule bv (:implements V)) (defbehaviour V (render [self & more])) (defn render (a b c) [a b c])",
    );
    assert_eq!(count_containing(&w, "behaviour V"), 0, "{w:?}");
    let fixed = file_warnings(
        "(defmodule bv (:implements V)) (defbehaviour V (render [self])) (defn render (a b c) [a b c])",
    );
    assert_eq!(
        count_containing(&fixed, "behaviour V"),
        1,
        "a fixed-arity op still pins the provider: {fixed:?}"
    );
}

// ---- non-tail recursion lint -------------------------------------------------------------

#[test]
fn a_zero_arity_first_arm_is_read_as_multi_arity() {
    let w = recursion_warnings("(defn loop1 (() (loop1 0)) ((i) (if (< i 3) (loop1 (inc i)) i)))");
    assert!(w.is_empty(), "both self-calls are tail calls: {w:?}");
    let real = recursion_warnings("(defn loop2 (() (loop2 0)) ((i) (+ 1 (loop2 i))))");
    assert_eq!(
        real.len(),
        1,
        "a real non-tail call in the second arm: {real:?}"
    );
}

#[test]
fn a_local_binding_shadows_the_function_for_the_recursion_lint() {
    let w = recursion_warnings("(defn step (x) (let (step (fn (y) (* y 2))) (+ 1 (step x))))");
    assert!(w.is_empty(), "the call is to the LOCAL step: {w:?}");
    let real = recursion_warnings("(defn step (x) (let (other 1) (+ other (step x))))");
    assert_eq!(real.len(), 1, "{real:?}");
}

// ---- guard purity resolves the head ------------------------------------------------------

#[test]
fn a_guard_calling_a_module_function_or_local_named_like_an_effect_is_pure() {
    let w = file_warnings(
        "(defmodule guard1) (defn exit (s) (= s :done)) \
         (defn classify ((s) :when (exit s) :finished) ((s) :running)) \
         (defn pick (x) (let (send (fn (v) (> v 1))) (match x ((v) :when (send v) :big) (_ :small))))",
    );
    assert_eq!(count_containing(&w, "effectful call"), 0, "{w:?}");
    let real = file_warnings("(defn pick (p x) (match x ((v) :when (send p v) :big) (_ :small)))");
    assert_eq!(
        count_containing(&real, "effectful call `send`"),
        1,
        "the primitive is still flagged: {real:?}"
    );
}

// ---- discarded qualified symbol ----------------------------------------------------------

#[test]
fn a_qualified_name_read_for_its_load_before_a_statement_is_not_flagged() {
    let w = file_warnings("(defn f () (do json/encode (io/puts \"x\") 1))");
    assert_eq!(
        count_containing(&w, "evaluated here and discarded"),
        0,
        "{w:?}"
    );
    let call_syntax = file_warnings("(defn f (s) (do string/length(s) 1))");
    assert_eq!(
        count_containing(&call_syntax, "evaluated here and discarded"),
        1,
        "`mod/f(x)` is still the slip it reports: {call_syntax:?}"
    );
}

// ---- `try` bodies are checked; `error-of` / `assert-error` are not -----------------------

#[test]
fn a_type_misuse_inside_a_try_body_is_reported() {
    let w = file_warnings("(defn f () (try (string/length 6) (catch e e)))");
    assert_eq!(count_containing(&w, "string/length"), 1, "{w:?}");
    let handler = file_warnings("(defn f () (try 1 (catch e (string/length 6))))");
    assert_eq!(
        count_containing(&handler, "string/length"),
        1,
        "{handler:?}"
    );
}

#[test]
fn error_testing_forms_still_provoke_their_failure_quietly() {
    let w = file_warnings(
        "(defmodule t (:use test)) (defn f () (assert-error (string/length 6)) (error-of (string/length 6)))",
    );
    assert_eq!(count_containing(&w, "string/length"), 0, "{w:?}");
    let unbound = file_warnings("(defmodule t (:use test)) (defn f () (error-of (no-such-fn 1)))");
    assert_eq!(
        count_containing(&unbound, "no-such-fn"),
        1,
        "an unbound name is still reported inside them (KI-67): {unbound:?}"
    );
}

#[test]
fn a_never_true_predicate_outside_a_dead_clause_keeps_its_type_mismatch_category() {
    // No eligible binding is narrowed, so there is no dead clause: the finding is about the
    // argument's type, and `:type-mismatch` — the category ~15 negative tests use — owns it.
    let w = file_warnings("(defn f () (list (keyword? (symbol \"x\"))))");
    assert_eq!(count_containing(&w, "can never be true"), 1, "{w:?}");
    let allowed =
        file_warnings("(defn f () (check-allow :type-mismatch (list (keyword? (symbol \"x\")))))");
    assert!(allowed.is_empty(), "{allowed:?}");
}

#[test]
fn a_non_exhaustive_literal_match_has_a_category() {
    let w = file_warnings("(defn f () (match 7 (1 :a) (2 :b)))");
    assert_eq!(count_containing(&w, "not exhaustive"), 1, "{w:?}");
    let allowed = file_warnings("(defn f () (check-allow :type-mismatch (match 7 (1 :a) (2 :b))))");
    assert_eq!(
        count_containing(&allowed, "not exhaustive"),
        0,
        "{allowed:?}"
    );
}

// ---- another file's imported abilities are not this file's business (B8) -------------

#[test]
fn a_file_is_not_told_about_abilities_it_cannot_see() {
    // `brood --check user.blsp other.blsp` in one process: `user` loads `sm`, whose sealed
    // `S` lacks an impl for `r` and whose `Ord` requires an `Eq` nobody implements. Those
    // registrations are process state, and `other.blsp` — with no path to `sm` — used to
    // be told about them, because the ability facts read the WHOLE registry.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "(defmodule rv-sm)\n\
             (defrecord c (x))\n\
             (defrecord r (y))\n\
             (defability S :sealed [c r] (op [self] :-> int))\n\
             (impl S rv-sm/c (op [s] 1))\n\
             (defability Eq (eqv [self other] :-> bool))\n\
             (defability Ord :requires [Eq] (compare-to [self other] :-> int))\n\
             (impl Ord rv-sm/c (compare-to [a b] 0))",
        )
        .expect("load the ability module");
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms =
        crate::syntax::reader::read_all(&mut heap, "(defmodule rv-other)\n(defn g (x) (+ x 1))")
            .expect("parse");
    let w: Vec<String> = check_file_mode(&mut heap, &forms, &[], false)
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    assert_eq!(count_containing(&w, "ability"), 0, "{w:?}");
}
