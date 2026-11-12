//! Typed parameters (ADR-420): `(defn f (x:int y -> int) …)` declares its signature where the
//! parameters are. What the checker must hold: a typed parameter is checked at every call, an
//! untyped one is unconstrained, a missing `-> R` leaves the result to INFERENCE (annotating
//! one parameter must never cost the return the checker inferred before), and a stated result
//! is still checked against the body.

use super::*;

#[test]
fn a_typed_parameter_is_checked_at_the_call() {
    let ws = file_warnings(
        "(defn area (w:number h:number -> number) (* w h))\n(defn use () (area \"a\" 2))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("area: argument 1 expects number")),
        "{ws:?}"
    );
}

#[test]
fn an_untyped_parameter_beside_a_typed_one_is_unconstrained() {
    let ws = file_warnings("(defn pair (x:int y) [x y])\n(defn use () (pair 1 \"anything\"))");
    assert!(
        !ws.iter().any(|w| w.contains("argument 2")),
        "an untyped parameter must accept anything: {ws:?}"
    );
}

/// The point of `_`: without `-> R` the result is still inferred, so a caller that misuses it
/// is still caught — and the body is not checked against a return nobody declared.
#[test]
fn a_typed_list_without_a_result_keeps_the_inferred_return() {
    let ws = file_warnings(
        "(defn half (x:int y:int) (+ x y))\n\
         (defn shout (s:string) (string/upper s))\n\
         (defn use () (shout (half 1 2)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("shout: argument 1 expects string") && w.contains("half")),
        "the inferred int return of `half` must reach the call: {ws:?}"
    );
    assert!(
        !ws.iter().any(|w| w.contains("declared return type")),
        "an open result is not a declaration to check the body against: {ws:?}"
    );
}

#[test]
fn a_stated_result_is_checked_against_the_body() {
    let ws = file_warnings("(defn wrong (x:int -> string) x)");
    assert!(
        ws.iter().any(|w| w.contains("declared return type string")),
        "{ws:?}"
    );
}

#[test]
fn a_private_definition_and_a_form_typed_parameter_are_read_too() {
    let ws = file_warnings(
        "(defn- total (xs: (list int) -> int) (reduce xs 0 +))\n(defn use () (total \"no\"))",
    );
    assert!(
        ws.iter().any(|w| w.contains("total: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn the_checker_binds_the_name_not_the_typed_symbol() {
    // A raw-form pass that bound `x:int` would read the body's `x` as unbound.
    let ws = file_warnings("(defn inc1 (x:int -> int) (+ x 1))");
    assert!(
        !ws.iter().any(|w| w.contains("unbound")),
        "the body's `x` is the parameter: {ws:?}"
    );
}

/// ADR-259 holds for typed parameters: an unreadable type is reported, not silently widened.
#[test]
fn a_misspelled_parameter_type_is_reported() {
    let ws = file_warnings("(defn f (s:strng -> int) 0)");
    assert!(
        ws.iter()
            .any(|w| w.contains("defn f: parameter `s`") && w.contains("unknown type `strng`")),
        "{ws:?}"
    );
    let ws = file_warnings("(defn g (s:string -> intt) 0)");
    assert!(
        ws.iter()
            .any(|w| w.contains("defn g: the result") && w.contains("unknown type `intt`")),
        "{ws:?}"
    );
}

/// A private helper passed as a value has no caller-derived parameter types, so its return
/// is read under what its body demands. Its declaration bounds that too: `count` demands
/// only a countable, and `w` read as one widened `unq`'s result past the `string` it is
/// declared over, so a truthiness test on it warned about lists and vectors. Found when
/// `string/length` (whose demand is `string`, which hid this) became `count`.
#[test]
fn a_typed_parameter_bounds_the_return_of_a_private_helper_used_as_a_value() {
    let ws = file_warnings(
        "(defn- unq (w:string) (if (> (count w) 1) (string/substring w 1 2) w))\n\
         (defn use (xs) (or (first (map xs unq)) \"\"))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("for truthiness")),
        "`unq` is declared over strings, so its result is a string: {ws:?}"
    );
}
