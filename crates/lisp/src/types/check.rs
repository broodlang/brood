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
//! ## Where signatures come from (Step 3)
//!
//! Three sources, simplest-first — *no inference engine* (`docs/types.md`):
//!
//! 1. **Primitives** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    a `Sig` ([contract point #6, enforced](../docs/types.md#compatibility-contract))
//!    so the checker just reads it from the global env (see [`primitive_sig`]).
//!    There is no parallel table to maintain.
//! 2. **Curated stdlib** — a small hand-vetted table for the variadic /
//!    `reduce`-based / higher-order Brood closures the checker can't infer but
//!    that matter (`+ - * / < <= > >= mod map filter reduce`; see
//!    [`curated_sig`]). Each is a Brood `defn`, but its sig is pinned by hand.
//! 3. **Basic inference** for a closure whose body is **one straight-line
//!    expression** (a single direct call to a known sig; no `if`/`cond`/`let`/
//!    `match`/recursion). Each closure parameter inherits the type the callee
//!    expects at the position(s) where the parameter is passed; the closure's
//!    return is the callee's. Sound because a straight-line use is
//!    unconditional — no control-flow analysis (see [`infer_sig`]).
//!
//! Argument types in a call come from literals and from a nested call's result
//! type; a variable is unknown (`None`) and never flagged.
//!
//! Vocabulary is `Option<Ty>` (known / unknown), not `GradualTy` — the
//! disjointness check only needs "do I know this type?". Forms inside `try` /
//! `error-of` / `assert-error` are skipped (they deliberately exercise failures).
//!
//! Not yet (later increments): inference through branches / guards / recursion
//! / higher-order; guard narrowing (`Ty::tested_by` is prepped); running
//! automatically in `brood <file>` / `nest test`. Today the entry point is
//! `brood --check` (CLI) and the `(check 'form)` builtin.

use crate::core::heap::Heap;
use crate::core::value::{self, Tag, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

/// The signature of a **primitive** named `name` — read from its `NativeFn`
/// (contract point #6, enforced). `None` when no global of that name exists,
/// or when it isn't a primitive (a Brood closure goes through [`curated_sig`]
/// or [`infer_sig`] instead).
///
/// Lookup goes through `heap.global()`, not `EnvId::GLOBAL` directly: in a real
/// runtime that's `EnvId::GLOBAL` (routed to the shared `runtime.globals`
/// table), but in the prelude-builder / test heap it's a *local* env that
/// `builtins::register` populated — `env_get` walks both transparently.
fn primitive_sig(heap: &Heap, name: &str) -> Option<Sig> {
    let sym = value::intern_existing(name)?;
    match heap.env_get(heap.global(), sym)? {
        Value::Native(id) => Some(heap.native(id).sig.clone()),
        _ => None,
    }
}

/// Signatures for the stable stdlib **closures** the checker can't infer but
/// that matter: the arithmetic/comparison kernel (variadic over numbers) and the
/// core higher-order fns. Hand-vetted, so sound — this is what makes `(+ 1 "x")`
/// catchable even though `+` is `(reduce %add 0 xs)`.
fn curated_sig(name: &str) -> Option<Sig> {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let any = Ty::ANY;
    let seq = Ty::LIST.union(Ty::of(Tag::Vector));
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));
    Some(match name {
        // variadic arithmetic: every argument must be a number
        "+" | "-" | "*" | "/" => Sig::variadic(num, num),
        // variadic comparison: numeric args, boolean result
        "<" | "<=" | ">" | ">=" => Sig::variadic(num, Ty::of(Tag::Bool)),
        // `mod` is Brood (over `rem`), but its types are fixed
        "mod" => Sig::new(vec![int, int], int),
        // higher-order: first arg callable, second a sequence
        "map" | "filter" => Sig::new(vec![callable, seq], seq),
        "reduce" => Sig::new(vec![callable, any, seq], any),
        _ => return None,
    })
}

/// Inferred signature for a **user closure** named `sym` whose body is one
/// straight-line expression — a single call to a callee with a known
/// primitive/curated sig. Each closure parameter inherits the type the callee
/// expects at the position(s) where the parameter is passed directly; the
/// closure's return is the callee's.
///
/// Deliberately *narrow*. Skipped when:
/// - the body isn't exactly one expression (branches, lets, multi-step bodies);
/// - the closure takes `&optional` / rest params (the call's positional arity is
///   already past where the simple rule pays off);
/// - the body isn't a call (a lone literal/variable doesn't pay for itself);
/// - the head is anything but a primitive or curated stdlib closure (in
///   particular, the closure's own name → recursion is ignored, per the rule).
///
/// Sound because a straight-line use is unconditional — no false positives.
fn infer_sig(heap: &Heap, name: &str) -> Option<Sig> {
    let sym = value::intern_existing(name)?;
    let Value::Fn(cid) = heap.env_get(heap.global(), sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    if closure.body.len() != 1 || !closure.optionals.is_empty() || closure.rest.is_some() {
        return None;
    }
    let body = closure.body[0];
    // Copy out before we ask sig_of (which borrows the heap again).
    let params: Vec<value::Symbol> = closure.params.clone();
    let self_name = closure.name;

    let items = list_items(heap, body)?;
    let Value::Sym(callee) = items.first().copied()? else {
        return None;
    };
    // No recursion — neither direct (the closure calls itself by name) nor
    // through inference (`sig_of` is the *non-inferring* lookup so a chain
    // like `defn a (x) (b x)` / `defn b (x) (a x)` can't loop).
    if self_name == Some(callee) {
        return None;
    }
    let callee_name = value::symbol_name(callee);
    let callee_sig = primitive_sig(heap, &callee_name).or_else(|| curated_sig(&callee_name))?;

    // Each closure parameter takes the type the callee expects where the
    // parameter is used. Multiple positions → intersect (the param must satisfy
    // every use). Unmentioned parameters stay `ANY`.
    let mut param_tys = vec![Ty::ANY; params.len()];
    for (i, &arg) in items[1..].iter().enumerate() {
        let Value::Sym(arg_sym) = arg else { continue };
        let Some(pos) = params.iter().position(|&p| p == arg_sym) else {
            continue;
        };
        let Some(expected) = callee_sig.param(i) else {
            continue;
        };
        param_tys[pos] = param_tys[pos].intersect(expected);
    }
    Some(Sig::new(param_tys, callee_sig.ret))
}

/// The signature for `name`, from any of the three sources (primitive → curated
/// → inferred). The non-inferring half is exposed as [`primitive_sig`] +
/// [`curated_sig`] so [`infer_sig`] can consult the callee's sig *without*
/// kicking off another inference (the rule says inference is one step deep).
fn sig_of(heap: &Heap, name: &str) -> Option<Sig> {
    primitive_sig(heap, name)
        .or_else(|| curated_sig(name))
        .or_else(|| infer_sig(heap, name))
}

/// The static type of an expression form, or `None` when it can't be pinned (a
/// variable, or a call whose callee has no known signature). `None` is "unknown"
/// and is never flagged. Self-evaluating literals get their exact tag; a `quote`d
/// datum gets the datum's tag; a call with a known signature gets its result type.
fn expr_ty(heap: &Heap, form: Value) -> Option<Ty> {
    match form {
        // A bare symbol in code is a variable reference — unknown.
        Value::Sym(_) => None,
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            match items.first().copied() {
                Some(Value::Sym(s)) => {
                    let head = value::symbol_name(s);
                    if head == "quote" {
                        return items.get(1).map(|&d| Ty::of_value(d));
                    }
                    sig_of(heap, &head).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // Int / Float / Str / Keyword / Bool / Nil / Vector: self-evaluating.
        other => Some(Ty::of_value(other)),
    }
}

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
    check_into(heap, form, &mut out);
    out
}

/// Forms whose contents are data (`quote`/`quasiquote`) or deliberately exercise
/// failures (`try` and the error-asserting test helpers it expands from) — don't
/// look inside them.
fn skips_body(name: &str) -> bool {
    matches!(
        name,
        "quote" | "quasiquote" | "try" | "error-of" | "assert-error"
    )
}

fn check_into(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

    if let Value::Sym(s) = head {
        let name = value::symbol_name(s);
        if skips_body(&name) {
            return;
        }
        if let Some(sig) = sig_of(heap, &name) {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                if let Some(arg_ty) = expr_ty(heap, arg) {
                    if arg_ty.is_disjoint(param) {
                        let msg = format!(
                            "{}: argument {} expects {}, got {} ({})",
                            name,
                            i + 1,
                            param,
                            arg_ty,
                            crate::syntax::printer::print(heap, arg),
                        );
                        // Locate to the call form (a Pair the reader positioned).
                        out.push((heap.form_pos(form), msg));
                    }
                }
            }
        }
    }

    // Recurse: arguments (and any nested forms) may themselves be calls.
    for &item in &items {
        check_into(heap, item, out);
    }
}

/// The elements of a proper list, or `None` for an improper list / non-list.
fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    loop {
        match v {
            Value::Nil => return Some(out),
            Value::Pair(p) => {
                let (head, tail) = heap.pair(p);
                out.push(head);
                v = tail;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::reader;

    /// A fresh builder heap with the primitive kernel registered (the source
    /// the checker reads sigs from) and a local root env serving as the heap's
    /// "global". Mirrors the prelude builder in `lib.rs`, minus the prelude
    /// itself — curated stdlib closures live in `curated_sig`, so we don't
    /// need to eval them to check their callers.
    fn heap_with_primitives() -> Heap {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        heap.set_global(root);
        crate::builtins::register(&mut heap, root);
        heap
    }

    fn warnings(src: &str) -> Vec<String> {
        let mut heap = heap_with_primitives();
        let form = reader::read_one(&mut heap, src).expect("parse");
        check_form(&heap, form)
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
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol
        assert!(warnings("(first)").is_empty()); // missing arg → nothing to check
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
        let heap = heap_with_primitives();
        let sig = primitive_sig(&heap, "string-length").expect("string-length is a primitive");
        assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
        assert_eq!(sig.ret, Ty::of(Tag::Int));
        // The "no useful info" lane: a variadic any-arg primitive (str) returns
        // a Sig that param-overlaps every input, so it never warns.
        let any_sig = primitive_sig(&heap, "str").expect("str is a primitive");
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
        for src in &[
            "(defn maybe (x) (if (int? x) (+ x 1) x))",
            "(defn shadow (x) (let (y x) (+ y 1)))",
        ] {
            let w = check_with_defs(&[src], "(maybe :k)");
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
}
