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
//! Not yet (later increments): inference through `cond`/`match`, structured /
//! `and`/`or`-chained guards, recursion, higher-order; unbound-symbol and
//! arity diagnostics; running automatically in `brood <file>` / `nest test`.
//! Today the entry point is `brood --check` (CLI) and the `(check 'form)`
//! builtin.

use std::collections::HashMap;

use crate::core::heap::Heap;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

/// Locally-known types for variables in scope — populated by `let`/`let*`
/// bindings and by an enclosing `if`'s guard. Globals are never tracked here
/// (they're redefinable under hot reload — `dynamic()`, not `Any`).
///
/// `Ty::ANY` and "absent" both mean "no useful info"; we keep absent variables
/// out of the map so the printer in tests stays uncluttered.
///
/// **Guard aliases.** When a `let` binds a name to a recognised guard call —
/// `(let (cond (int? x)) (if cond …))` — we also remember that the bound name
/// *is* the result of testing that variable, so the inner `if cond` can
/// narrow `x` (not the bool `cond` itself). The aliasing is sound because
/// Brood is immutable: between the let and the if, neither `x` nor `cond` can
/// change, so the assertion the guard recorded still applies.
#[derive(Clone, Default)]
struct Ctx {
    types: HashMap<Symbol, Ty>,
    /// `bound-name → (variable, type-it-asserts)`: a `let`-stored guard result.
    guards: HashMap<Symbol, (Symbol, Ty)>,
}

impl Ctx {
    /// The locally-known type for `sym`, or `None` if it isn't tracked.
    fn get(&self, sym: Symbol) -> Option<Ty> {
        self.types.get(&sym).copied()
    }
    /// The guard (variable + asserted type) `sym` was bound to, if any.
    fn guard(&self, sym: Symbol) -> Option<(Symbol, Ty)> {
        self.guards.get(&sym).copied()
    }
    /// **Narrow** `sym` to the intersection with `ty` (a guard refinement —
    /// the same lexical variable in the same scope getting tighter). The
    /// caller already knows `sym` lives in this scope (e.g. it's a free
    /// variable inside an `if`'s branch); for an unknown one we treat the
    /// prior as `ANY`, so the intersection is just `ty`.
    fn narrow(&self, sym: Symbol, ty: Ty) -> Ctx {
        let mut c = self.clone();
        let prior = c.types.get(&sym).copied().unwrap_or(Ty::ANY);
        c.types.insert(sym, prior.intersect(ty));
        c
    }
    /// **Bind** `sym` to `ty`, overwriting any prior entry — a fresh let-bound
    /// or fn-param variable shadows the outer. `None` clears the entry so a
    /// shadowing binding of unknown type doesn't keep an outer narrowing.
    /// Always clears any guard-alias entry for `sym` (a fresh binding doesn't
    /// inherit one).
    fn bind(&self, sym: Symbol, ty: Option<Ty>) -> Ctx {
        let mut c = self.clone();
        match ty {
            Some(t) => {
                c.types.insert(sym, t);
            }
            None => {
                c.types.remove(&sym);
            }
        }
        c.guards.remove(&sym);
        c
    }
    /// Record that `sym` was let-bound to the result of testing `target` for
    /// `ty` — so a later `(if sym then else)` narrows `target` accordingly.
    /// Self-aliasing (`(let (x (int? x)) …)` would shadow the outer `x` the
    /// guard means to narrow) is rejected.
    fn add_guard(&self, sym: Symbol, target: Symbol, ty: Ty) -> Ctx {
        if sym == target {
            return self.clone();
        }
        let mut c = self.clone();
        c.guards.insert(sym, (target, ty));
        c
    }
}

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

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    match form {
        // A bare symbol is a variable reference — looked up in the local ctx
        // (let-bound RHS / if-guard narrowing). A miss = unknown, not flagged.
        Value::Sym(s) => ctx.get(s),
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
    check_into(heap, form, &Ctx::default(), &mut out);
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

/// If `test` is a recognisable type guard over a single variable, return the
/// `(sym, asserted_type)` pair — the type `sym` provably has when `test` is
/// truthy. A leading `(not …)` flips the assertion via [`Ty::negate`]. A bare
/// `Sym` is looked up in `ctx`'s guard-alias table (a `let`-stored guard
/// result — `(let (cond (int? x)) (if cond …))`). `None` for any test that
/// isn't a pure pattern-matchable guard (so we never narrow on something we
/// can't soundly invert in the else-branch).
fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<(Symbol, Ty)> {
    if let Value::Sym(s) = test {
        return ctx.guard(s);
    }
    let items = list_items(heap, test)?;
    if items.len() != 2 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // (not <inner>) — invert the inner assertion; everything else proceeds.
    if head_name == "not" {
        let (sym, ty) = guard_assertion(heap, items[1], ctx)?;
        return Some((sym, ty.negate()));
    }
    let ty = Ty::tested_by(&head_name)?;
    match items[1] {
        Value::Sym(s) => Some((s, ty)),
        _ => None,
    }
}

fn check_into(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

    // Special-cased forms that introduce scope or refine types. Each handles
    // its own argument-walking and returns; the generic path below doesn't run.
    if let Value::Sym(s) = head {
        let name = value::symbol_name(s);
        if skips_body(&name) {
            return;
        }
        match name.as_str() {
            "if" => {
                check_if(heap, &items, ctx, out);
                return;
            }
            "let" | "let*" => {
                check_let(heap, &items, ctx, out);
                return;
            }
            _ => {}
        }
        if let Some(sig) = sig_of(heap, &name) {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                if let Some(arg_ty) = expr_ty(heap, arg, ctx) {
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
        check_into(heap, item, ctx, out);
    }
}

/// `(if test then else?)` — check the test in the outer ctx, then descend
/// into each branch with the ctx narrowed by what the test would assert.
/// `else` defaults to `nil` (matches the evaluator), so absent or non-pair
/// branches simply contribute no warnings.
fn check_if(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let test = items.get(1).copied().unwrap_or(Value::Nil);
    let then_form = items.get(2).copied().unwrap_or(Value::Nil);
    let else_form = items.get(3).copied().unwrap_or(Value::Nil);

    check_into(heap, test, ctx, out);

    let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
        Some((sym, ty)) => (ctx.narrow(sym, ty), ctx.narrow(sym, ty.negate())),
        None => (ctx.clone(), ctx.clone()),
    };
    check_into(heap, then_form, &then_ctx, out);
    check_into(heap, else_form, &else_ctx, out);
}

/// `(let bindings body…)` — walk the bindings sequentially (matching how the
/// evaluator binds), checking each RHS in the in-flight ctx and shadowing the
/// new name into it. Then check the body in the extended ctx.
///
/// Quietly skips a malformed bindings shape (a pattern-target `let`, an
/// improper list, an odd number of binding items): those are evaluator-level
/// errors and aren't this checker's job.
fn check_let(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&binds_form) = items.get(1) else {
        return;
    };
    let Some(binds) = bindings(heap, binds_form) else {
        // Unknown shape — just recurse generically so we still check nested calls.
        for &item in items {
            check_into(heap, item, ctx, out);
        }
        return;
    };
    if binds.len() % 2 != 0 {
        return;
    }
    let mut scope = ctx.clone();
    let mut i = 0;
    while i < binds.len() {
        let Value::Sym(name) = binds[i] else {
            // Pattern-target binding (post-Step 4 work) — skip narrowing for it
            // but still check the RHS as an expression.
            check_into(heap, binds[i + 1], &scope, out);
            i += 2;
            continue;
        };
        let rhs = binds[i + 1];
        check_into(heap, rhs, &scope, out);
        let rhs_ty = expr_ty(heap, rhs, &scope);
        let rhs_guard = guard_assertion(heap, rhs, &scope);
        scope = scope.bind(name, rhs_ty);
        if let Some((target, gty)) = rhs_guard {
            scope = scope.add_guard(name, target, gty);
        }
        i += 2;
    }
    for &body_form in &items[2..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// Parse a `let` bindings form — accepts both `(name val name val …)` lists
/// and `[name val name val …]` vectors, the two shapes the reader emits.
fn bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
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
}
