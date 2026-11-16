//! The **If-T type-narrowing benchmark** (Guo et al., `utahplt/ifT-benchmark`) ported to
//! Brood (ADR-725): its 13 items, each a program the checker must accept and one it must
//! reject. Every accept case is silent in plain and strict mode; every reject case is
//! reported in strict mode, and in plain mode too unless plain mode's gradual reading is the
//! reason it is not (a merely-wider value, which only strict reads by inclusion).
//!
//! Two reject cases are NOT reported, deliberately: their "unclear" value is an unannotated
//! parameter or a declared `any`, which Brood reads as `dynamic()` — the gradual valve that
//! lets a redefinable program be checked at all (types.md, contract #4). The benchmark's
//! pseudo-language types them as a static top. Those two are pinned silent below, so a
//! change to that reading is a decision someone has to make, not a drift.
//!
//! Translation notes: `String.length` is `%string-length`; a struct is a `(record :a any)`; a
//! user predicate's `x is T` result is inferred from its body, and `implies x is T` is the
//! one-sided guard inference gives an `and`; `merge_with_union`'s reassignment is a `cond`
//! bound by `let` (Brood has no assignment).

use super::*;

fn plain(src: &str) -> Vec<String> {
    file_warnings_mode(src, false)
}

fn strict(src: &str) -> Vec<String> {
    file_warnings_mode(src, true)
}

/// Accepted in both modes.
fn accepts(item: &str, src: &str) {
    let (plain_ws, strict_ws) = (plain(src), strict(src));
    assert!(
        plain_ws.is_empty() && strict_ws.is_empty(),
        "{item}: expected silence, plain {plain_ws:?} strict {strict_ws:?}"
    );
}

/// Rejected in both modes.
fn rejects(item: &str, src: &str) {
    let (plain_ws, strict_ws) = (plain(src), strict(src));
    assert!(
        !plain_ws.is_empty() && !strict_ws.is_empty(),
        "{item}: expected a report, plain {plain_ws:?} strict {strict_ws:?}"
    );
}

/// Rejected in strict mode; plain mode's gradual reading leaves a merely-wider value alone.
fn rejects_strictly(item: &str, src: &str) {
    let strict_ws = strict(src);
    assert!(
        !strict_ws.is_empty(),
        "{item}: expected a strict report, got none"
    );
}

#[test]
fn positive() {
    accepts(
        "positive",
        "(defn f (x) (if (string? x) (%string-length x) x))",
    );
    rejects("positive", "(defn f (x) (if (string? x) (+ x 1) x))");
}

#[test]
fn negative() {
    accepts(
        "negative",
        "(defn f (x:(or string number) -> number) (if (string? x) (%string-length x) (+ x 1)))",
    );
    rejects(
        "negative",
        "(defn f (x:(or string number bool) -> number) (if (string? x) (%string-length x) (+ x 1)))",
    );
}

#[test]
fn connectives() {
    accepts(
        "connectives",
        "(defn f (x:(or string number) -> number) (if (not (number? x)) (%string-length x) 0))\n\
         (defn g (x -> number) (if (or (string? x) (number? x)) (f x) 0))\n\
         (defn h (x:(or string number bool) -> number)\n\
           (if (and (not (bool? x)) (not (number? x))) (%string-length x) 0))",
    );
    rejects(
        "connectives f",
        "(defn f (x:(or string number) -> number) (if (not (number? x)) (+ x 1) 0))",
    );
    rejects(
        "connectives g",
        "(defn g (x -> number) (if (or (string? x) (number? x)) (+ x 1) 0))",
    );
    rejects(
        "connectives h",
        "(defn h (x:(or string number bool) -> number)\n\
           (if (and (not (bool? x)) (not (number? x))) (+ x 1) 0))",
    );
}

#[test]
fn nesting_body() {
    accepts(
        "nesting_body",
        "(defn f (x:(or string number bool) -> number)\n\
           (if (not (string? x)) (if (not (bool? x)) (+ x 1) 0) 0))",
    );
    rejects(
        "nesting_body",
        "(defn f (x:(or string number bool) -> number)\n\
           (if (or (string? x) (number? x)) (if (or (number? x) (bool? x)) (%string-length x) 0) 0))",
    );
}

#[test]
fn struct_fields() {
    accepts(
        "struct_fields",
        "(defn f (x:(record :a any) -> number) (if (number? (:a x)) (:a x) 0))",
    );
    // The body yields `0 | string` against a declared `number`: merely wider.
    rejects_strictly(
        "struct_fields",
        "(defn f (x:(record :a any) -> number) (if (string? (:a x)) (:a x) 0))",
    );
}

#[test]
fn tuple_elements() {
    accepts(
        "tuple_elements",
        "(defn f (x:(tuple any any) -> number) (if (number? (nth x 0)) (nth x 0) 0))",
    );
    // By design, not reported: `(nth x 1)` is a declared `any` — `dynamic()`.
    let src =
        "(defn f (x:(tuple any any) -> number) (if (number? (nth x 0)) (+ (nth x 0) (nth x 1)) 0))";
    assert!(
        plain(src).is_empty() && strict(src).is_empty(),
        "{:?}",
        strict(src)
    );
}

#[test]
fn tuple_length() {
    accepts(
        "tuple_length",
        "(defn f (x:(or (tuple number number) (tuple string string string)) -> number)\n\
           (if (= (count x) 2) (+ (nth x 0) (nth x 1)) (%string-length (nth x 0))))",
    );
    rejects(
        "tuple_length",
        "(defn f (x:(or (tuple number number) (tuple string string string)) -> number)\n\
           (if (= (count x) 2) (+ (nth x 0) (nth x 1)) (+ (nth x 0) (nth x 1))))",
    );
}

#[test]
fn alias() {
    accepts(
        "alias",
        "(defn f (x) (let (y (string? x)) (if y (%string-length x) x)))",
    );
    rejects(
        "alias",
        "(defn f (x) (let (y (string? x)) (if y (+ x 1) x)))",
    );
    // The benchmark's mutable-alias half has no Brood counterpart: bindings never change.
}

#[test]
fn nesting_condition() {
    accepts(
        "nesting_condition",
        "(defn f (x y -> number) (if (if (number? x) (string? y) false) (+ x (%string-length y)) 0))",
    );
    // By design, not reported: on the `(string? y)` path `x` is the unannotated parameter,
    // `dynamic()`. What IS proven is checked — `y` is a string on both paths:
    let src = "(defn f (x y -> number) (if (if (number? x) (string? y) (string? y)) (+ x (%string-length y)) 0))";
    assert!(
        plain(src).is_empty() && strict(src).is_empty(),
        "{:?}",
        strict(src)
    );
    let ws = plain("(defn f (x y) (if (if (number? x) (string? y) (string? y)) (+ 1 y) 0))");
    assert!(
        ws.iter().any(|warning| warning.contains("got string (y)")),
        "{ws:?}"
    );
}

#[test]
fn merge_with_union() {
    accepts(
        "merge_with_union",
        "(defn f (x -> (or string number))\n\
           (let (y (cond (string? x) (str x \"hello\") (number? x) (+ x 1) else nil)) (if (nil? y) 0 y)))",
    );
    // `y` is `number | string`, a let-bound call result: merely wider than `+` wants.
    rejects_strictly(
        "merge_with_union",
        "(defn f (x -> (or string number))\n\
           (let (y (cond (string? x) (str x \"hello\") (number? x) (+ x 1) else nil)) (if (nil? y) 0 (+ y 1))))",
    );
}

#[test]
fn predicate_2way() {
    let helper = "(defn helper (x:(or string number)) (string? x))\n";
    accepts(
        "predicate_2way",
        &format!(
            "{helper}(defn g (x:(or string number) -> number) (if (helper x) (%string-length x) x))"
        ),
    );
    rejects(
        "predicate_2way",
        &format!("{helper}(defn g (x:(or string number) -> number) (if (helper x) (+ x 1) x))"),
    );
}

#[test]
fn predicate_1way() {
    let helper = "(defn helper (x:(or string number)) (and (number? x) (> x 0)))\n";
    accepts(
        "predicate_1way",
        &format!("{helper}(defn g (x:(or string number) -> number) (if (helper x) (+ x 1) 0))"),
    );
    rejects(
        "predicate_1way",
        &format!(
            "{helper}(defn g (x:(or string number) -> number) (if (helper x) (+ x 1) (%string-length x)))"
        ),
    );
}

#[test]
fn predicate_checked() {
    accepts(
        "predicate_checked",
        "(sig helper ((or string number bool) -> (is string)))\n(defn helper (x) (string? x))\n\
         (sig g ((or string number bool) -> (is (or number bool))))\n(defn g (x) (not (helper x)))",
    );
    rejects(
        "predicate_checked f",
        "(sig f ((or string number bool) -> (is string)))\n(defn f (x) (or (string? x) (number? x)))",
    );
    rejects(
        "predicate_checked g",
        "(sig g ((or string number bool) -> (is (or number bool))))\n(defn g (x) (number? x))",
    );
}
