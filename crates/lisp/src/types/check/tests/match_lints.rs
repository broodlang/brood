//! `match` lints: exhaustiveness over keyword/int/bool/string/mixed enums, guard purity, redundant clauses.

use super::*;

#[test]
fn match_exhaustiveness_flags_a_missing_keyword_arm() {
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")))
(sig f ((or :ok :error :pending) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains(":pending")),
        "expected a missing-:pending warning, got {w:?}"
    );
}

#[test]
fn guard_purity_flags_an_effect_in_a_match_when_guard() {
    let src = "
(defn f (n counter)
  (match n
    (x :when (do (%table-put counter :seen 1) (> x 0)) :pos)
    (_ :neg)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("%table-put") && s.contains(":when` guard")),
        "expected an effectful-guard warning naming %table-put, got {w:?}"
    );
}

#[test]
fn guard_purity_flags_an_effect_in_a_receive_when_guard() {
    let src = "
(defn worker (counter)
  (receive
    (n :when (< (%table-incr counter :seen) 100) n)
    (_ :skip)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("%table-incr") && s.contains("guard")),
        "expected an effectful-guard warning naming %table-incr, got {w:?}"
    );
}

#[test]
fn guard_purity_is_silent_for_a_pure_guard() {
    let src = "
(defn f (n)
  (match n
    (x :when (> x 0) :pos)
    (x :when (int? x) :int)
    (_ :other)))
(defn g (a b) :when (>= a b) a)
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains(":when` guard")),
        "a pure guard must be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn guard_purity_does_not_flag_an_effect_in_the_clause_body() {
    // The effect is in the body, where it belongs — only the guard is linted.
    let src = "
(defn f (n)
  (match n
    (x :when (> x 0) (io/puts x))
    (_ :neg)))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains(":when` guard")),
        "an effect in the clause body must not be flagged, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_when_every_arm_is_covered() {
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")
    (:pending \"waiting\")))
(sig f ((or :ok :error :pending) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a fully-covered match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_with_a_catch_all_clause() {
    // A catch-all makes the throw disappear from the compiled tree
    // entirely — trivially exhaustive regardless of how few literal arms
    // are listed.
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (_ \"anything else\")))
(sig f ((or :ok :error :pending) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a catch-all match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_int_arm() {
    let src = "
(defn f (code)
  (match code
    (200 \"ok\")
    (404 \"missing\")))
(sig f ((or 200 404 500) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("500")),
        "expected a missing-500 warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_arm_in_a_mixed_kind_enum() {
    // (or :ok 5) — a keyword literal and an int literal on the same
    // declared type (ADR-121 generalizes the old pure-one-kind check).
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")))
(sig f ((or :ok 5) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains('5')),
        "expected a missing-5 warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_arm_with_a_trailing_nil() {
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    (:error \"bad\")))
(sig f ((or :ok :error nil) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("nil")),
        "expected a missing-nil warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_bool_arm() {
    // Note: bare `bool` in a sig is the *unrefined* flat tag (no
    // `lit_bool` set) — `(or true false)` is what actually declares the
    // enumerable 2-value literal type this check needs.
    let src = "
(defn f (x) (match x (true \"yes\")))
(sig f ((or true false) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("false")),
        "expected a missing-false warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_string_arm() {
    let src = "
(defn f (m)
  (match m
    (\"GET\" 1)))
(sig f ((or \"GET\" \"POST\") -> int))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("POST")),
        "expected a missing-POST warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_is_silent_when_a_mixed_kind_enum_is_fully_covered() {
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    (5 \"five\")
    (nil \"nothing\")))
(sig f ((or :ok 5 nil) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a fully-covered mixed-kind match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_declines_a_destructuring_clause_mixed_in() {
    // A non-literal pattern among those tried (here, a vector destructure)
    // means the check can't reason about coverage — bail rather than
    // guess.
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    ([a b] \"pair\")))
(sig f ((or :ok :error) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a match mixing a literal with a destructuring pattern should stay silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_for_a_non_literal_scrutinee_type() {
    // status's declared type is bare `keyword` — not a bounded literal
    // enum — so there's nothing to enumerate against.
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")))
(sig f (keyword -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a non-literal-enum scrutinee should stay silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_redundancy_flags_an_adjacent_duplicate_clause() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:ok 2)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains(":ok")),
        "expected an unreachable-clause warning, got {w:?}"
    );
}

#[test]
fn match_redundancy_flags_a_non_adjacent_duplicate_clause() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:error 2)
    (:ok 3)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains(":ok")),
        "expected an unreachable-clause warning for the non-adjacent duplicate, got {w:?}"
    );
}

#[test]
fn match_redundancy_is_silent_with_no_duplicates() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:error 2)
    (_ 3)))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("unreachable clause")),
        "no duplicate clauses should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_redundancy_fires_on_a_hand_written_eq_chain_too() {
    // Purely structural — not `match`-specific. A hand-written same-symbol
    // `%eq`-if chain with a duplicate literal is unreachable the same way.
    let src = "
(defn f (x)
  (if (%eq x 5)
    :a
    (if (%eq x 5)
      :b
      :c)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains('5')),
        "expected an unreachable-clause warning for the hand-written chain, got {w:?}"
    );
}
