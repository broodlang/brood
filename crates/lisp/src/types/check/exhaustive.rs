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
//! - **any** clause it can't prove total (a `:when` guard, a non-record pattern, a record
//!   pattern with a refutable field sub-pattern) → the whole `match` defers;
//! - ids are compared by their final `mod/NAME` segment, so a name that fails to line up can
//!   only *under*-count coverage (miss a real gap), never invent one.

use std::collections::BTreeSet;

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::Pos;
use crate::types::Ty;

use super::ctx::Ctx;
use super::infer::expr_ty;
use super::walk::{fn_params, list_items};

/// Entry: walk every top-level form for sealed-`match` exhaustiveness, starting from the
/// file's accumulated `ctx` (globals + sigs + abilities).
pub(super) fn check_matches(
    heap: &Heap,
    forms: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    for &form in forms {
        walk(heap, form, ctx, out);
    }
}

/// The final `mod/name` segment of an id/name string — bare `circle`, `u/circle`, and
/// `editor/display/circle` all reduce to `circle`. Matching on this can only *under*-count
/// coverage (a benign missed gap), never manufacture one, so it stays sound.
fn last_seg(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Recurse, threading `ctx`. Binding forms extend the scope; a `match` is analysed; a
/// `quote`/`quasiquote` subtree is skipped (its contents are data / patterns, not code).
fn walk(heap: &Heap, form: Value, ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
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
                    walk_defn(heap, &items, ctx, out);
                    return;
                }
                "def" => {
                    walk_def(heap, &items, ctx, out);
                    return;
                }
                "fn" => {
                    walk_fn(heap, &items, ctx, out);
                    return;
                }
                "let" | "letrec" => {
                    walk_let(heap, &items, ctx, out);
                    return;
                }
                "match" | "match*" => {
                    analyze_match(heap, form, &items, ctx, out);
                    return;
                }
                _ => {}
            }
        }
        // Default: recurse into every element (head included, if it is itself a form).
        for &it in &items {
            walk(heap, it, ctx, out);
        }
    })
}

/// `(defn name (params…) body…)` — seed each param from `name`'s declared `sig` (so a
/// sealed-ability-typed param resolves to its record-id set), then walk the body. Only the
/// single-arity shape is seeded; a multi-clause `defn` falls through to a plain recurse
/// (unseeded → scrutinees stay unknown → silent, still sound).
fn walk_defn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let (Some(&Value::Sym(name)), Some(&params_form)) = (items.get(1), items.get(2)) else {
        recurse_rest(heap, items, ctx, out);
        return;
    };
    // Only a bare parameter *list* is the seedable single-arity shape; a clause head
    // `((a) …)` (multi-arity) or a docstring is not.
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        recurse_rest(heap, items, ctx, out);
        return;
    }
    let sig = ctx.declared_sig(name);
    let params = fn_params(heap, params_form);
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        let ty = sig.as_ref().and_then(|s| s.param(i));
        scope = scope.bind(p, ty);
    }
    for &b in items.get(3..).unwrap_or(&[]) {
        walk(heap, b, &scope, out);
    }
}

/// `(def name (fn …))` — the shape `defn` expands to; seed the fn from `name`'s sig.
fn walk_def(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    if let (Some(&Value::Sym(name)), Some(&val)) = (items.get(1), items.get(2)) {
        if let Some(vitems) = list_items(heap, val) {
            if matches!(vitems.first(), Some(&Value::Sym(h)) if value::symbol_is(h, "fn")) {
                let sig = ctx.declared_sig(name);
                walk_fn_seeded(heap, &vitems, ctx, out, sig.as_ref());
                return;
            }
        }
    }
    recurse_rest(heap, items, ctx, out);
}

/// `(fn (params…) body…)` — params are binders (unknown type unless seeded); walk the body.
fn walk_fn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    walk_fn_seeded(heap, items, ctx, out, None);
}

fn walk_fn_seeded(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    sig: Option<&crate::types::Sig>,
) {
    let Some(&params_form) = items.get(1) else {
        return;
    };
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        // Multi-clause fn (or malformed) — recurse the bodies without seeding.
        for &b in items.get(1..).unwrap_or(&[]) {
            walk(heap, b, ctx, out);
        }
        return;
    }
    let params = fn_params(heap, params_form);
    let mut scope = ctx.clone();
    for (i, &p) in params.iter().enumerate() {
        scope = scope.bind(p, sig.and_then(|s| s.param(i)));
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, out);
    }
}

/// `(let (v1 e1 v2 e2 …) body…)` — bind each simple `sym` target to `expr_ty(rhs)` in the
/// progressively-extended scope (a destructuring target binds nothing), then walk the body.
fn walk_let(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let mut scope = ctx.clone();
    if let Some(&binds_form) = items.get(1) {
        if let Some(binds) = list_items(heap, binds_form) {
            let mut i = 0;
            while i + 1 < binds.len() {
                let target = binds[i];
                let rhs = binds[i + 1];
                // Walk the RHS too (it may contain a match), in the scope so far.
                walk(heap, rhs, &scope, out);
                if let Value::Sym(s) = target {
                    let ty = expr_ty(heap, rhs, &scope);
                    scope = scope.bind(s, ty);
                }
                i += 2;
            }
        }
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, out);
    }
}

fn recurse_rest(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    for &it in items.iter().skip(1) {
        walk(heap, it, ctx, out);
    }
}

/// `(match scrutinee clause…)` — if the scrutinee's type is a closed record-id set, check that
/// the clauses cover it. Always recurse the scrutinee and clause bodies (for nested matches).
fn analyze_match(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    if let Some(&scrutinee) = items.get(1) {
        if let Some(ty) = expr_ty(heap, scrutinee, ctx) {
            if let Some(ids) = record_id_set(&ty) {
                check_coverage(heap, &items[2..], &ids, heap.form_pos_only(form), out);
            }
        }
        // Recurse into the scrutinee and each clause's guard + body (a clause's pattern is not
        // code, and pattern-bound vars aren't in scope here — a nested match depending on one
        // just resolves to `None` and defers).
        walk(heap, scrutinee, ctx, out);
        for &clause in &items[2..] {
            if let Some(citems) = list_items(heap, clause) {
                let guarded = matches!(citems.get(1),
                    Some(&Value::Keyword(k)) if value::symbol_is(k, "when"));
                let body_start = if guarded { 3 } else { 1 };
                if guarded {
                    if let Some(&g) = citems.get(2) {
                        walk(heap, g, ctx, out);
                    }
                }
                for &b in citems.get(body_start..).unwrap_or(&[]) {
                    walk(heap, b, ctx, out);
                }
            }
        }
    }
}

/// The closed set of record-id final-segments a type denotes, or `None` when the type is not a
/// record shape carrying a closed keyword-literal `:__id__` (an open `:__id__`, or a non-record
/// type, yields `None` — the scrutinee's ids aren't enumerable, so exhaustiveness defers).
fn record_id_set(ty: &Ty) -> Option<BTreeSet<String>> {
    let fields = ty.record_fields()?;
    let (id_ty, _) = fields.get(&value::intern("__id__"))?;
    let lits = id_ty.as_lit()?;
    if lits.is_empty() {
        return None;
    }
    Some(lits.iter().map(|&s| value::symbol_name(s)).collect())
}

/// Warn for each id in `ids` no clause provably handles. Sound: any clause not provably total
/// (a guard, a non-record pattern, a refutable record field pattern) defers the whole match;
/// an unguarded catch-all makes it exhaustive.
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
            return; // malformed clause — defer
        };
        let Some(&pat) = citems.first() else {
            return;
        };
        // Any guard means this clause is not guaranteed to fire; reasoning about coverage past
        // it is unsound, so defer the whole match.
        if matches!(citems.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "when")) {
            return;
        }
        if is_catch_all(pat) {
            return; // total fallback → exhaustive
        }
        if let Some(id) = record_pattern_total_id(heap, pat) {
            covered.insert(last_seg(&id).to_string());
            continue;
        }
        return; // an arm we can't prove total → defer (sound)
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

/// The record id of a *total* record pattern `(record NAME)` / `(record NAME {…})` — one whose
/// field pattern always matches (absent, `{}`, or only `:keys`/`:or` destructuring). `None`
/// for a non-record pattern or a record pattern with a refutable `{:k p}` field entry.
fn record_pattern_total_id(heap: &Heap, pat: Value) -> Option<String> {
    let items = list_items(heap, pat)?;
    match items.first() {
        Some(&Value::Sym(h)) if value::symbol_is(h, "record") => {}
        _ => return None,
    }
    let Some(&Value::Sym(name)) = items.get(1) else {
        return None;
    };
    // Optional field map must be provably total.
    match items.get(2) {
        None => {}
        Some(&Value::Map(mid)) => {
            for (k, _) in heap.map_entries(mid) {
                match k {
                    Value::Keyword(kw)
                        if value::symbol_is(kw, "keys") || value::symbol_is(kw, "or") => {}
                    _ => return None, // an explicit `{:k p}` entry is refutable → not total
                }
            }
        }
        Some(_) => return None,
    }
    Some(value::symbol_name(name))
}
