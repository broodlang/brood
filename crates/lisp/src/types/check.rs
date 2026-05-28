//! Step 4: a small **advisory** type checker — the consumer of the `Ty` lattice,
//! so the type system actually *does* something.
//!
//! It walks a macro-expanded form and warns when a call passes an argument that
//! is *provably* the wrong type — its type is **disjoint** from what the callee
//! accepts (`(first 5)`, `(+ 1 "x")`). Disjointness (not subtyping) is the rule,
//! so a superset (`number` where `int` is wanted), an `any` result, or an
//! unknown argument all overlap the expected type and are never flagged — **no
//! false positives**. It never raises and never gates — it returns warnings
//! (contract point #5).
//!
//! ## Module map
//!
//! Split by concern, not by special form:
//! - [`ctx`] — the `Ctx` value the walk threads, recording binders, type
//!   narrowings, guard aliases, and file-local globals.
//! - [`sigs`] — where signatures + arities come from (primitive / curated /
//!   one-step-inferred).
//! - [`guards`] — predicates on forms: which heads are syntax keywords,
//!   which `if`-tests are recognisable guards, what an expression's type is.
//! - [`walk`] — the recursive `check_into` and the per-special-form helpers
//!   (`if`/`let`/`fn`/`def`/`defn`) plus `collect_def_names`.
//!
//! ## Where signatures come from (Step 3)
//!
//! Three sources, simplest-first — *no inference engine* (`docs/types.md`):
//!
//! 1. **Primitives** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    a `Sig` ([contract point #6, enforced](../docs/types.md#compatibility-contract))
//!    so the checker just reads it from the global env (see
//!    [`sigs::primitive_sig`]). There is no parallel table to maintain.
//! 2. **Curated stdlib** — a small hand-vetted table for the variadic /
//!    `reduce`-based / higher-order Brood closures the checker can't infer but
//!    that matter (`+ - * / < <= > >= mod map filter reduce`; see
//!    [`sigs::curated_sig`]). Each is a Brood `defn`, but its sig is pinned by hand.
//! 3. **Basic inference** for a closure whose body is **one straight-line
//!    expression** (a single direct call to a known sig; no `if`/`cond`/`let`/
//!    `match`/recursion). Each closure parameter inherits the type the callee
//!    expects at the position(s) where the parameter is passed; the closure's
//!    return is the callee's. Sound because a straight-line use is
//!    unconditional — no control-flow analysis (see [`sigs::sig_of`]).
//!
//! Argument types in a call come from literals, nested calls with a known
//! return type, and **a context-tracked map of local-variable narrowings**:
//!
//! - A `let`/`let*` binding's RHS contributes its `expr_ty` as the variable's
//!   type (so `(let (x 1) (first x))` flags `first` — `x` is known `int`).
//! - An `if`'s test is matched against the predicate-narrowing table
//!   ([`Ty::tested_by`]). On a `(pred? sym)` test the *then*-branch narrows
//!   `sym` to `tested_by(pred)`, the *else*-branch to its complement; a leading
//!   `(not …)` flips the assertion. Bindings inside a branch override the
//!   narrowing as ordinary shadowing.
//!
//! Vocabulary is `Option<Ty>` (known / unknown), not `GradualTy` — the
//! disjointness check only needs "do I know this type?". Forms inside `try` /
//! `error-of` / `assert-error` are skipped (they deliberately exercise failures).
//!
//! ## Beyond type misuse
//!
//! The walk also emits two non-type diagnostics, sharing the same scope
//! infrastructure:
//!
//! - **Arity**: a call whose argument count isn't admitted by the callee's
//!   declared `Arity` (from [`NativeFn`](crate::core::value::NativeFn) for a
//!   primitive, or from `Closure.{params, optionals, rest}` for a Brood
//!   closure). See [`sigs::arity_of`].
//! - **Unbound symbols**: a call head that resolves to nothing — not a
//!   primitive, not a curated stdlib closure, not in local scope (fn/let), not
//!   a file-local def, not a syntactic keyword, and not in the heap's globals.
//!   Driven by [`Ctx::is_local`](ctx::Ctx::is_local) (the local + file-global
//!   view) plus a global-env lookup. Scope is honoured: `fn`/`lambda`/`defn`/
//!   `defmacro` bind their params into `Ctx` before walking the body, and
//!   [`check_file`] accumulates top-level `def`/`defn`/`defmacro` / `defdyn`
//!   names across the forms in a file.
//!
//! Not yet (later increments): inference through `cond`/`match`, structured /
//! `and`/`or`-chained guards, recursion, higher-order; running automatically
//! in `brood <file>` / `nest test`. Today the entry points are `brood
//! --check` (CLI; see [`check_file`]) and the `(check 'form)` builtin.

mod ctx;
mod guards;
mod sigs;
mod walk;

use crate::core::heap::Heap;
use crate::core::value::Value;
use crate::error::Pos;

use ctx::Ctx;
use walk::{check_into, collect_def_names};

/// Check one form, returning a warning per provable misuse. Empty when nothing is
/// provably wrong (which includes "not enough static info").
pub fn check_form(heap: &Heap, form: Value) -> Vec<String> {
    check_located(heap, form)
        .into_iter()
        .map(|(_, msg)| msg)
        .collect()
}

/// Like [`check_form`], but each warning carries the source `Pos` of the call it
/// was found in (when known) — for `file:line:col:` diagnostics from `brood
/// --check` / `nest check`. The position is the *call form*'s, recorded by the
/// reader; an unrecorded form (e.g. one a macro synthesised) yields `None`.
pub fn check_located(heap: &Heap, form: Value) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    check_into(heap, form, &Ctx::default(), &mut out);
    out
}

/// Check a sequence of top-level forms together, threading file-local
/// definitions across them so a `(defn foo …)` at the top isn't flagged when
/// a later form calls `foo`. This is the entry point for `brood --check
/// <file>` / `nest check`.
///
/// Each form is **macro-expanded first** (like the `(check 'form)` builtin),
/// so threading macros (`->`/`->>`), pattern syntax (`match`), test framework
/// wrappers (`test`/`describe`/…), and any user macro that rearranges code
/// are checked against their *expanded* shape — not the surface syntax that
/// would otherwise mistake `(map inc)` inside `(->> xs (map inc))` for a
/// 1-arg call. Source positions survive expansion where the macro rebuilds
/// through `rebuild_list` (the common case); positions on macro-introduced
/// new code are absent.
///
/// File-local def names are accumulated by a **recursive** scan over the
/// expanded forms, so a `(defn foo …)` nested inside a macro body
/// (e.g. inside `(test … (defn foo …) …)`) still shields a later `(foo …)`
/// — `def`s define globally in Brood regardless of nesting position
/// (`docs/language.md`).
///
/// A form whose macroexpansion fails (a malformed macro call) falls back to
/// its un-expanded shape — the eval path will surface the same parse-time
/// error later anyway, so the checker just stays quiet there.
pub fn check_file(heap: &mut Heap, forms: &[Value]) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    // Pass 1: macroexpand each form (recording the expanded shape we'll also
    // walk in pass 2). A macroexpand failure isn't this pass's job to report,
    // so we fall back to the un-expanded form silently.
    let root = heap.global();
    let expanded: Vec<Value> = forms
        .iter()
        .map(|&f| crate::eval::macros::macroexpand_all(heap, f, root).unwrap_or(f))
        .collect();
    // Pass 2: collect every `(def name …)` in the expanded tree (top level
    // *or* nested — `defn` inside `test`/`describe`/`when`/… still defines a
    // global once it runs, so the checker honours that). `defmacro` stays a
    // special form (it doesn't expand to `def`), so we match it too.
    let mut ctx = Ctx::default();
    for &form in &expanded {
        collect_def_names(heap, form, &mut ctx);
    }
    // Pass 3: check each expanded form with the accumulated file-globals.
    for &form in &expanded {
        check_into(heap, form, &ctx, &mut out);
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    // The submodules' items are still accessed by name in these tests —
    // import them explicitly now that they're not all in this file.
    use super::sigs::primitive_sig;
    use crate::core::value::Tag;
    use crate::syntax::reader;
    use crate::types::Ty;

    /// A full `Interp` — primitives + the loaded prelude. We need the prelude
    /// in the global env so the new unbound-symbol diagnostic doesn't false-
    /// flag every Brood-side stdlib name (`list`, `int?`, `zero?`, `inc`, …);
    /// the previous primitives-only setup worked when the checker silently
    /// skipped unknown callees, but Step 4's unbound check has to know what's
    /// genuinely bound.
    fn warnings(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        check_form(&interp.heap, form)
    }

    /// `warnings` but with macroexpansion — what `(check 'form)` and
    /// `check-file` actually do. Required to exercise post-expansion shapes
    /// like `match` (a `defmacro` whose pattern compiler lowers to
    /// `let`+`if`+`%eq`), threading macros, and the test-framework wrappers.
    fn warnings_expanded(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        check_form(&interp.heap, form)
    }

    #[test]
    fn flags_literal_misuse_of_primitives() {
        assert!(warnings("(first 5)")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
        assert!(warnings("(string-length :k)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("keyword")));
        assert!(warnings("(%add 1 \"x\")")
            .iter()
            .any(|w| w.contains("%add")));
        assert!(warnings("(vector-ref [1 2] :k)")
            .iter()
            .any(|w| w.contains("vector-ref")));
    }

    #[test]
    fn no_false_positives_when_type_is_unknown_or_right() {
        assert!(warnings("(first (list 1 2))").is_empty()); // arg is a non-sig call → dynamic
        assert!(warnings("(first xs)").is_empty()); // variable → dynamic
        assert!(warnings("(first [1 2 3])").is_empty()); // vector is allowed
        assert!(warnings("(%add 1 2)").is_empty());
        assert!(warnings("(string-length \"hi\")").is_empty());
    }

    #[test]
    fn propagates_primitive_result_types() {
        // string-length returns int; first wants a list/vector → flag the int.
        assert!(warnings("(first (string-length \"a\"))")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
    }

    #[test]
    fn an_any_result_is_not_a_false_positive() {
        // vector-ref's result type is `any` (unknown), so feeding it to
        // string-length (wants string) must NOT warn — `any` overlaps `string`.
        assert!(warnings("(string-length (vector-ref [1] 0))").is_empty());
    }

    #[test]
    fn does_not_descend_into_quote() {
        assert!(warnings("(quote (first 5))").is_empty());
    }

    #[test]
    fn curated_closures_are_checked() {
        // `+`, `<`, `map` are Brood closures, but their curated sigs let us flag
        // provable misuse — the headline cases.
        assert!(warnings("(+ 1 \"x\")")
            .iter()
            .any(|w| w.contains('+') && w.contains("number")));
        assert!(warnings("(< 1 :k)").iter().any(|w| w.contains('<')));
        // map's first argument must be callable; an int is not.
        assert!(warnings("(map 1 xs)")
            .iter()
            .any(|w| w.contains("map") && w.contains("argument 1")));
        // Correct uses, and an unknown (variable) callable, stay silent.
        assert!(warnings("(+ 1 2)").is_empty());
        assert!(warnings("(map inc xs)").is_empty()); // inc is a variable → unknown
    }

    #[test]
    fn skips_error_testing_forms() {
        // `try` and the error-asserting helpers deliberately exercise failures,
        // so misuse inside them is not flagged.
        assert!(warnings("(try (first 5) (catch e e))").is_empty());
        assert!(warnings("(error-of (first 5))").is_empty());
        assert!(warnings("(assert-error (first 5))").is_empty());
        // ...but a sibling form outside the skipped one is still checked.
        assert!(!warnings("(do (first 5) (try (first 6) (catch e e)))").is_empty());
    }

    #[test]
    fn covers_the_other_signed_primitives() {
        assert!(warnings("(mod 7 3)").is_empty());
        assert!(warnings("(mod 7 \"x\")").iter().any(|w| w.contains("mod")));
        assert!(warnings("(rem :a 3)").iter().any(|w| w.contains("rem")));
        assert!(warnings("(vector-length 5)")
            .iter()
            .any(|w| w.contains("vector-length")));
        assert!(warnings("(substring \"hi\" \"a\" 1)")
            .iter()
            .any(|w| w.contains("substring") && w.contains("argument 2")));
        assert!(warnings("(%lt 1 :k)").iter().any(|w| w.contains("%lt")));
    }

    #[test]
    fn reports_each_bad_argument() {
        // Both args provably wrong → two distinct warnings (one per position).
        let w = warnings("(mod \"a\" :b)");
        assert_eq!(w.len(), 2, "{:?}", w);
        assert!(w.iter().any(|s| s.contains("argument 1")));
        assert!(w.iter().any(|s| s.contains("argument 2")));
    }

    #[test]
    fn nested_misuse_is_found() {
        // A wrong call buried inside an argument is still reported.
        let w = warnings("(vector-length (cons (first 5) 2))");
        assert!(w.iter().any(|s| s.contains("first")));
    }

    #[test]
    fn atoms_and_malformed_forms_do_not_panic() {
        for src in ["5", "foo", "\"s\"", ":k", "()", "(5 6 7)", "(first)"] {
            // No panic, and no spurious warning on a bare atom / non-symbol head /
            // missing argument.
            let _ = warnings(src);
        }
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol — no diagnostics
        // `(first)` is now an arity diagnostic (0 args; first needs 1).
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1")));
    }

    // ------------- Step 3: sigs sourced from NativeFn, closure inference --------------

    /// The eight test cases below need real user-defined closures, which means
    /// running a `defn` against the global table. The `Interp` builds the full
    /// prelude (curated stdlib closures and all) on top of the primitive kernel
    /// — exactly the surface a checker is supposed to see.
    fn check_with_defs(defs: &[&str], src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        for d in defs {
            interp.eval_str(d).expect("def");
        }
        let form =
            crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse expression");
        // Macro-expand so any prelude wrappers (defn → fn, etc.) are gone, like
        // `brood --check`/the `check` builtin do before calling check_form.
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        check_form(&interp.heap, form)
    }

    #[test]
    fn primitive_sigs_are_read_from_native_fn() {
        // The point of Step 3: there is no parallel `primitive_sig` table.
        // The sig the checker uses for `string-length` *is* the one declared
        // next to its `Arity` in `builtins.rs`. If we ever drop the sig field
        // (or set it wrong), this catches it.
        let interp = crate::Interp::new();
        let sig = primitive_sig(&interp.heap, "string-length")
            .expect("string-length is a primitive");
        assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
        assert_eq!(sig.ret, Ty::of(Tag::Int));
        // The "no useful info" lane: a variadic any-arg primitive (str) returns
        // a Sig that param-overlaps every input, so it never warns.
        let any_sig = primitive_sig(&interp.heap, "str").expect("str is a primitive");
        assert_eq!(any_sig.rest, Some(Ty::ANY));
    }

    #[test]
    fn infers_a_straight_line_wrapper() {
        // (defn inc (x) (+ x 1)) → x : number (from +'s rest type).
        // So `(inc :k)` is a provable misuse.
        let w = check_with_defs(&["(defn inc (x) (+ x 1))"], "(inc :k)");
        assert!(
            w.iter().any(|s| s.contains("inc") && s.contains("number")),
            "expected an `inc :k` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_return_type_propagates() {
        // (defn inc (x) (+ x 1)) returns the number `+` returns; feeding it into
        // `string-length` (wants string) is a provable misuse.
        let w = check_with_defs(
            &["(defn inc (x) (+ x 1))"],
            "(string-length (inc 1))",
        );
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "expected a `string-length` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_params_intersect_across_positions() {
        // (defn add (x y) (+ x y)) — both x and y at + positions → number.
        let w = check_with_defs(&["(defn add (x y) (+ x y))"], "(add \"a\" 2)");
        assert!(w.iter().any(|s| s.contains("add")), "got {:?}", w);
    }

    #[test]
    fn does_not_infer_through_branches_or_lets() {
        // A body with `if`/`let` is *not* a single straight-line expression —
        // inference must skip it, leaving the closure as untyped (no warning).
        // The point: zero false positives from inference's lack of control flow.
        for (defs, call) in &[
            ("(defn maybe (x) (if (int? x) (+ x 1) x))", "(maybe :k)"),
            ("(defn shadow (x) (let (y x) (+ y 1)))", "(shadow :k)"),
        ] {
            let w = check_with_defs(&[defs], call);
            assert!(
                w.is_empty(),
                "branchy / binding bodies must not infer (so no warning): {:?}",
                w
            );
        }
    }

    #[test]
    fn does_not_infer_through_recursion() {
        // A self-recursive call has no fixed sig to read from — must skip,
        // even though the body is structurally a single call.
        let w = check_with_defs(&["(defn loop (x) (loop x))"], "(loop :k)");
        assert!(w.is_empty(), "recursive defns must not infer: {:?}", w);
    }

    #[test]
    fn skips_inference_for_variadic_or_optional_closures() {
        // A variadic-tail closure isn't a "fixed-arity straight-line" — skip.
        let w = check_with_defs(&["(defn vlist (& xs) (first xs))"], "(vlist 1 2 3)");
        assert!(w.is_empty(), "variadic defns must not infer: {:?}", w);
    }

    // ------------- Step 4: scope tracking + guard narrowing --------------

    #[test]
    fn let_binding_propagates_its_rhs_type() {
        // The RHS is a literal int — `(first x)` should flag, because x : int
        // shadows "unknown" in the body. (This is the basic let-tracking.)
        let w = warnings("(let (x 1) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_from_nested_call_propagates() {
        // RHS is a known primitive whose return type is int. So `x : int`,
        // and `(first x)` flags.
        let w = warnings("(let (x (string-length \"hi\")) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_of_unknown_rhs_stays_silent() {
        // RHS is a variable (unknown), so x stays unknown — `(first x)` must
        // not warn. (No false positives from let-tracking.)
        let w = warnings("(let (x foo) (first x))");
        assert!(w.is_empty(), "got {:?}", w);
    }

    #[test]
    fn inner_let_shadows_outer_binding() {
        // The outer x : int; the inner x : string. `(first x)` in the body
        // refers to the inner, which is a string — and `first` accepts list /
        // vector, disjoint from string. So a warning is still expected, but
        // the *narrowing message* must be "string", not "int". This is the
        // shadowing-correctness check (outer narrowing must not leak in).
        let w = warnings("(let (x 1) (let (x \"hi\") (first x)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("string")),
            "expected the inner string to be the source, got {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "outer int must not leak through shadowing: {:?}",
            w
        );
    }

    #[test]
    fn shadowing_with_unknown_rhs_clears_prior_narrowing() {
        // Outer x : int; inner x : <unknown var>. Inside the inner let, x is
        // unknown — `(first x)` must NOT warn (the outer narrowing must not
        // leak through the shadow).
        let w = warnings("(let (x 1) (let (x foo) (first x)))");
        assert!(w.is_empty(), "shadow must clear the prior type: {:?}", w);
    }

    #[test]
    fn vector_let_bindings_are_recognised() {
        // `(let [x 1] …)` (vector shape) must work the same as `(let (x 1) …)`.
        let w = warnings("(let [x 1] (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "vector-form let bindings must populate the ctx: {:?}",
            w
        );
    }

    #[test]
    fn let_star_behaves_like_let_for_typing() {
        // `let*` shares the sequential-binding semantics; the checker handles
        // both via the same path. Verify it still tracks types.
        let w = warnings("(let* (x 1) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let* must populate the ctx like let: {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_lets_a_then_branch_flag_a_misuse() {
        // In the then-branch of `(if (int? x) …)`, x : int — `(first x)` flags.
        let w = warnings("(if (int? x) (first x) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected guard narrowing to flag (first x) when x : int, got {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_does_not_leak_into_the_else_branch() {
        // The else-branch narrows x to `not int`, which overlaps list / vector;
        // so `(first x)` must NOT warn there.
        let w = warnings("(if (int? x) nil (first x))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "else branch must not have x narrowed to int: {:?}",
            w
        );
    }

    #[test]
    fn negated_guard_flips_the_narrowing() {
        // (if (not (int? x)) …) — the then-branch narrows x to `not int`, the
        // else-branch to int.
        let w = warnings("(if (not (int? x)) nil (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "the else of a negated guard must narrow to the inner type: {:?}",
            w
        );
    }

    #[test]
    fn guards_for_number_and_list_unions_narrow_to_the_union() {
        // (if (number? x) (first x) …) — x : number = int|float in the then,
        // which is disjoint from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (first x) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("number")),
            "number? must narrow to int|float: {:?}",
            w
        );
        // The list? guard should *not* warn in the then (list overlaps first's
        // expected type).
        let w = warnings("(if (list? x) (first x) nil)");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "list? must not produce a false positive on (first x): {:?}",
            w
        );
    }

    #[test]
    fn non_guard_tests_dont_narrow() {
        // The test isn't a recognised type predicate, so x stays unknown in
        // both branches — `(first x)` must not warn.
        let w = warnings("(if (zero? x) (first x) (first x))");
        assert!(w.is_empty(), "non-tag-guard test must not narrow: {:?}", w);
    }

    #[test]
    fn nested_guards_compose_their_narrowings() {
        // (if (number? x) (if (int? x) … (first x)) …) — in the inner else,
        // x is narrowed to `number ∩ ¬int` = float, which is still disjoint
        // from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (if (int? x) nil (first x)) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("float")),
            "nested guards must compose to float (= number ∩ ¬int): {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_when_used_as_an_if_test() {
        // The user-written shape `(let (cond (int? x)) (if cond …))` — Brood is
        // immutable, so `cond` faithfully reflects `(int? x)` until the let
        // ends. The guard-alias table maps `cond → (x, int)`, and the inner
        // `if cond` narrows x to int in the then-branch.
        let w = warnings("(let (cond (int? x)) (if cond (first x) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected let-bound guard to flag (first x) in the then: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_in_the_else_branch_too() {
        // Else-branch sees x as `not int`, which overlaps list / vector, so
        // no warning — same as the direct-test case.
        let w = warnings("(let (cond (int? x)) (if cond nil (first x)))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "the else of a let-bound guard must narrow to ¬int, not int: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_can_be_negated_in_the_if() {
        // `(if (not cond) …)` flips the narrowing — same as `(not (int? x))`.
        let w = warnings("(let (cond (int? x)) (if (not cond) nil (first x)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected negation to flip the let-bound guard: {:?}",
            w
        );
    }

    #[test]
    fn rebinding_the_guard_name_clears_the_alias() {
        // After `(let (cond <unknown>) …)` shadowing, `cond` no longer aliases
        // the int-guard, so `(if cond …)` must not narrow x.
        let w = warnings(
            "(let (cond (int? x)) (let (cond foo) (if cond (first x) nil)))",
        );
        assert!(
            w.is_empty(),
            "shadowing must drop the guard alias: {:?}",
            w
        );
    }

    #[test]
    fn rebinding_to_a_non_guard_value_clears_the_alias() {
        // Same as above but with an int literal rather than an unknown var.
        let w = warnings(
            "(let (cond (int? x)) (let (cond 1) (if cond (first x) nil)))",
        );
        assert!(
            w.is_empty(),
            "shadowing with a non-guard value must drop the alias: {:?}",
            w
        );
    }

    #[test]
    fn self_aliased_guard_is_not_recorded() {
        // `(let (x (int? x)) …)` shadows the outer x with a bool; the inner
        // body's `x` is the bool, not the original — narrowing the original
        // would be unsound (it's no longer reachable), so we must not record
        // the guard. (No assertion about a warning either way — the point is
        // we don't crash and don't introduce a stale alias.)
        let w = warnings("(let (x (int? x)) (if x x nil))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "self-aliased guards must not propagate to inner uses: {:?}",
            w
        );
    }

    #[test]
    fn let_inside_a_then_branch_can_shadow_a_narrowing() {
        // Outer narrowing: x : int. Inner shadow: x : string. The body now
        // sees x as string, so the narrowing message names string.
        let w = warnings("(if (int? x) (let (x \"hi\") (first x)) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("string")),
            "shadow must override the guard narrowing: {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "the int narrowing must not leak through the shadow: {:?}",
            w
        );
    }

    // ---------------- Step 4: arity + unbound-symbol diagnostics ----------------

    #[test]
    fn flags_too_few_arguments() {
        // `first` expects exactly 1; 0 is wrong.
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1") && w.contains("got 0")));
        // `string-length` expects exactly 1.
        assert!(warnings("(string-length)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("expected 1")));
    }

    #[test]
    fn flags_too_many_arguments() {
        // `rem` is `exact(2)`; calling with 3 is wrong.
        assert!(warnings("(rem 1 2 3)")
            .iter()
            .any(|w| w.contains("rem") && w.contains("expected 2") && w.contains("got 3")));
    }

    #[test]
    fn arity_message_handles_range_and_variadic() {
        // `map-get` is `range(2, 3)` → "expected 2 to 3".
        assert!(warnings("(map-get {})")
            .iter()
            .any(|w| w.contains("map-get") && w.contains("2 to 3")));
        // `apply` is `at_least(2)` → "expected 2 or more"; 1 is too few.
        assert!(warnings("(apply f)")
            .iter()
            .any(|w| w.contains("apply") && w.contains("2 or more")));
    }

    #[test]
    fn arity_pass_is_silent_for_correct_calls() {
        assert!(warnings("(first [1 2])")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        assert!(warnings("(rem 7 3)")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        // Variadic: any count is fine.
        for n in 0..=5 {
            let args = (0..n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            let w = warnings(&format!("(+ {})", args));
            assert!(
                w.iter().all(|s| !s.contains("number of arguments")),
                "(+ {}…) should not warn arity: {:?}",
                n,
                w
            );
        }
    }

    #[test]
    fn flags_unbound_call_heads() {
        assert!(warnings("(frobnicate 1)")
            .iter()
            .any(|w| w.contains("unbound symbol: frobnicate")));
        assert!(warnings("(typo-name :hi)")
            .iter()
            .any(|w| w.contains("unbound symbol: typo-name")));
    }

    #[test]
    fn unbound_is_silent_for_in_scope_names() {
        // fn/lambda params don't look unbound when used as call heads or
        // referenced in the body.
        assert!(warnings("(fn (f) (f 1 2))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // let bindings: same.
        assert!(warnings("(let (g (fn (x) x)) (g 1))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // Syntactic keywords aren't bound but are never "unbound".
        for src in &["(do 1 2 3)", "(when true 1)", "(cond)", "(and)", "(or)"] {
            assert!(
                warnings(src).iter().all(|w| !w.contains("unbound")),
                "syntactic keyword must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn unbound_is_silent_for_prelude_names() {
        // The prelude is loaded in our test heap (via Interp::new()), so
        // stdlib names resolve. `inc`, `list`, `int?`, `even?`, … are all fine.
        for src in &[
            "(inc 1)",
            "(list 1 2 3)",
            "(int? 5)",
            "(zero? 0)",
            "(map (fn (x) x) [1 2 3])",
        ] {
            assert!(
                warnings(src)
                    .iter()
                    .all(|w| !w.contains("unbound")),
                "prelude name must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn file_globals_make_later_forms_see_earlier_defs() {
        // `check_file` accumulates top-level def names. Without that,
        // `(my-fn 1)` in form 2 would be flagged unbound — `my-fn` isn't in
        // the heap (no eval), only in the file.
        let interp = crate::Interp::new();
        let src = "(defn my-fn (x) (+ x 1))\n(my-fn 1)";
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
        let out = check_file(&mut heap, &forms);
        let msgs: Vec<_> = out.into_iter().map(|(_, m)| m).collect();
        assert!(
            msgs.iter().all(|m| !m.contains("unbound symbol: my-fn")),
            "file-local defns must shield later calls: {:?}",
            msgs
        );
    }

    #[test]
    fn fn_params_with_rest_and_optional_dont_leak() {
        // The marker symbols `&`/`&optional` themselves are *not* binders;
        // the names that follow them are.
        assert!(warnings("(fn (x & ys) (cons x ys))")
            .iter()
            .all(|w| !w.contains("unbound")));
        assert!(warnings("(fn (x &optional (y 0)) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn defn_body_sees_its_params_in_scope() {
        // A user defn whose body references its params must not flag them as
        // unbound. (The `defn` macro hasn't been expanded — the CLI checks
        // un-expanded forms — so this tests the un-expanded surface path.)
        assert!(warnings("(defn my-fn (x y) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn arity_check_works_for_user_defns_in_a_real_interp() {
        // Once a defn is evaluated, its arity is derivable from its Closure.
        // `inc` (prelude) is `(defn inc (n) …)` → exact(1).
        let w = check_with_defs(&[], "(inc 1 2)");
        assert!(
            w.iter()
                .any(|s| s.contains("inc") && s.contains("expected 1")),
            "user defn arity should be enforced: {:?}",
            w
        );
    }

    // ---- Step 4 final pieces: %eq-as-guard + let-alias propagation --------
    //
    // `match` lowers `(match x (5 body) …)` to
    // `(let (m__N x) (if (%eq m__N 5) (do body) …))`. To flag a misuse on
    // `x` in `body` (where the literal pattern asserts x's type), the checker
    // needs two pieces: (1) recognise `(%eq sym lit)` as a guard asserting
    // `sym : type-of(lit)`; (2) when a `let` binds a name to another symbol,
    // propagate narrowings between the two via the alias chain.

    #[test]
    fn match_literal_pattern_narrows_the_scrutinee() {
        // `(match x (5 (first x)))` — the literal-int pattern asserts x : int;
        // `(first x)` in the body must then flag. Goes through macroexpansion
        // because `match` is a `defmacro` whose pattern compiler lowers to
        // `let`+`if`+`%eq`; the checker's narrowing rides the lowered shape.
        let w = warnings_expanded("(match x (5 (first x)) (_ nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "match int-literal pattern should narrow x: {:?}",
            w
        );
    }

    #[test]
    fn match_keyword_pattern_narrows_the_scrutinee() {
        // Mirror of the int case for a keyword literal.
        let w = warnings_expanded("(match x (:foo (first x)) (_ nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("keyword")),
            "match keyword-literal pattern should narrow x: {:?}",
            w
        );
    }

    #[test]
    fn eq_against_a_literal_is_a_guard() {
        // The mechanism that powers match: `(%eq m 5)` in a test position
        // narrows `m` to `:int` in the then-branch. (Symmetric — both
        // `(%eq m 5)` and `(%eq 5 m)` should narrow.)
        let w = warnings("(if (%eq m 5) (first m) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "%eq with sym + literal should narrow: {:?}",
            w
        );
        let w = warnings("(if (%eq 5 m) (first m) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "%eq with literal + sym (reversed) should narrow: {:?}",
            w
        );
    }

    #[test]
    fn eq_between_two_variables_is_not_a_guard() {
        // Equality between two unknowns asserts nothing about either's type.
        // No false positive must fire on the body.
        let w = warnings("(if (%eq a b) (first a) nil)");
        assert!(
            w.iter().all(|s| !s.contains("first")),
            "%eq between two vars should not narrow: {:?}",
            w
        );
    }

    #[test]
    fn let_alias_propagates_narrowing_in_both_directions() {
        // The match pattern compiler's exact shape: alias `m` to `x`, then
        // narrow `m` via a guard. The narrowing must flow back onto `x` so a
        // body that uses `x` (not `m`) still sees the asserted type.
        let w = warnings("(let (m x) (if (int? m) (first x) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let-alias should propagate narrowing from m to x: {:?}",
            w
        );
        // And the symmetric direction: narrow x, alias-narrows m.
        let w = warnings("(let (m x) (if (int? x) (first m) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let-alias should propagate narrowing from x to m: {:?}",
            w
        );
    }

    #[test]
    fn shadowing_clears_an_alias() {
        // An inner let that rebinds an aliased name to something else breaks
        // the chain — the new binding is the new name's type, no alias.
        // `(let (m x) (let (m 5) (first m)))` flags the inner `(first m)`
        // because `m` is now int, but that's via the literal-type binding,
        // not the broken alias.
        let w = warnings("(let (m x) (let (m 5) (first m)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "shadowed let should still warn on the inner int: {:?}",
            w
        );
        // The outer `x` must not be narrowed by the inner shadowing.
        let w = warnings("(let (m x) (let (m 5) (println x)))");
        assert!(
            w.iter().all(|s| !s.contains("first")),
            "shadowing must not leak narrowing back to the original: {:?}",
            w
        );
    }
}
