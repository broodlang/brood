//! The recursive walk: visit every sub-form, open the right scope at each
//! binder (`let` / `fn` / `defn` / …), and at every call site cross-check
//! arity + per-argument type against what the callee accepts.
//!
//! Each `check_*` helper for a special form clones its enclosing `Ctx`, adds
//! the binder(s) it introduces, walks its body in that extended scope, and
//! returns — the generic call-form path at the bottom of [`check_into`] runs
//! only for non-special heads. [`fn_params`], [`list_items`], [`bindings`]
//! are the tiny syntax-shape readers the rest of the walk shares; they're
//! `pub(super)` so the sibling submodules (`sigs`, `guards`) can use them.

use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::value::{self, Symbol, Value};
use crate::error::Pos;

use super::ctx::Ctx;
use super::guards::{expr_ty, guard_assertion, is_syntactic_keyword};
use super::sigs::{arity_of, arity_str, curated_sig, is_globally_bound, sig_of};

/// What the walk does at a head symbol. `Generic` is the fall-through for any
/// head that isn't one of the recognised special forms / skip-body markers —
/// the walk treats it as a normal call (resolves sig + arity, checks for
/// unbound). One [`SymbolMap`] lookup decides: pre-consolidation each call
/// allocated a `String` via `value::symbol_name` just to feed a chain of
/// `matches!(name.as_str(), "if" | …)` plus `skips_body(&name)` — that was
/// the hot allocation the review flagged. (`eval/mod.rs` uses the same
/// `SymbolMap` pattern on its own loop.)
#[derive(Clone, Copy)]
enum SpecialHead {
    /// `quote` / `quasiquote` / `try` / `error-of` / `assert-error` / `%try`
    /// — return without descending. Mirrors `guards::skips_body`.
    SkipBody,
    If,
    /// `let` / `let*` — sequential bind, no pre-binding.
    Let,
    /// `letrec` — pre-bind every name before walking RHSs (mutual recursion).
    Letrec,
    /// `fn` / `lambda` — open a fresh scope with the params bound.
    Fn,
    /// `def` — `name` is a binder, value is an expression.
    Def,
    /// `defn` / `defmacro` — same shape as `fn`/`lambda` plus a binder name.
    Defn,
}

static SPECIAL_HEAD: LazyLock<SymbolMap<SpecialHead>> = LazyLock::new(|| {
    use SpecialHead::*;
    [
        ("quote", SkipBody),
        ("quasiquote", SkipBody),
        ("try", SkipBody),
        ("error-of", SkipBody),
        ("assert-error", SkipBody),
        ("%try", SkipBody),
        ("if", If),
        ("let", Let),
        ("let*", Let),
        ("letrec", Letrec),
        ("fn", Fn),
        ("lambda", Fn),
        ("def", Def),
        ("defn", Defn),
        ("defmacro", Defn),
    ]
    .into_iter()
    .map(|(n, k)| (value::intern(n), k))
    .collect()
});

/// Walk `form` recursively, adding to `ctx.file_globals` every name introduced
/// by a `(def name …)` or `(defmacro name …)` — at any depth, since Brood's
/// `def` always binds globally regardless of where it textually sits (a
/// `(when … (def x 1))` makes `x` a global when the `when` runs).
///
/// Recursion stops at forms whose body is data, not code (`quote` /
/// `quasiquote`) — a `(quote (def x …))` is a literal list, not a binder.
/// Doesn't recurse into a `fn`/`lambda` body either: a `def` *inside* a
/// closure body only fires when the closure is called, but since the body
/// runs later and Brood's `def` is global, the result is the same — we still
/// want the name in scope. So we *do* recurse there. The only thing we skip
/// is `quote`/`quasiquote`.
pub(super) fn collect_def_names(heap: &Heap, form: Value, ctx: &mut Ctx) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    // Lock-free `symbol_is` instead of allocating the head's spelling — the
    // walk visits every nested form, and only four comparisons are needed.
    if value::symbol_is(head, "quote") || value::symbol_is(head, "quasiquote") {
        return;
    }
    if value::symbol_is(head, "def") || value::symbol_is(head, "defmacro") {
        if let Some(&Value::Sym(name)) = items.get(1) {
            ctx.add_file_global(name);
        }
    }
    for &item in &items[1..] {
        collect_def_names(heap, item, ctx);
    }
}

pub(super) fn check_into(
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
        // One `SymbolMap` probe dispatches the recognised special-form heads —
        // no `value::symbol_name` allocation for the common short-circuit
        // paths (`if`/`let`/`fn`/…). The fallthrough computes the spelling
        // once for the call-resolution work below (sig/arity/error messages).
        if let Some(&kind) = SPECIAL_HEAD.get(&s) {
            match kind {
                SpecialHead::SkipBody => return,
                SpecialHead::If => {
                    check_if(heap, &items, ctx, out);
                    return;
                }
                SpecialHead::Let => {
                    check_let(heap, &items, ctx, out, false);
                    return;
                }
                SpecialHead::Letrec => {
                    // `letrec` pre-binds every name to `nil` so all bindings are
                    // visible in every RHS — that's the mutual-recursion reason
                    // letrec exists. The checker mirrors this: it pre-binds the
                    // names into the inner scope *before* walking the RHSs, so a
                    // self-recursive or mutually-recursive call doesn't get
                    // flagged unbound.
                    check_let(heap, &items, ctx, out, true);
                    return;
                }
                SpecialHead::Fn => {
                    check_fn(heap, &items, ctx, out);
                    return;
                }
                SpecialHead::Def => {
                    check_def(heap, &items, ctx, out);
                    return;
                }
                SpecialHead::Defn => {
                    check_defn(heap, &items, ctx, out);
                    return;
                }
            }
        }
        let name = value::symbol_name(s);

        // Resolve the callee's signature + arity (separate concerns; either
        // may be available without the other).
        let sig = sig_of(heap, &name);
        let arity = arity_of(heap, &name);
        // Unbound-symbol diagnostic: warn only when the head is **truly not
        // resolvable** — not local, not a syntactic keyword, not in the global
        // env (which includes `Value::Macro`s like `test` / `assert=` that
        // `arity_of` doesn't describe), and not in the curated stdlib table.
        // The unbound check is independent of "is the sig informative" —
        // a macro is bound even though it has no value-type sig.
        if !ctx.is_local(s)
            && !is_syntactic_keyword(&name)
            && !is_globally_bound(heap, &name)
            && curated_sig(&name).is_none()
        {
            out.push((heap.form_pos(form), format!("unbound symbol: {}", name)));
            // Still recurse into args below — they may carry their own issues.
        }

        // Arity check (independent of sig — they're separate concerns).
        if let Some(a) = arity {
            let argc = items.len() - 1;
            if !a.accepts(argc) {
                out.push((
                    heap.form_pos(form),
                    format!(
                        "{}: wrong number of arguments — expected {}, got {}",
                        name,
                        arity_str(a),
                        argc,
                    ),
                ));
            }
        }

        if let Some(sig) = sig {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                //
                // A `NEVER` arg type means "this branch is unreachable" — every
                // intersection with NEVER is NEVER, so it'd warn against
                // *every* param. That's all noise: the code can't execute, so
                // there's no real misuse. Skip. This shows up in pattern-match
                // lowering where a guard has narrowed a variable to a type
                // that has no inhabitants for the current branch.
                if let Some(arg_ty) = expr_ty(heap, arg, ctx) {
                    if arg_ty.is_never() {
                        continue;
                    }
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

/// `(fn (params...) docstring? body...)` (and `lambda` — the same closure
/// shape) — parse the parameter list, bind each into `ctx`, then walk the body
/// in the extended scope. Parameter positions (`& rest`, `&optional`) are
/// binders, not references, so they're not flagged as unbound.
fn check_fn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&params_form) = items.get(1) else {
        return;
    };
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    // Skip a leading docstring (a lone string when more body follows).
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// `(def name value)` — the binder is in position 1, the value in 2. Don't
/// flag `name` as an unbound *reference* (it's a binder); walk `value` as an
/// expression. `name` is added to the file-globals accumulator inside
/// [`check_file`], not here (which checks one form in isolation).
fn check_def(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&value_form) = items.get(2) else {
        // `(def name)` — degenerate; skip.
        return;
    };
    check_into(heap, value_form, ctx, out);
}

/// `(defn name (params...) docstring? body...)` and the structurally identical
/// `defmacro` — the body lives in a fresh scope with `params` bound. Like
/// `def`, the `name` is a binder, not a reference; file-global accumulation
/// happens in [`check_file`].
fn check_defn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&params_form) = items.get(2) else {
        return;
    };
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    let body_start = match (items.get(3), items.get(4)) {
        (Some(Value::Str(_)), Some(_)) => 4,
        _ => 3,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// The set of parameter-binder symbols introduced by a `fn`/`defn`/`defmacro`
/// parameter list. Handles the three Brood shapes uniformly:
///
/// - positional: `(x y z)` → `{x, y, z}`
/// - optional:   `(x &optional (y 0))` → `{x, y}`
/// - rest:       `(x & ys)` → `{x, ys}`
///
/// `&` / `&optional` themselves are markers, not binders, so they're filtered
/// out. The result is *just* what would be in scope — used to seed `Ctx`
/// without false-flagging the inner body's references.
fn fn_params(heap: &Heap, form: Value) -> Vec<Symbol> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Sym(s) => {
                // Lock-free `symbol_is` to filter the parameter-list markers
                // — three name compares without ever allocating the spelling.
                if value::symbol_is(s, "&")
                    || value::symbol_is(s, "&optional")
                    || value::symbol_is(s, "&rest")
                {
                    continue;
                }
                out.push(s);
            }
            // `&optional` defaults: `(name default)` — the binder is at [0].
            Value::Pair(_) | Value::Vector(_) => {
                let inner = match item {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => list_items(heap, item).unwrap_or_default(),
                };
                if let Some(&Value::Sym(s)) = inner.first() {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
    out
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

/// `(let bindings body…)` / `(let* …)` / `(letrec …)` — walk the bindings,
/// then check the body in the extended ctx. `letrec` pre-binds every name to
/// "in scope, type unknown" before walking RHSs, matching the evaluator's
/// nil-pre-bind so a self/mutual-recursive call inside a RHS isn't flagged
/// unbound. `let`/`let*` walk sequentially — each RHS sees only the
/// previously-bound names. (The let-vs-let* scope distinction doesn't affect
/// the unbound check since we only widen names; type-flow stays sound.)
///
/// Quietly skips a malformed bindings shape (a pattern-target `let`, an
/// improper list, an odd number of binding items): those are evaluator-level
/// errors and aren't this checker's job.
fn check_let(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    letrec: bool,
) {
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
    // letrec: pre-bind every name to `None` (in scope, no known type) so each
    // RHS can refer to its peers (and to itself).
    if letrec {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                scope = scope.bind(name, None);
            }
            j += 2;
        }
    }
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
        // A plain `(let (name other) …)` aliases `name` to `other` — narrowing
        // either propagates to the other via `narrow_chain`. This is what
        // makes the `match` pattern compiler's `(let (m__28 x) (if (%eq m__28
        // lit) …))` expansion narrow the user's `x`, not just the internal
        // `m__28`. We don't gate on `other` being a known local: it might be
        // a free reference (e.g. when checking a bare form via
        // `(check 'form)`) or a top-level global — either way, narrowing
        // inside the branch is sound (it describes "if this branch is
        // reached, then…", vacuously true on unreachable paths).
        if let Value::Sym(target) = rhs {
            scope = scope.add_alias(name, target);
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
/// `pub(super)` because `sigs` (`infer_sig`) and `guards` (`guard_assertion`,
/// `expr_ty`) all need to peel a list head off a call form.
pub(super) fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
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
