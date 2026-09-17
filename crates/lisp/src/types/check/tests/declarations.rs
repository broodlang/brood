//! The declaration itself must be readable (Pass 2.85): misspelled type names and constructors, arity contradictions, sigs for names never defined.

use super::*;

// ---- the declaration itself must be readable (Pass 2.85) ----
// A `(sig …)` is read ahead of every other signature source, so a declaration the
// parser silently drops is worse than none: the position widens to `any` and the
// author is told nothing. All four shapes below used to exit 0 with no diagnostic.

#[test]
fn a_misspelled_type_name_in_a_sig_is_reported() {
    let ws = file_warnings("(sig f (strng -> int))\n(defn f (s) 0)");
    assert!(
        ws.iter().any(|w| w.contains("sig f: unknown type `strng`")),
        "{ws:?}"
    );
}

#[test]
fn a_misspelled_type_constructor_in_a_sig_is_reported() {
    let ws = file_warnings("(sig f ((tupel int) -> int))\n(defn f (t) 0)");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig f: unknown type constructor `tupel`")),
        "{ws:?}"
    );
    // …and the innermost offender wins, not the enclosing constructor.
    let ws = file_warnings("(sig f ((vector strng) -> int))\n(defn f (v) 0)");
    assert!(
        ws.iter().any(|w| w.contains("unknown type `strng`")),
        "{ws:?}"
    );
}

#[test]
fn a_sig_whose_arity_contradicts_the_definition_is_reported() {
    let ws = file_warnings("(sig f (int -> int))\n(defn f (a b) a)");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig f: declares 1 argument(s) but the definition takes 2")),
        "{ws:?}"
    );
}

#[test]
fn a_sig_arity_that_merely_narrows_the_definition_is_silent() {
    // A multi-arm `defn` annotated with one arm's arrow overlaps the definition's
    // hull — not provably wrong, so it must stay silent (the no-false-positive rule).
    let ws = file_warnings("(sig f (int -> int))\n(defn f ((a) a) ((a b) a))");
    assert!(!ws.iter().any(|w| w.contains("sig f:")), "{ws:?}");
    // A `&optional` definition against a fixed-arity sig, likewise.
    let ws = file_warnings("(sig g (int -> int))\n(defn g (a &optional b) a)");
    assert!(!ws.iter().any(|w| w.contains("sig g:")), "{ws:?}");
}

#[test]
fn a_sig_for_a_name_that_is_never_defined_is_reported() {
    let ws = file_warnings("(sig ghost (int -> int))");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig ghost: nothing named `ghost` is defined here")),
        "{ws:?}"
    );
    // Order doesn't matter — the def may follow the sig.
    let ws = file_warnings("(sig f (int -> int))\n(defn f (a) a)");
    assert!(!ws.iter().any(|w| w.contains("nothing named")), "{ws:?}");
}

#[test]
fn a_capitalised_unknown_type_name_stays_silent_inside_an_arrow_too() {
    // The first cut reported `(Shape -> int)` as a *malformed arrow*: every part read
    // as fine (the capitalised name being deliberately silent), so the walk fell
    // through to the structural message and named the wrong thing. A part that does
    // not parse now decides the whole expression — including when its verdict is
    // silence.
    let ws = file_warnings("(sig w (Shape -> int))\n(defn w (x) 1)\n(defn p () (w 42))");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
    let ws = file_warnings("(sig w ((vector Shape) -> int))\n(defn w (x) 1)");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
    let ws = file_warnings("(sig w ((record :s Shape) -> int))\n(defn w (x) 1)");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
}

#[test]
fn a_capitalised_unknown_type_name_stays_silent() {
    // An ability used as a type resolves by bare name (ADR-181/186), and a
    // single-file check only knows the abilities the file itself declares — so an
    // unknown *capitalised* name is assumed to be one, and never reported.
    let ws = file_warnings("(sig f (Shape -> int))\n(defn f (s) 0)");
    assert!(!ws.iter().any(|w| w.contains("unknown type")), "{ws:?}");
}

#[test]
fn every_type_constructor_the_grammar_parses_is_known_to_the_validator() {
    // `type_expr_problem` reports an unrecognised head, so a constructor added to
    // `parse_type` and not to `TYPE_HEADS` would be reported as unknown — a lint that
    // fires on correct code. Pin the two lists together: each head must parse.
    let mut interp = crate::Interp::new();
    for head in super::annot::TYPE_HEADS {
        let src = match head {
            "map" => "(map keyword int)".to_string(),
            "record" => "(record :a int)".to_string(),
            "tuple" => "(tuple int string)".to_string(),
            "rec" => "(rec X (or nil (vector X)))".to_string(),
            "int" => "(int 0 _)".to_string(),
            "len" => "(len (vector int) 1 _)".to_string(),
            "not" => "(not nil)".to_string(),
            _ => format!("({head} int)"),
        };
        let form = reader::read_one(&mut interp.heap, &src).expect("parse");
        assert!(
            super::annot::type_expr_problem(&interp.heap, form).is_none(),
            "`{head}` is in TYPE_HEADS but `{src}` does not read as a type"
        );
    }
}

// A declaration that names exactly the REQUIRED positions of a `defn` with `&optional`
// parameters seeds those positions. It used to be refused whole (the closure's parameter
// count fell outside the declared arity range), so every parameter of such a function was
// unknown — bedit's `ed-visible-lines`, ten declared over ten required and two optionals,
// read `(+ y k)` as `number` with `y` declared `int` two lines up. The undeclared optionals
// stay unknown; a declaration that misaligns the required positions is still refused.
#[test]
fn a_sig_over_the_required_positions_seeds_a_defn_with_undeclared_optionals() {
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig takes-int (int -> int))\n\
         (defn takes-int (n) (inc n))\n\
         (sig opt2 (int int -> int))\n\
         (defn opt2 (top y &optional (memo? false))\n\
           (takes-int (+ y top)))",
        true,
    );
    assert!(ws.is_empty(), "`y` and `top` are declared ints — {ws:?}");
    // …and a genuine mismatch on a declared position is reported through the seed.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig opt3 (string int -> int))\n\
         (defn opt3 (s y &optional (memo? false))\n\
           (+ s y))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("expects number, got string") || w.contains("got s")),
        "{ws:?}"
    );
}

// A `deftype` past the lattice's node budget is widened to its bare tags by the
// constructors, silently — which is how every `model`-typed read in bedit went `number` the
// day its `model` record crossed the line by one optional field. The declaration now says
// so. A record wide enough for the budget stays whole; one three times over it warns.
#[test]
fn a_deftype_past_the_shape_budget_is_reported_at_the_declaration() {
    let wide = |n: usize| {
        let fields: String = (0..n).map(|i| format!(" :f{i} int")).collect();
        format!("(defmodule t)\n(deftype big (record{fields}))\n(sig g (big -> int))\n(defn g (b) (:f0 b))")
    };
    let ws = file_warnings(&wide(crate::types::MAX_TY_NODES / 2));
    assert!(ws.is_empty(), "{ws:?}");
    let ws = file_warnings(&wide(crate::types::MAX_TY_NODES * 3));
    assert!(
        ws.iter()
            .any(|w| w.contains("deftype big") && w.contains("exceeds the checker's budget")),
        "{ws:?}"
    );
}

// A5 (2026-09-17): the declaration is authoritative (ADR-259), and outside contracts mode
// nothing else stands between a wrong one and its callers — so where the body's result is
// the unknown and the declared return could not be checked against anything, strict says
// the declaration is TRUSTED there. Plain mode stays silent: a declaration is what the
// author meant, and this is not a mismatch.
#[test]
fn a_declared_return_the_body_cannot_verify_is_reported_as_trusted_under_strict() {
    let src = "(defmodule t)\n\
               (sig f (map -> int))\n\
               (defn f (m) (get m :k))\n\
               (sig g (int -> int))\n\
               (defn g (n) (+ n 1))";
    let strict = file_warnings_mode(src, true);
    assert_eq!(
        strict
            .iter()
            .filter(|w| w.contains("trusted, not verified"))
            .count(),
        1,
        "{strict:?}"
    );
    assert!(
        strict
            .iter()
            .any(|w| w.contains("f: declared return type int is trusted, not verified")),
        "{strict:?}"
    );
    let plain = file_warnings_mode(src, false);
    assert!(plain.is_empty(), "{plain:?}");
    // …and a `(check-allow :type-mismatch …)` around it is the author saying so.
    let allowed = file_warnings_mode(
        "(defmodule t)\n\
         (sig f (map -> int))\n\
         (check-allow :type-mismatch (defn f (m) (get m :k)))",
        true,
    );
    assert!(allowed.is_empty(), "{allowed:?}");
}
