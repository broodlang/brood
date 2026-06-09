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
    // ---- element types preserved through structural combinators ----
    ("(string-length (first (reverse [1 2 3])))", "string-length"),      // reverse vector<int> → int
    ("(string-length (first (sort [1 2 3])))", "string-length"),         // sort preserves int
    ("(string-length (first (sort-by (fn (x) x) [1 2 3])))", "string-length"), // sort-by preserves int
    ("(string-length (first (take 2 [1 2 3])))", "string-length"),       // take preserves int
    ("(string-length (first (drop 1 [1 2 3])))", "string-length"),       // drop preserves int
    ("(string-length (first (cons 1 (list 2 3))))", "string-length"),    // cons: int | int = int
    ("(string-length (first (append [1 2] [3 4])))", "string-length"),   // append: int ∪ int = int
    ("(string-length (first (concat [1 2] [3 4])))", "string-length"),   // concat: same
    // ---- type-variable sigs: return type resolved from argument types ----
    (
        "(sig identity (?A -> ?A)) (defn identity (x) x) (string-length (identity 42))",
        "string-length",
    ), // identity(?A → ?A) on int → int, not a string
    (
        "(sig my-first ((list ?A) -> ?A)) (defn my-first (xs) (first xs)) (string-length (my-first (list 1 2 3)))",
        "string-length",
    ), // my-first on list<int> → int
    (
        r#"(sig const (?A ?B -> ?A)) (defn const (x y) x) (string-length (const 42 "x"))"#,
        "string-length",
    ), // const(?A ?B → ?A) on (int str) → int
    // ---- expanded curated sigs: predicates return bool ----
    ("(+ 1 (number? 42))", "+"),              // number? → bool, not number
    ("(+ 1 (empty? (list)))", "+"),           // empty? → bool
    ("(+ 1 (list? (list 1 2)))", "+"),        // list? → bool
    ("(+ 1 (contains? {:a 1} :a))", "+"),     // contains? → bool
    ("(+ 1 (member? 1 (list 1 2)))", "+"),    // member? → bool
    ("(+ 1 (some? int? (list 1 2)))", "+"),   // some? → bool
    ("(+ 1 (every? int? (list 1 2)))", "+"),  // every? → bool
    // ---- expanded curated sigs: string converters ----
    (r#"(+ 1 (symbol->string 'foo))"#, "+"),  // symbol->string → string
    (r#"(+ 1 (join ", " (list "a" "b")))"#, "+"), // join → string
    (r#"(+ 1 (string-capitalize "hello"))"#, "+"), // string-capitalize → string
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
    // ---- structural combinators: correct uses stay silent ----
    "(+ 1 (first (reverse [1 2 3])))",          // int element is fine for +
    "(+ 1 (first (sort [1 2 3])))",
    "(+ 1 (first (take 2 [1 2 3])))",
    "(+ 1 (first (drop 1 [1 2 3])))",
    "(+ 1 (first (cons 1 (list 2 3))))",        // int | int = int, fine for +
    "(+ 1 (first (append [1 2] [3 4])))",
    // unknown sequence → no refinement propagated, no warning
    "(fn (xs) (+ 1 (first (reverse xs))))",
    "(fn (xs ys) (+ 1 (first (append xs ys))))", // both unknown → unrefined
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
    // ---- expanded curated sigs: correct uses stay silent ----
    "(if (number? 42) :yes :no)",             // number? used as a predicate (bool is fine)
    "(if (empty? (list)) :yes :no)",          // empty? as predicate
    r#"(if (contains? {:a 1} :a) :yes :no)"#, // contains? as predicate
    r#"(string-length (symbol->string 'foo))"#, // symbol→string→length is fine
    r#"(string-length (join ", " (list "a")))"#, // join→string→length fine
    // ---- type-variable sigs: correct uses stay silent ----
    "(sig identity (?A -> ?A)) (defn identity (x) x) (+ 1 (identity 42))",
    "(sig my-first ((list ?A) -> ?A)) (defn my-first (xs) (first xs)) (+ 1 (my-first (list 1 2 3)))",
    r#"(sig const (?A ?B -> ?A)) (defn const (x y) x) (+ 1 (const 42 "x"))"#,
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
