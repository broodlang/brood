//! Predicates over forms that the walker dispatches on:
//!
//! - [`is_syntactic_keyword`] — which heads are *not* callables, so an
//!   "unbound symbol" warning doesn't fire on them. (The "don't descend
//!   into this body" predicate that used to live here is now folded into
//!   `walk::SPECIAL_HEAD` so the dispatch is one `SymbolMap` probe.)
//! - [`guard_assertion`] / [`literal_eq_guard`] — pull a `(sym, type)` pair
//!   out of an `if`-test when it's a recognised guard, so the walk can
//!   narrow the variable in each branch.
//! - [`expr_ty`] — the static type of a form `in ctx`, the single
//!   "do I know what this expression returns?" probe the misuse-check
//!   reads off.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::Ty;

use super::ctx::Ctx;
use super::sigs::sig_of;
use super::walk::list_items;

/// Names that have *syntactic* meaning but aren't bound values — never flag
/// these as unbound. Mirrors `eval::SPECIAL_NAMES` plus the macros that the
/// reader / un-expanded forms may carry (the CLI's `--check` doesn't
/// macroexpand). `catch` is the carrier-form for `try`'s catcher, not a
/// callable; `&` / `&optional` are parameter-list markers.
pub(super) fn is_syntactic_keyword(name: &str) -> bool {
    matches!(
        name,
        kw::QUOTE
            | kw::QUASIQUOTE
            | kw::UNQUOTE
            | kw::UNQUOTE_SPLICING
            | kw::IF
            | kw::DO
            | kw::DEF
            | kw::FN
            | kw::LET
            | kw::LETREC
            | kw::DEFMACRO
            | kw::DEFN
            | kw::DEFDYN
            | kw::DEFMODULE
            | kw::MODULE_DOC
            | kw::WHEN
            | kw::UNLESS
            | kw::COND
            | kw::AND
            | kw::OR
            | kw::THREAD_FIRST
            | kw::THREAD_LAST
            | kw::MATCH
            | kw::CASE
            | kw::TRY
            | kw::CATCH
            | kw::THROW
            | kw::BINDING
            | kw::FOR
            | kw::SPAWN
            | kw::AMP
            | kw::AMP_OPTIONAL
            | kw::AMP_REST
    )
}

// `skips_body` used to live here; it's now folded into the
// `SpecialHead::SkipBody` arm of `walk::SPECIAL_HEAD` (one `SymbolMap` probe
// shared with the special-form dispatch, no per-call string allocation).
// Names that route through that arm: `quote`, `quasiquote`, `try`,
// `error-of`, `assert-error`, `%try`. `%try` matters post-expansion: the
// macroexpand pass rewrites `(try …)` to `(%try (fn () body) (fn (e) handler))`
// before `check_file` walks the tree, and without `%try` in that arm the walk
// would descend into the "I expect this to fail" body and flag every
// `(error-of (cons 1))` in the test suite.

/// A recognised type guard over a single variable: when `test` is truthy, `sym`
/// provably has type `ty`. `then_only` marks a guard whose *negation is unsound*
/// — a falsy `test` does **not** establish `¬ty`, so the else-branch must not be
/// narrowed (the `and` short-circuit is the case: a falsy `and` may have failed
/// on a *later* conjunct, so the first conjunct can still hold). An ordinary type
/// predicate is biconditional (`then_only = false`): the else-branch narrows to
/// `¬ty` soundly.
pub(super) struct Guard {
    pub(super) sym: Symbol,
    pub(super) ty: Ty,
    pub(super) then_only: bool,
}

/// If `test` is a recognisable type guard over a single variable, return the
/// [`Guard`] it implies. A leading `(not …)` flips the assertion via
/// [`Ty::negate`]. A bare `Sym` is looked up in `ctx`'s guard-alias table (a
/// `let`-stored guard result — `(let (cond (int? x)) (if cond …))`). `None` for
/// any test that isn't a pure single-variable guard.
pub(super) fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<Guard> {
    if let Value::Sym(s) = test {
        // A let-stored guard alias — recorded only for biconditional guards
        // (see `check_let`), so it narrows the else-branch too.
        let (sym, ty) = ctx.guard(s)?;
        return Some(Guard {
            sym,
            ty,
            then_only: false,
        });
    }
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // (not <inner>) — invert the inner assertion. Only invertible when `inner`
    // is itself biconditional; a `then_only` inner can't be soundly negated
    // (we'd be reasoning from `inner` being false), so we decline.
    if items.len() == 2 && head_name == kw::NOT {
        let inner = guard_assertion(heap, items[1], ctx)?;
        if inner.then_only {
            return None;
        }
        return Some(Guard {
            sym: inner.sym,
            ty: inner.ty.negate(),
            then_only: false,
        });
    }
    // `(%eq sym literal)` / `(%eq literal sym)` — equality against a literal
    // asserts the variable has the literal's runtime tag. The `match` pattern
    // compiler emits this for literal patterns (e.g. `(match x (5 …))`
    // lowers through `(let (m x) (if (%eq m 5) …))` — and the let-alias
    // machinery threads the narrowing back to `x`). Variadic `=` reaches us
    // pre-expanded as `%eq` calls when arities are 2, so we only need to
    // recognise the primitive shape.
    if items.len() == 3 && head_name == kw::EQ_PRIM {
        if let Some((sym, ty)) = literal_eq_guard(items[1], items[2])
            .or_else(|| literal_eq_guard(items[2], items[1]))
        {
            // **`then_only`:** `(%eq m lit)` being true proves `m` has `lit`'s
            // tag, but being *false* proves nothing about the tag — `m ≠ "x"`
            // can still be another string. So the else-branch must NOT narrow to
            // `¬ty` (that flagged a valid `(string-length m)` after `(= m "x")`).
            // (`nil` is the one tag where `≠ nil` *would* imply `¬nil`, but we
            // don't special-case it — dropping that narrowing only loses
            // precision, never soundness.)
            return Some(Guard {
                sym,
                ty,
                then_only: true,
            });
        }
        return None;
    }
    // The `and` short-circuit expansion `(let (g E) (if g _ g))` — a truthy
    // `and` implies its first conjunct `E` holds, so an `(if (and (pred? x) …) …)`
    // narrows `x` in the *then* branch. Matched post-`macroexpand_all` (when the
    // `(and …)` surface is already this shape); the `or` expansion
    // `(if g g _)` is deliberately *not* matched (a truthy `or` implies nothing
    // about its first operand). This is what lets the `match` compiler's
    // `(if (and (vector? m) (= (vector-length m) 2)) …)` narrow `m` to a vector,
    // so the guarded `vector-ref m i` isn't flagged against a list/other scrutinee.
    // **`then_only`:** a falsy `and` may have failed on a later conjunct, so the
    // else-branch must NOT be narrowed to `¬E` (that was a real false positive —
    // an else-branch `(vector-ref m i)` on a value that *is* a longer vector).
    if head_name == kw::LET && items.len() == 3 {
        if let Some(g) = and_first_conjunct_guard(heap, items[1], items[2], ctx) {
            return Some(g);
        }
    }
    if items.len() != 2 {
        return None;
    }
    let ty = Ty::tested_by(&head_name)?;
    match items[1] {
        Value::Sym(s) => Some(Guard {
            sym: s,
            ty,
            then_only: false,
        }),
        _ => None,
    }
}

/// Recognise the `and`-expansion `(let (g E) (if g _ g))` and return the guard
/// its first conjunct `E` asserts, marked `then_only` (the negation is unsound —
/// see [`Guard`]). The binding must be exactly one name `g`, and the body must be
/// `(if g <then> g)` — test and *else* both `g` (the `and` shape; `or` is
/// `(if g g <else>)` and must not match).
fn and_first_conjunct_guard(heap: &Heap, binding: Value, body: Value, ctx: &Ctx) -> Option<Guard> {
    let bs = list_items(heap, binding)?;
    if bs.len() != 2 {
        return None; // a multi-binding `let` isn't the `and` shape
    }
    let Value::Sym(g) = bs[0] else { return None };
    let cond = bs[1];
    let body_items = list_items(heap, body)?;
    // `(if test then else)` — 4 items; test == g and else == g.
    let is_if = matches!(body_items.first(), Some(&Value::Sym(s)) if value::symbol_is(s, kw::IF));
    if body_items.len() != 4 || !is_if {
        return None;
    }
    let is_g = |v: Value| matches!(v, Value::Sym(s) if s == g);
    if !is_g(body_items[1]) || !is_g(body_items[3]) {
        return None;
    }
    let inner = guard_assertion(heap, cond, ctx)?;
    Some(Guard {
        then_only: true, // a falsy `and` doesn't establish `¬E`
        ..inner
    })
}

/// If `a` is a symbol and `b` is a self-evaluating literal, return the guard
/// `(a, type-of(b))`. Used by `guard_assertion`'s `%eq` arm to recognise both
/// `(%eq sym lit)` and `(%eq lit sym)`. Returns `None` when `b` is itself a
/// variable — equality between two unknowns asserts nothing.
fn literal_eq_guard(a: Value, b: Value) -> Option<(Symbol, Ty)> {
    let Value::Sym(s) = a else { return None };
    // A literal is anything that's not a symbol / pair / vector / map.
    // Strings, ints, floats, keywords, booleans, nil all self-evaluate and
    // have a definite tag; pairs/vectors/maps are constructions whose pieces
    // could be unknown.
    match b {
        Value::Sym(_) | Value::Pair(_) | Value::Vector(_) | Value::Map(_) => None,
        other => Some((s, Ty::of_value(other))),
    }
}

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
pub(super) fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    match form {
        // A bare symbol is a variable reference — looked up in the local ctx
        // (let-bound RHS / if-guard narrowing). A miss = unknown, not flagged.
        Value::Sym(s) => ctx.get(s),
        // A vector literal `[a b c]` — its elements are evaluated, so the element
        // type is the union of their types (Step 5+, ADR-078). Any unknown element
        // → unrefined `vector`.
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            Some(match element_union(heap, &items, ctx) {
                Some(e) => Ty::vector_of(e),
                None => Ty::of(Tag::Vector),
            })
        }
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            match items.first().copied() {
                Some(Value::Sym(s)) => {
                    if value::symbol_is(s, kw::QUOTE) {
                        return items.get(1).map(|&d| Ty::of_value(d));
                    }
                    // A user `(sig …)` declaration is authoritative for the
                    // result type — consult it unless a *lexical* local (fn/let)
                    // shadows the name. (A file-global with a declared sig is the
                    // target, so guard on `is_lexical_local`, not `is_local`.)
                    if !ctx.is_lexical_local(s) {
                        if let Some(sg) = ctx.declared_sig(s) {
                            return Some(sg.ret);
                        }
                    }
                    // Sequence-aware refinements (`list`/`vector` constructors,
                    // `first`/`last`/`nth` extractors) when the head isn't a local
                    // shadow; else the callee's flat result type.
                    if !ctx.is_local(s) {
                        if let Some(t) = seq_aware_call_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                    }
                    sig_of(heap, s).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // Int / Float / Str / Keyword / Bool / Nil: self-evaluating.
        other => Some(Ty::of_value(other)),
    }
}

/// The union of the element forms' types, or `None` if empty or any element is
/// unknown (so the element type can't be pinned — stay unrefined, never wrong).
fn element_union(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for &it in items {
        let t = expr_ty(heap, it, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    acc
}

/// Element-aware result type for the sequence builtins — `None` falls through to
/// the callee's flat signature. `(list …)`/`(vector …)` build a refined
/// sequence; `(first xs)`/`(last xs)`/`(nth xs i)` extract the element type
/// (widened with `nil` for the empty / out-of-range case, so the result is a
/// sound superset). Only refines when the element type is actually known.
fn seq_aware_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if value::symbol_is(head, "list") {
        return element_union(heap, &items[1..], ctx).map(Ty::list_of);
    }
    if value::symbol_is(head, "vector") {
        return element_union(heap, &items[1..], ctx).map(Ty::vector_of);
    }
    if value::symbol_is(head, "first")
        || value::symbol_is(head, "last")
        || value::symbol_is(head, "nth")
    {
        let arg = *items.get(1)?;
        let elem = expr_ty(heap, arg, ctx)?.elem_ty().cloned()?;
        // first/last/nth yield `nil` on an empty / out-of-range sequence.
        return Some(elem.union(Ty::of(Tag::Nil)));
    }
    // `(filter pred coll)` keeps `coll`'s element type — the result is the items
    // that pass, so `nil | list<A>` for `A = elem(coll)` (ADR-078 parametric
    // results). `None` element → fall through to the flat curated `list`.
    if value::symbol_is(head, "filter") {
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty().cloned());
        return list_result(a);
    }
    // `(map f coll)` → `nil | list<B>`, `B` = the callback's return type applied
    // to `coll`'s element type. Unknown callback / element → flat `list`.
    if value::symbol_is(head, "map") {
        let f = *items.get(1)?;
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty().cloned());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result(b);
    }
    // `(reduce f init coll)` / `(fold f init coll)` reduce to an accumulator typed
    // `ty(init) | B`, where `B` is the 2-arg callback's return (`(f acc x)`). The
    // accumulator can grow across steps, so it's over-approximated as `any` for
    // the callback inference (sound — a superset); the result joins the
    // empty-input case (`init`) with a step result (`B`). The no-init
    // `(reduce f coll)` form starts the accumulator at `coll`'s first element.
    // Both `init` and `B` must be known, else flat.
    if value::symbol_is(head, "reduce") || value::symbol_is(head, "fold") {
        let f = *items.get(1)?;
        let (init_ty, coll) = match items.len() {
            // (fold f init coll) / (reduce f init coll)
            4 => (expr_ty(heap, items[2], ctx), items[3]),
            // (reduce f coll) — initial accumulator is the first element
            3 if value::symbol_is(head, "reduce") => {
                let coll = items[2];
                let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty().cloned());
                (elem, coll)
            }
            _ => return None,
        };
        let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty().cloned());
        let b = callback_ret(heap, f, &[Some(Ty::ANY), elem], ctx);
        return match (init_ty, b) {
            (Some(i), Some(b)) => Some(i.union(b)),
            _ => None,
        };
    }
    None
}

/// The result type of a list-producing combinator (`map`/`filter`): `nil |
/// list<elem>` — empty input maps/filters to `nil`. `None` element → `None`, so
/// the caller falls back to the flat curated `list` (never a too-narrow result).
fn list_result(elem: Option<Ty>) -> Option<Ty> {
    elem.map(|e| Ty::list_of(e).union(Ty::of(Tag::Nil)))
}

/// The return type of a HOF callback `f` whose parameters receive the given
/// `inputs` types (`[elem]` for `map`'s `(f x)`; `[any, elem]` for `reduce`/`fold`'s
/// `(f acc x)`, the accumulator over-approximated as `any`). A `None` input is an
/// unknown parameter type.
/// - a named **global** fn → its signature's return type (`sig_of`);
/// - a straight-line lambda with exactly `inputs.len()` plain params → `body`'s
///   type with each `pᵢ` bound to `inputs[i]` (identity preserves its input);
/// - anything else (a local var, an unknown form) → `None` (flat result).
///
/// The lambda case is the only new inference, and it only computes a *forward*
/// result type — it never *checks* the body, so it doesn't reopen the deferred
/// guarded-use false-positive class.
fn callback_ret(heap: &Heap, f: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    match f {
        // A local binding shadows the global table — its return type isn't known.
        Value::Sym(s) if ctx.is_local(s) => None,
        Value::Sym(s) => sig_of(heap, s).map(|sig| sig.ret),
        Value::Pair(_) => lambda_ret(heap, f, inputs, ctx),
        _ => None,
    }
}

/// The return type of a **simple** single-clause lambda `(fn (p…) body)` —
/// exactly `inputs.len()` plain-symbol parameters and one body expression —
/// computed by binding each `pᵢ` to `inputs[i]` and typing `body`. `None` for
/// anything subtler (wrong param count / multi-body / docstring / variadic /
/// destructuring / non-`fn` head), so the result stays flat.
fn lambda_ret(heap: &Heap, form: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    let items = list_items(heap, form)?;
    let Some(Value::Sym(head)) = items.first().copied() else {
        return None;
    };
    if !value::symbol_is(head, kw::FN) {
        return None;
    }
    // Exactly `(fn <param-list> <body>)` — one param list + one body expression.
    let parts = &items[1..];
    if parts.len() != 2 {
        return None;
    }
    let params = list_items(heap, parts[0])?;
    if params.len() != inputs.len() {
        return None; // arity must match what the combinator supplies
    }
    let mut sub = ctx.clone();
    for (param, input) in params.iter().zip(inputs) {
        let Value::Sym(p) = param else {
            return None; // not a plain-symbol parameter
        };
        sub = sub.bind(*p, input.clone());
    }
    expr_ty(heap, parts[1], &sub)
}
