//! Sealed-`match` exhaustiveness — ADR-187 part 2. Advisory, and **sound by construction**:
//! it warns only when a `match` provably leaves a possible case unhandled.
//!
//! A `match` on a scrutinee whose static type is a *finite, closed set of record ids* — the
//! type a sealed ability resolves to (ADR-181/186: `%{__id__: (:a | :b | …)}`) — should cover
//! every member id or carry a catch-all. This pass walks the **un-expanded** forms (`match`
//! survives only pre-expansion), resolves each scrutinee's type via [`expr_ty`] over a `Ctx`
//! it threads (defn params seeded from their `sig`, `let` bindings), extracts the id set, and
//! warns for any member no clause handles.
//!
//! Everything it cannot *prove* defers to silence, so it never false-positives:
//! - scrutinee type unknown / not a closed record-id set (`expr_ty` → `None`, or no keyword
//!   literal on `:__id__`) → no warning;
//! - an unguarded `(record NAME …)` arm counts as covering NAME regardless of its inner field
//!   pattern — over-counting coverage only ever *under*-warns (misses a partial-field gap),
//!   never invents one — while a `:when` guard or any non-record, non-catch-all arm defers the
//!   whole `match`;
//! - ids are compared by their final `mod/NAME` segment, so a name that fails to line up can
//!   only *under*-count coverage, never manufacture a gap.

use std::collections::BTreeSet;

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::Pos;
use crate::types::Ty;

use super::ctx::Ctx;
use super::infer::expr_ty;
use super::sigs::declared_heap_sig;
use super::walk::{fn_params, list_items};

/// Entry: walk every top-level form for sealed-`match` exhaustiveness, from the file's
/// accumulated `ctx` (globals + sigs + abilities). `(defmodule M …)` is a directive that sets
/// the namespace for the *following* top-level forms, so a defn's sig is stored qualified as
/// `M/name`; we track the current namespace here to qualify the lookup.
pub(super) fn check_matches(
    heap: &Heap,
    forms: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let mut ns: Option<String> = None;
    for &form in forms {
        if let Some(items) = list_items(heap, form) {
            if matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, "defmodule")) {
                if let Some(&Value::Sym(m)) = items.get(1) {
                    ns = Some(value::symbol_name(m));
                }
            }
        }
        walk(heap, form, ctx, ns.as_deref(), out);
    }
}

/// The final `mod/name` segment of an id/name string — bare `circle`, `u/circle`, and
/// `editor/display/circle` all reduce to `circle`. Matching on this can only *under*-count
/// coverage (a benign missed gap), never manufacture one, so it stays sound.
fn last_seg(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// A defn/def name's declared sig. `register_declared_sig` qualifies each `(sig …)` target to
/// the file's namespace (ADR-188), so inside a `(defmodule M …)` the current file's sig for a
/// bare `name` is stored in `ctx` under `M/name` — try that before the (cross-file) heap store.
fn sig_of(
    heap: &Heap,
    ctx: &Ctx,
    name: value::Symbol,
    ns: Option<&str>,
) -> Option<crate::types::Sig> {
    let qualified = ns.map(|n| value::intern(&format!("{}/{}", n, value::symbol_name(name))));
    ctx.declared_sig(name)
        .or_else(|| qualified.and_then(|q| ctx.declared_sig(q)))
        .or_else(|| declared_heap_sig(heap, name))
        .or_else(|| qualified.and_then(|q| declared_heap_sig(heap, q)))
}

/// Recurse, threading `ctx` and the current namespace. Binding forms extend the scope; a
/// `match` is analysed; a `quote`/`quasiquote` subtree is skipped (data / patterns, not code).
fn walk(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&head) = items.first() else {
            return;
        };
        if let Value::Sym(h) = head {
            match value::symbol_name(h).as_str() {
                "quote" | "quasiquote" => return,
                "defn" | "defmacro" => {
                    walk_defn(heap, &items, ctx, ns, out);
                    return;
                }
                "def" => {
                    walk_def(heap, &items, ctx, ns, out);
                    return;
                }
                "fn" => {
                    walk_fn(heap, &items, ctx, ns, out);
                    return;
                }
                "let" | "letrec" => {
                    walk_let(heap, &items, ctx, ns, out);
                    return;
                }
                "match" | "match*" => {
                    analyze_match(heap, form, &items, ctx, ns, out);
                    return;
                }
                _ => {}
            }
        }
        for &it in &items {
            walk(heap, it, ctx, ns, out);
        }
    })
}

/// `(defn name (params…) body…)` — seed each param from `name`'s declared `sig` (so a
/// sealed-ability-typed param resolves to its record-id set), then walk the body. Only the
/// single-arity shape is seeded; a multi-clause `defn` falls through to a plain recurse
/// (unseeded → scrutinees stay unknown → silent, still sound).
fn walk_defn(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let (Some(&Value::Sym(name)), Some(&params_form)) = (items.get(1), items.get(2)) else {
        recurse_rest(heap, items, ctx, ns, out);
        return;
    };
    // Only a bare parameter *list* is the seedable single-arity shape; a clause head
    // `((a) …)` (multi-arity) or a docstring is not.
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        recurse_rest(heap, items, ctx, ns, out);
        return;
    }
    let sig = sig_of(heap, ctx, name, ns);
    let params = fn_params(heap, params_form);
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        scope = scope.bind(p, sig.as_ref().and_then(|s| s.param(i)));
    }
    for &b in items.get(3..).unwrap_or(&[]) {
        walk(heap, b, &scope, ns, out);
    }
}

/// `(def name (fn …))` — the shape `defn` expands to; seed the fn from `name`'s sig.
fn walk_def(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if let (Some(&Value::Sym(name)), Some(&val)) = (items.get(1), items.get(2)) {
        if let Some(vitems) = list_items(heap, val) {
            if matches!(vitems.first(), Some(&Value::Sym(h)) if value::symbol_is(h, "fn")) {
                let sig = sig_of(heap, ctx, name, ns);
                walk_fn_seeded(heap, &vitems, ctx, ns, out, sig.as_ref());
                return;
            }
        }
    }
    recurse_rest(heap, items, ctx, ns, out);
}

/// `(fn (params…) body…)` — params are binders (unknown type unless seeded); walk the body.
fn walk_fn(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    walk_fn_seeded(heap, items, ctx, ns, out, None);
}

fn walk_fn_seeded(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
    sig: Option<&crate::types::Sig>,
) {
    let Some(&params_form) = items.get(1) else {
        return;
    };
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        for &b in items.get(1..).unwrap_or(&[]) {
            walk(heap, b, ctx, ns, out);
        }
        return;
    }
    let params = fn_params(heap, params_form);
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        scope = scope.bind(p, sig.and_then(|s| s.param(i)));
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, ns, out);
    }
}

/// `(let (v1 e1 …) body…)` — bind each simple `sym` target to `expr_ty(rhs)` in the
/// progressively-extended scope (a destructuring target binds nothing), then walk the body.
fn walk_let(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let mut scope = ctx.clone();
    if let Some(&binds_form) = items.get(1) {
        if let Some(binds) = list_items(heap, binds_form) {
            let mut i = 0;
            while i + 1 < binds.len() {
                let target = binds[i];
                let rhs = binds[i + 1];
                walk(heap, rhs, &scope, ns, out);
                if let Value::Sym(s) = target {
                    let ty = expr_ty(heap, rhs, &scope);
                    scope = scope.bind(s, ty);
                }
                i += 2;
            }
        }
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, ns, out);
    }
}

fn recurse_rest(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &it in items.iter().skip(1) {
        walk(heap, it, ctx, ns, out);
    }
}

/// `(match scrutinee clause…)` — if the scrutinee's type is a closed record-id set, check that
/// the clauses cover it. Always recurse the scrutinee and clause bodies (for nested matches).
fn analyze_match(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    ns: Option<&str>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if let Some(&scrutinee) = items.get(1) {
        if let Some(ty) = expr_ty(heap, scrutinee, ctx) {
            if let Some(ids) = record_id_set(&ty) {
                check_coverage(heap, &items[2..], &ids, heap.form_pos_only(form), out);
            }
        }
        walk(heap, scrutinee, ctx, ns, out);
        for &clause in &items[2..] {
            if let Some(citems) = list_items(heap, clause) {
                let guarded = matches!(citems.get(1),
                    Some(&Value::Keyword(k)) if value::symbol_is(k, "when"));
                let body_start = if guarded { 3 } else { 1 };
                if guarded {
                    if let Some(&g) = citems.get(2) {
                        walk(heap, g, ctx, ns, out);
                    }
                }
                for &b in citems.get(body_start..).unwrap_or(&[]) {
                    walk(heap, b, ctx, ns, out);
                }
            }
        }
    }
}

/// The closed set of record ids a type denotes, or `None` when the type is not a record shape
/// carrying a closed keyword-literal `:__id__` (an open `:__id__`, or a non-record type, yields
/// `None` — the scrutinee's ids aren't enumerable, so exhaustiveness defers).
fn record_id_set(ty: &Ty) -> Option<BTreeSet<String>> {
    let fields = ty.record_fields()?;
    let (id_ty, _) = fields.get(&value::intern("__id__"))?;
    let lits = id_ty.as_lit()?;
    if lits.is_empty() {
        return None;
    }
    Some(lits.iter().map(|&s| value::symbol_name(s)).collect())
}

/// Warn for each id in `ids` no clause provably handles. Sound: an unguarded catch-all makes
/// the match exhaustive; an unguarded `(record NAME …)` covers NAME; any other clause (a
/// `:when` guard, or a non-record non-catch-all pattern that could match any id) defers the
/// whole match.
fn check_coverage(
    heap: &Heap,
    clauses: &[Value],
    ids: &BTreeSet<String>,
    pos: Option<Pos>,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for &clause in clauses {
        let Some(citems) = list_items(heap, clause) else {
            return;
        };
        let Some(&pat) = citems.first() else {
            return;
        };
        // A guard means the clause is not guaranteed to fire; reasoning past it is unsound.
        if matches!(citems.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "when")) {
            return;
        }
        if is_catch_all(pat) {
            return; // total fallback → exhaustive
        }
        if let Some(id) = record_pattern_id(heap, pat) {
            // Over-counting a refutable inner (`{:r 5}`) only under-warns; never a false one.
            covered.insert(last_seg(&id).to_string());
            continue;
        }
        return; // a non-record, non-catch-all arm could match any id → defer (sound)
    }
    for id in ids {
        if !covered.contains(last_seg(id)) {
            out.push((
                pos,
                format!(
                    "sealed match: no clause handles :{} (add a clause or a `_` catch-all)",
                    id
                ),
            ));
        }
    }
}

/// A pattern that matches every value: `_` or a bare binder symbol.
fn is_catch_all(pat: Value) -> bool {
    matches!(pat, Value::Sym(_))
}

/// The record id of a record pattern `(record NAME …)` — its `NAME`, ignoring the (optional)
/// field pattern. `None` for a non-record pattern.
fn record_pattern_id(heap: &Heap, pat: Value) -> Option<String> {
    let items = list_items(heap, pat)?;
    match items.first() {
        Some(&Value::Sym(h)) if value::symbol_is(h, "record") => {}
        _ => return None,
    }
    match items.get(1) {
        Some(&Value::Sym(name)) => Some(value::symbol_name(name)),
        _ => None,
    }
}
