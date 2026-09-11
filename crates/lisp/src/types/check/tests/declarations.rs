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
            "or" | "and" => format!("({head} int string)"),
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
