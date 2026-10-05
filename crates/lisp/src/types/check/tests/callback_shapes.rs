//! The callback shapes and narrowings the 2026-10-02 review left unchecked, closed on
//! 2026-10-03: a keyword, a named function, a `(comp …)` and an `(apply f … xs)` are held to
//! what they are handed, and a destructured `[tag v]` — or a `match` arm's binder — reads its
//! own alternative of a tagged-tuple union. Each case pairs the finding with the correct
//! program beside it, which must stay silent.

use super::*;

/// The warnings of `src` under `--strict`, which reads merely-wider results too.
fn strict_warnings(src: &str) -> Vec<String> {
    file_warnings_mode(src, true)
}

fn has(ws: &[String], needle: &str) -> bool {
    ws.iter().any(|w| w.contains(needle))
}

#[test]
fn a_keyword_callback_types_as_its_accessor() {
    let ws = file_warnings(
        "(sig bad ((list (record :x int)) -> any))\n\
         (defn bad (ps) (string/length (first (map ps :x))))\n\
         (sig good ((list (record :x string)) -> any))\n\
         (defn good (ps) (string/length (or (first (map ps :x)) \"\")))",
    );
    assert!(
        has(
            &ws,
            "string/length: argument 1 expects string, got nil | int"
        ),
        "`(map ps :x)` over int fields is a list of ints: {ws:?}"
    );
    assert_eq!(ws.len(), 1, "the string-field version is clean: {ws:?}");
}

#[test]
fn a_named_callback_is_held_to_the_element_it_is_handed() {
    let ws = file_warnings(
        "(sig bad ((vector string) -> any))\n\
         (defn bad (xs) (map xs inc))\n\
         (sig good ((vector string) -> any))\n\
         (defn good (xs) (map xs string/length))",
    );
    assert!(
        has(
            &ws,
            "map: argument 2 is a callback handed string at position 1, but inc takes number"
        ),
        "{ws:?}"
    );
    assert_eq!(ws.len(), 1, "`string/length` over strings is clean: {ws:?}");
}

#[test]
fn a_composed_callback_is_checked_at_both_ends_and_between_its_stages() {
    let ws = file_warnings(
        "(sig bad ((vector string) -> any))\n\
         (defn bad (xs) (map xs (comp string/length inc)))\n\
         (sig good ((vector string) -> any))\n\
         (defn good (xs) (map xs (comp inc string/length)))",
    );
    assert!(
        has(
            &ws,
            "comp: inc returns number, but string/length takes string"
        ),
        "the stage chain is broken: {ws:?}"
    );
    assert!(
        has(&ws, "is a callback handed string at position 1"),
        "the composition's first stage is handed the element: {ws:?}"
    );
    assert_eq!(
        ws.len(),
        2,
        "`(comp inc string/length)` over strings is clean: {ws:?}"
    );
}

#[test]
fn apply_holds_the_applied_function_to_what_it_spreads() {
    let ws = file_warnings(
        "(sig bad ((list int) -> any))\n\
         (defn bad (xs) (apply string/length xs))\n\
         (sig good ((list string) -> any))\n\
         (defn good (xs) (apply string/length xs))",
    );
    assert!(
        has(
            &ws,
            "apply: string/length takes string at position 1, but is handed nil | int"
        ),
        "{ws:?}"
    );
    assert_eq!(ws.len(), 1, "spreading strings into it is clean: {ws:?}");
}

#[test]
fn a_record_collection_with_its_own_iteration_is_not_judged_by_its_fields() {
    // A `Seqable` record iterates what its impl says (`record_test`'s `stack`); reading it
    // as a map's entries would report `inc` handed a tuple — the false positive the
    // `__id__` guard exists for.
    let ws = file_warnings(
        "(defrecord stack (items))\n\
         (impl Seqable stack (->seq [s] (get s :items)))\n\
         (defn use-it () (map (stack (list 10 20)) inc))",
    );
    assert!(!has(&ws, "is a callback handed"), "{ws:?}");
}

#[test]
fn a_destructured_tag_narrows_its_sibling() {
    let ws = strict_warnings(
        "(sig f ((or (tuple :ok int) (tuple :err string)) -> int))\n\
         (defn f (r) (let ([tag v] r) (if (= tag :ok) (inc v) (string/length v))))",
    );
    assert!(
        ws.is_empty(),
        "`v` is int under :ok and string under :err: {ws:?}"
    );
    let ws = strict_warnings(
        "(sig f ((or (tuple :ok int) (tuple :err string)) -> int))\n\
         (defn f (r) (let ([tag v] r) (if (= tag :ok) (string/length v) 0)))",
    );
    assert!(
        has(&ws, "string/length: argument 1 expects string, got int"),
        "and a wrong use under the narrowing is still found: {ws:?}"
    );
}

#[test]
fn a_match_arm_returns_its_own_alternative_to_the_return_check() {
    let ws = strict_warnings(
        "(sig f ((or (tuple :ok int) (tuple :err string)) -> int))\n\
         (defn f (r) (match r ([:ok v] (inc v)) ([:err m] (string/length m))))",
    );
    assert!(ws.is_empty(), "each arm is int: {ws:?}");
}

#[test]
fn a_finding_inside_a_duplicated_clause_is_reported_once() {
    let ws = file_warnings(
        "(sig f ((or (tuple :ok int) (tuple :err string)) -> any))\n\
         (defn f (r) (match r ([:ok v] v) ([:err m] (inc m))))",
    );
    let inc: Vec<&String> = ws
        .iter()
        .filter(|w| w.contains("inc: argument 1"))
        .collect();
    assert_eq!(
        inc.len(),
        1,
        "`match` copies the :err clause into two branches: {ws:?}"
    );
}
