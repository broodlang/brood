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
    ("(math/rem 1 2 3)", "rem"),                  // arity: expects 2
    // ---- function arrows: callback arity ----
    ("(map (list 1 2 3) cons)", "callback"),         // cons is 2-ary; map calls with 1
    ("(map (list 1 2 3) (fn (a b) a))", "callback"), // 2-ary lambda under map
    ("(reduce (list 1 2 3) 0 (fn (a) a))", "callback"), // 1-ary callback; reduce calls with 2
    ("(map (list 1 2 3) (fn (a b & c) a))", "callback"), // variadic lambda needs >=2; map calls with 1
    ("(map (list 1 2 3) (fn (a b &optional c) a))", "callback"), // 2 required + optional; min 2 > 1
    // ---- element types from literals / constructors ----
    ("(string/length (first [1 2 3]))", "string/length"),       // vector literal → int
    (r#"(+ 1 (first (list "a" "b")))"#, "+"),                    // (list …) → string
    // ---- parametric HOF results: types flow through ----
    ("(string/length (first (map (list 1 2 3) inc)))", "string/length"), // map → number
    ("(string/length (first (filter (list 1 2 3) even?)))", "string/length"), // filter preserves int
    ("(string/length (reduce (list 1 2 3) 0 +))", "string/length"),      // reduce → number
    (
        "(string/length (fold (list 1 2 3) 0 (fn (acc x) (+ acc x))))",
        "string/length",
    ), // fold → number (lambda callback)
    // ---- element types preserved through structural combinators ----
    ("(string/length (first (reverse [1 2 3])))", "string/length"),      // reverse vector<int> → int
    ("(string/length (first (sort [1 2 3])))", "string/length"),         // sort preserves int
    ("(string/length (first (sort-by (fn (x) x) [1 2 3])))", "string/length"), // sort-by preserves int
    ("(string/length (first (take [1 2 3] 2)))", "string/length"),       // take preserves int
    ("(string/length (first (drop [1 2 3] 1)))", "string/length"),       // drop preserves int
    ("(string/length (first (cons 1 (list 2 3))))", "string/length"),    // cons: int | int = int
    ("(string/length (first (append [1 2] [3 4])))", "string/length"),   // append: int ∪ int = int
    // ---- type-variable sigs: return type resolved from argument types ----
    (
        "(sig identity (?A -> ?A)) (defn identity (x) x) (string/length (identity 42))",
        "string/length",
    ), // identity(?A → ?A) on int → int, not a string
    (
        "(sig my-first ((list ?A) -> ?A)) (defn my-first (xs) (first xs)) (string/length (my-first (list 1 2 3)))",
        "string/length",
    ), // my-first on list<int> → int
    (
        r#"(sig const (?A ?B -> ?A)) (defn const (x y) x) (string/length (const 42 "x"))"#,
        "string/length",
    ), // const(?A ?B → ?A) on (int str) → int
    // ---- expanded curated sigs: predicates return bool ----
    ("(+ 1 (number? 42))", "+"),              // number? → bool, not number
    ("(+ 1 (empty? (list)))", "+"),           // empty? → bool
    ("(+ 1 (list? (list 1 2)))", "+"),        // list? → bool
    ("(+ 1 (contains? {:a 1} :a))", "+"),     // contains? → bool
    ("(+ 1 (includes? (list 1 2) 1))", "+"),    // includes? → bool
    ("(+ 1 (any? (list 1 2) int?))", "+"),    // any? → bool
    ("(+ 1 (every? (list 1 2) int?))", "+"),  // every? → bool
    // ---- expanded curated sigs: string converters ----
    (r#"(+ 1 (string/join ", " (list "a" "b")))"#, "+"), // join → string
    (r#"(+ 1 (string/capitalize "hello"))"#, "+"), // capitalize → string
    // ---- op names must be unique within a module (ADR-172) ----
    // two abilities declaring the same op name `area` clobber each other's generic fn.
    (
        "(defability Shape (area [self])) (defability Sizer (area [thing]))",
        "unique per module",
    ),
    // ---- multimethod missing-method: a fully-known tuple with no method (ADR-179) ----
    (
        "(defmulti mm) (defmethod mm [:int :int] (a b) a) (defn f () (mm 1 \"x\"))",
        "no method for",
    ),
    // inference hook: a record-typed *variable* arg with no method (ADR-179).
    (
        "(defrecord usd (cents)) (defmulti scale) (defmethod scale [usd :int] (m n) m) \
         (defn f () (let (x (usd 1)) (scale x 2.5)))",
        "no method for",
    ),
    // operator sugar: `(+ record scalar)` routes to num/add; no method → warn (ADR-179).
    (
        "(defrecord usd (cents)) (defmethod num/add [usd usd] (a b) a) (defn f () (+ (usd 1) 2.5))",
        "num/add",
    ),
    // operator sugar: `(< record scalar)` routes to compare-to.
    (
        "(defrecord usd (cents)) (defmethod compare-to [usd usd] (a b) 0) (defn f () (< (usd 1) 5))",
        "compare-to",
    ),
];

/// Each snippet must produce **zero** warnings — the false-positive guards.
const SHOULD_NOT_WARN: &[&str] = &[
    // ---- correct higher-order calls ----
    "(map (list 1 2 3) inc)",                   // right-arity named callback
    "(map (list 1 2 3) +)",                     // variadic callback accepts 1
    "(map (list 1 2 3) (fn (x) (+ x 1)))",      // right-arity lambda
    "(reduce (list 1 2 3) 0 +)",                // right-arity, numeric
    "(reduce (list 1 2 3) 0 (fn (acc x) (+ acc x)))", // 2-ary lambda for reduce
    "(map (list 1 2 3) (fn (& xs) (apply + xs)))", // variadic lambda (math/min 0) accepts 1
    "(map (list 1 2 3) (fn (x &optional y) x))", // 1 required + optional accepts 1
    "(reduce (list 1 2 3) 0 (fn (acc x & more) (+ acc x)))", // variadic min 2 == reduce's 2
    // ---- parametric results used correctly (number element is fine for +) ----
    "(+ 1 (first (map (list 1 2 3) inc)))",
    "(+ 1 (reduce (list 1 2 3) 0 +))",
    "(+ 1 (first (map (list 1 2 3) (fn (x) x))))", // identity preserves int
    // ---- imprecise-but-overlapping element types must not warn ----
    r#"(+ 1 (first [1 "a"]))"#,                 // int|string|nil overlaps number
    // ---- unknown inputs → no refinement, no warning ----
    "(fn (xs) (+ 1 (first xs)))",               // unknown sequence
    "(fn (f) (map (list 1 2 3) f))",            // local callback, unknown arity
    "(fn (init) (string/length (reduce (list 1 2 3) init +)))", // unknown init type
    // ---- structural combinators: correct uses stay silent ----
    "(+ 1 (first (reverse [1 2 3])))",          // int element is fine for +
    "(+ 1 (first (sort [1 2 3])))",
    "(+ 1 (first (take [1 2 3] 2)))",
    "(+ 1 (first (drop [1 2 3] 1)))",
    "(+ 1 (first (cons 1 (list 2 3))))",        // int | int = int, fine for +
    "(+ 1 (first (append [1 2] [3 4])))",
    // unknown sequence → no refinement propagated, no warning
    "(fn (xs) (+ 1 (first (reverse xs))))",
    "(fn (xs ys) (+ 1 (first (append xs ys))))", // both unknown → unrefined
    // ---- guard-narrowing soundness (the fixed false positives) ----
    // `and` short-circuit: a falsy `(and (vector? m) …)` doesn't prove m isn't a
    // vector → the else-branch must NOT narrow m.
    "(fn (m) (if (and (vector? m) (%eq (seq/vector-length m) 2)) (seq/vector-ref m 0) (seq/vector-ref m 0)))",
    // `%eq`: `m ≠ \"x\"` doesn't prove m isn't a string → else-branch not narrowed.
    r#"(fn (m) (if (%eq m "x") :yes (string/length m)))"#,
    // match: a list value against a vector pattern lowers to a guarded vector-ref
    // that must stay quiet (the scrutinee narrows to a vector inside the guard).
    "(match (list 1 2) ([a b] :vec) (_ :not-vec))",
    // ---- correct occurrence typing (then-branch narrowing is sound) ----
    "(fn (x) (if (int? x) (+ x 1) 0))",
    // ---- expanded curated sigs: correct uses stay silent ----
    "(if (number? 42) :yes :no)",             // number? used as a predicate (bool is fine)
    "(if (empty? (list)) :yes :no)",          // empty? as predicate
    r#"(if (contains? {:a 1} :a) :yes :no)"#, // contains? as predicate
    r#"(string/length (string/join ", " (list "a")))"#, // join→string→length fine
    // ---- type-variable sigs: correct uses stay silent ----
    "(sig identity (?A -> ?A)) (defn identity (x) x) (+ 1 (identity 42))",
    "(sig my-first ((list ?A) -> ?A)) (defn my-first (xs) (first xs)) (+ 1 (my-first (list 1 2 3)))",
    r#"(sig const (?A ?B -> ?A)) (defn const (x y) x) (+ 1 (const 42 "x"))"#,
    // ---- ability op-name uniqueness: only a REAL same-name collision warns ----
    // distinct op names across two abilities are fine (each binds its own generic fn).
    "(defability Shape (area [self])) (defability Boxer (volume [self]))",
    // redeclaring the SAME ability (hot reload) re-binds the same op — not a collision.
    "(defability Shape (area [self])) (defability Shape (area [self]))",
    // ---- multimethod coverage: a covered tuple, a derived mirror, and :default stay silent ----
    // an exact method covers the call.
    "(defmulti mm) (defmethod mm [:int :int] (a b) a) (defn f () (mm 1 2))",
    // a :commutative op's [A B] method also covers the mirror order [B A] — must NOT warn.
    "(defmulti mm :commutative) (defmethod mm [:int :string] (a b) a) (defn f () (mm \"x\" 1))",
    // a :default method catches any tuple.
    "(defmulti mm) (defmethod mm :default (a b) a) (defn f () (mm 1 \"x\"))",
    // an arg of unknown identity (a variable) leaves the tuple uncertain — defer, don't warn.
    "(defmulti mm) (defmethod mm [:int :int] (a b) a) (defn f (x) (mm x 2))",
    // ---- operator sugar: only a record operand is checked, and a covered pair stays silent ----
    // pure numbers never route to a multimethod — `(+ 1 2)` must NOT warn.
    "(defrecord usd (cents)) (defmethod num/add [usd usd] (a b) a) (defn f () (+ 1 2))",
    // a covered record pair (`num/add [usd usd]`) stays silent.
    "(defrecord usd (cents)) (defmethod num/add [usd usd] (a b) a) (defn f () (+ (usd 1) (usd 2)))",
    // a covered comparison pair (`compare-to [usd usd]`) stays silent.
    "(defrecord usd (cents)) (defmethod compare-to [usd usd] (a b) 0) (defn f () (< (usd 1) (usd 2)))",
    // inference: a covered record-variable call stays silent.
    "(defrecord usd (cents)) (defmulti scale) (defmethod scale [usd :int] (m n) m) \
     (defn f () (let (x (usd 1)) (scale x 3)))",
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

/// The (line, col) of the first warning for `src` whose message contains
/// `needle`, or None. Uses the positioned `check_file` output directly.
fn warning_pos(src: &str, needle: &str) -> Option<(u32, u32)> {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all(&mut interp.heap, src).expect("parse");
    check_file(&mut interp.heap, &forms)
        .into_iter()
        .find(|(_, m)| m.contains(needle))
        .and_then(|(p, _)| p.map(|p| (p.line, p.col)))
}

/// Finer finding spans (2026-07-23): a type/callback-arity finding anchors at
/// the offending ARGUMENT when it is a positioned sub-form (a nested call),
/// not the call head.
#[test]
fn type_findings_anchor_at_the_offending_argument() {
    // `(+ 10 20)` starts at column 16; the call head `string-length` at 1.
    // Before the fix this pointed at column 1.
    let src = "(string/length (+ 10 20))";
    let (line, col) = warning_pos(src, "string/length").expect("a warning");
    assert_eq!(line, 1);
    assert_eq!(
        col, 16,
        "the type finding should anchor at the argument `(+ 10 20)` (col 16), not the call head"
    );

    // A callback-arity finding likewise points at the callback argument.
    // `(fn (a b) a)` is the second token after `(map ` → column 6.
    let cb = "(map (list 1 2 3) (fn (a b) a))";
    let (l2, c2) = warning_pos(cb, "callback").expect("a callback warning");
    assert_eq!(l2, 1);
    assert_eq!(
        c2, 6,
        "the callback finding should anchor at the lambda argument"
    );

    // A bare (unpositioned) literal argument falls back to the call form.
    let lit = "(string/length 42)";
    let (l3, c3) = warning_pos(lit, "string/length").expect("a warning");
    assert_eq!(
        (l3, c3),
        (1, 1),
        "a bare-literal arg falls back to the call head"
    );
}
