//! A consolidated catalog of what the advisory type checker (ADR-023/024/078)
//! **must catch** and what it **must not** flag (false-positive guards), all in
//! one place. Each case is checked independently via the real `check_file` path
//! (prelude loaded, whole-file mode — what `nest check` runs), so this is a
//! direct, 100%-explicit confirmation of the checker's surface:
//!
//! - structured types: function arrows (callback-arity), sequence element types
//!   (`first`/`last`/`nth`, vector/list literals + constructors), parametric HOF
//!   results (`map`/`filter`/`reduce`/`fold`);
//! - the base disjointness + arity + unbound diagnostics;
//! - the guard-narrowing soundness fixes (`and`-short-circuit and `%eq` must NOT
//!   narrow their else-branch; `match` list-vs-vector patterns).
//!
//! Lives as a Rust test (not a `.blsp` fixture) on purpose: a `.blsp` full of
//! deliberate type errors would be scanned by `nest check` / the project audit
//! and spew warnings. Here each snippet is checked in isolation.

use brood::types::check::check_file;
use brood::Interp;

/// The checker warnings for a self-contained snippet — the exact `check_file`
/// path the CLI/LSP use (prelude loaded, whole-file mode). A fresh `Interp` per
/// call keeps cases independent.
fn warnings(src: &str) -> Vec<String> {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all(&mut interp.heap, src).expect("parse");
    check_file(&mut interp.heap, &forms)
        .into_iter()
        .map(|(_, msg)| msg)
        .collect()
}

/// `(code, needle)` — `code` must produce a warning containing `needle` (so we
/// confirm the *right* diagnostic fired, not an incidental one).
const SHOULD_WARN: &[(&str, &str)] = &[
    // ---- base: disjoint argument, wrong arity ----
    ("(first 5)", "first"),                  // int isn't a sequence
    (r#"(+ 1 "x")"#, "+"),                    // string isn't a number
    ("(rem 1 2 3)", "rem"),                  // arity: expects 2
    // ---- function arrows: callback arity ----
    ("(map cons (list 1 2 3))", "callback"),         // cons is 2-ary; map calls with 1
    ("(map (fn (a b) a) (list 1 2 3))", "callback"), // 2-ary lambda under map
    ("(reduce (fn (a) a) 0 (list 1 2 3))", "callback"), // 1-ary callback; reduce calls with 2
    // ---- element types from literals / constructors ----
    ("(string-length (first [1 2 3]))", "string-length"),       // vector literal → int
    (r#"(+ 1 (first (list "a" "b")))"#, "+"),                    // (list …) → string
    // ---- parametric HOF results: types flow through ----
    ("(string-length (first (map inc (list 1 2 3))))", "string-length"), // map → number
    ("(string-length (first (filter even? (list 1 2 3))))", "string-length"), // filter preserves int
    ("(string-length (reduce + 0 (list 1 2 3)))", "string-length"),      // reduce → number
    (
        "(string-length (fold (fn (acc x) (+ acc x)) 0 (list 1 2 3)))",
        "string-length",
    ), // fold → number (lambda callback)
];

/// Each snippet must produce **zero** warnings — the false-positive guards.
const SHOULD_NOT_WARN: &[&str] = &[
    // ---- correct higher-order calls ----
    "(map inc (list 1 2 3))",                   // right-arity named callback
    "(map + (list 1 2 3))",                     // variadic callback accepts 1
    "(map (fn (x) (+ x 1)) (list 1 2 3))",      // right-arity lambda
    "(reduce + 0 (list 1 2 3))",                // right-arity, numeric
    "(reduce (fn (acc x) (+ acc x)) 0 (list 1 2 3))", // 2-ary lambda for reduce
    // ---- parametric results used correctly (number element is fine for +) ----
    "(+ 1 (first (map inc (list 1 2 3))))",
    "(+ 1 (reduce + 0 (list 1 2 3)))",
    "(+ 1 (first (map (fn (x) x) (list 1 2 3))))", // identity preserves int
    // ---- imprecise-but-overlapping element types must not warn ----
    r#"(+ 1 (first [1 "a"]))"#,                 // int|string|nil overlaps number
    // ---- unknown inputs → no refinement, no warning ----
    "(fn (xs) (+ 1 (first xs)))",               // unknown sequence
    "(fn (f) (map f (list 1 2 3)))",            // local callback, unknown arity
    "(fn (init) (string-length (reduce + init (list 1 2 3))))", // unknown init type
    // ---- guard-narrowing soundness (the fixed false positives) ----
    // `and` short-circuit: a falsy `(and (vector? m) …)` doesn't prove m isn't a
    // vector → the else-branch must NOT narrow m.
    "(fn (m) (if (and (vector? m) (%eq (vector-length m) 2)) (vector-ref m 0) (vector-ref m 0)))",
    // `%eq`: `m ≠ \"x\"` doesn't prove m isn't a string → else-branch not narrowed.
    r#"(fn (m) (if (%eq m "x") :yes (string-length m)))"#,
    // match: a list value against a vector pattern lowers to a guarded vector-ref
    // that must stay quiet (the scrutinee narrows to a vector inside the guard).
    "(match (list 1 2) ([a b] :vec) (_ :not-vec))",
    // ---- correct occurrence typing (then-branch narrowing is sound) ----
    "(fn (x) (if (int? x) (+ x 1) 0))",
];

#[test]
fn checker_catches_every_should_warn_case() {
    for (code, needle) in SHOULD_WARN {
        let w = warnings(code);
        assert!(
            w.iter().any(|m| m.contains(needle)),
            "expected a warning containing {needle:?} for:\n    {code}\ngot: {w:?}"
        );
    }
}

#[test]
fn checker_is_silent_on_every_should_not_warn_case() {
    for code in SHOULD_NOT_WARN {
        let w = warnings(code);
        assert!(
            w.is_empty(),
            "expected NO warnings (false-positive) for:\n    {code}\ngot: {w:?}"
        );
    }
}
