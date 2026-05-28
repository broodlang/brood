//! Predicates over forms that the walker dispatches on:
//!
//! - [`is_syntactic_keyword`] / [`skips_body`] — which heads are *not*
//!   callables (so an "unbound symbol" warning doesn't fire on them) and
//!   which bodies are data the walker should not descend into.
//! - [`guard_assertion`] / [`literal_eq_guard`] — pull a `(sym, type)` pair
//!   out of an `if`-test when it's a recognised guard, so the walk can
//!   narrow the variable in each branch.
//! - [`expr_ty`] — the static type of a form `in ctx`, the single
//!   "do I know what this expression returns?" probe the misuse-check
//!   reads off.

use crate::core::heap::Heap;
use crate::core::value::{self, Symbol, Value};
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
        "quote"
            | "quasiquote"
            | "unquote"
            | "unquote-splicing"
            | "if"
            | "do"
            | "def"
            | "fn"
            | "lambda"
            | "let"
            | "let*"
            | "letrec"
            | "defmacro"
            | "defn"
            | "defdyn"
            | "defmodule"
            | "module-doc"
            | "when"
            | "unless"
            | "cond"
            | "and"
            | "or"
            | "->"
            | "->>"
            | "match"
            | "case"
            | "try"
            | "catch"
            | "throw"
            | "binding"
            | "for"
            | "spawn"
            | "&"
            | "&optional"
            | "&rest"
    )
}

/// Forms whose contents are data (`quote`/`quasiquote`) or deliberately
/// exercise failures (`try` / `error-of` / `assert-error` pre-expansion;
/// `%try` post-expansion — they all bottom out at the same primitive). Don't
/// look inside.
///
/// **Post-expansion matters.** `check_file` macroexpands first so threading
/// macros and `match` patterns get their real shape — but that also rewrites
/// `(try …)` to `(%try (fn () body) (fn (e) handler))`. Without `%try` here,
/// the walk would descend into the user's "I expect this to fail" body and
/// flag the very errors they're asserting on (every `(error-of (cons 1))` in
/// the test suite would warn). `assert-error` / `error-of` expand *through*
/// `try`, so `%try` covers them too.
pub(super) fn skips_body(name: &str) -> bool {
    matches!(
        name,
        "quote" | "quasiquote" | "try" | "error-of" | "assert-error" | "%try"
    )
}

/// If `test` is a recognisable type guard over a single variable, return the
/// `(sym, asserted_type)` pair — the type `sym` provably has when `test` is
/// truthy. A leading `(not …)` flips the assertion via [`Ty::negate`]. A bare
/// `Sym` is looked up in `ctx`'s guard-alias table (a `let`-stored guard
/// result — `(let (cond (int? x)) (if cond …))`). `None` for any test that
/// isn't a pure pattern-matchable guard (so we never narrow on something we
/// can't soundly invert in the else-branch).
pub(super) fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<(Symbol, Ty)> {
    if let Value::Sym(s) = test {
        return ctx.guard(s);
    }
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // (not <inner>) — invert the inner assertion; everything else proceeds.
    if items.len() == 2 && head_name == "not" {
        let (sym, ty) = guard_assertion(heap, items[1], ctx)?;
        return Some((sym, ty.negate()));
    }
    // `(%eq sym literal)` / `(%eq literal sym)` — equality against a literal
    // asserts the variable has the literal's runtime tag. The `match` pattern
    // compiler emits this for literal patterns (e.g. `(match x (5 …))`
    // lowers through `(let (m x) (if (%eq m 5) …))` — and the let-alias
    // machinery threads the narrowing back to `x`). Variadic `=` reaches us
    // pre-expanded as `%eq` calls when arities are 2, so we only need to
    // recognise the primitive shape.
    if items.len() == 3 && head_name == "%eq" {
        if let Some(g) = literal_eq_guard(items[1], items[2]) {
            return Some(g);
        }
        if let Some(g) = literal_eq_guard(items[2], items[1]) {
            return Some(g);
        }
        return None;
    }
    if items.len() != 2 {
        return None;
    }
    let ty = Ty::tested_by(&head_name)?;
    match items[1] {
        Value::Sym(s) => Some((s, ty)),
        _ => None,
    }
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
