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
use crate::types::check::{Finding, Lint};

use super::ctx::Ctx;
use super::infer::expr_ty;
use super::sigs::declared_heap_sig;
use super::walk::{fn_form_items, fn_params, list_items, resolves_to_macro};

/// Entry: walk every top-level form for sealed-`match` exhaustiveness, from the file's
/// accumulated `ctx` (globals + sigs + abilities). `(defmodule M …)` is a directive that sets
/// the namespace for the *following* top-level forms, so a defn's sig is stored qualified as
/// `M/name`; we track the current namespace here to qualify the lookup.
pub(super) fn check_matches(heap: &Heap, forms: &[Value], ctx: &Ctx, out: &mut Vec<Finding>) {
    let mut namespace: Option<String> = None;
    for &form in forms {
        if let Some(items) = list_items(heap, form)
            && matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, "defmodule"))
            && let Some(&Value::Sym(module)) = items.get(1)
        {
            namespace = Some(value::symbol_name(module));
        }
        walk(heap, form, ctx, namespace.as_deref(), out);
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
    namespace: Option<&str>,
) -> Option<crate::types::Sig> {
    let qualified =
        namespace.map(|module| value::intern(&format!("{}/{}", module, value::symbol_name(name))));
    ctx.declared_sig(name)
        .or_else(|| qualified.and_then(|q| ctx.declared_sig(q)))
        .or_else(|| declared_heap_sig(heap, name))
        .or_else(|| qualified.and_then(|q| declared_heap_sig(heap, q)))
}

/// Recurse, threading `ctx` and the current namespace. Binding forms extend the scope; a
/// `match` is analysed; a `quote`/`quasiquote` subtree is skipped (data / patterns, not code).
fn walk(heap: &Heap, form: Value, ctx: &Ctx, namespace: Option<&str>, out: &mut Vec<Finding>) {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
        let Some(items) = list_items(heap, form) else {
            return;
        };
        let Some(&head) = items.first() else {
            return;
        };
        if let Value::Sym(h) = head {
            let name = value::symbol_name(h);
            // `/%match` (a root escape, ADR-638) is the same form as `%match`
            match name
                .strip_prefix('/')
                .filter(|n| !n.is_empty())
                .unwrap_or(&name)
            {
                "quote" | "quasiquote" => return,
                "defn" | "defmacro" => {
                    walk_defn(heap, &items, ctx, namespace, out);
                    return;
                }
                "def" => {
                    walk_def(heap, &items, ctx, namespace, out);
                    return;
                }
                "fn" => {
                    walk_fn(heap, &items, ctx, namespace, out);
                    return;
                }
                "let" | "letrec" => {
                    walk_let(heap, &items, ctx, namespace, out);
                    return;
                }
                "match" | "%match" => {
                    analyze_match(heap, form, &items, ctx, namespace, out);
                    return;
                }
                _ => {}
            }
        }
        // Any other form may bind: `if-let`, `for`, `try`'s `catch`, a multi-clause `fn`'s
        // `((s) body)` — a binder this pass cannot read. Forget, for its subtree, every
        // name it mentions, so a rebinding never keeps the OUTER sealed type.
        let scope = if binds_nothing(heap, ctx, head) {
            ctx.clone()
        } else {
            forget_symbols_in(heap, ctx, &items[1..])
        };
        for &it in &items {
            walk(heap, it, &scope, namespace, out);
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
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    let (Some(&Value::Sym(name)), Some(&params_form)) = (items.get(1), items.get(2)) else {
        recurse_rest(heap, items, ctx, namespace, out);
        return;
    };
    // Only a bare parameter *list* is the seedable single-arity shape; a clause head
    // `((a) …)` (multi-arity) or a docstring is not.
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        recurse_rest(heap, items, ctx, namespace, out);
        return;
    }
    let sig = sig_of(heap, ctx, name, namespace);
    let params = fn_params(heap, params_form);
    // A destructuring parameter (`(fn ([s]) …)`) binds names `fn_params` does not list.
    let mut scope = forget_symbols_in(heap, ctx, &[params_form]);
    for (i, &p) in params.iter().enumerate() {
        scope = scope.bind(p, sig.as_ref().and_then(|s| s.param(i)));
    }
    for &b in items.get(3..).unwrap_or(&[]) {
        walk(heap, b, &scope, namespace, out);
    }
}

/// `(def name (fn …))` — the shape `defn` expands to; seed the fn from `name`'s sig.
fn walk_def(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    if let (Some(&Value::Sym(name)), Some(&val)) = (items.get(1), items.get(2))
        && let Some(value_items) = fn_form_items(heap, val)
    {
        let sig = sig_of(heap, ctx, name, namespace);
        walk_fn_seeded(heap, &value_items, ctx, namespace, out, sig.as_ref());
        return;
    }
    recurse_rest(heap, items, ctx, namespace, out);
}

/// `(fn (params…) body…)` — params are binders (unknown type unless seeded); walk the body.
fn walk_fn(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    walk_fn_seeded(heap, items, ctx, namespace, out, None);
}

fn walk_fn_seeded(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
    sig: Option<&crate::types::Sig>,
) {
    let Some(&params_form) = items.get(1) else {
        return;
    };
    if !matches!(params_form, Value::Pair(_) | Value::Nil) {
        for &b in items.get(1..).unwrap_or(&[]) {
            walk(heap, b, ctx, namespace, out);
        }
        return;
    }
    let params = fn_params(heap, params_form);
    // A destructuring parameter (`(fn ([s]) …)`) binds names `fn_params` does not list.
    let mut scope = forget_symbols_in(heap, ctx, &[params_form]);
    for (i, &p) in params.iter().enumerate() {
        scope = scope.bind(p, sig.and_then(|s| s.param(i)));
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, namespace, out);
    }
}

/// `(let (v1 e1 …) body…)` — bind each simple `sym` target to `expr_ty(rhs)` in the
/// progressively-extended scope (a destructuring target binds nothing), then walk the body.
fn walk_let(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    let mut scope = ctx.clone();
    if let Some(&binds_form) = items.get(1)
        && let Some(binds) = list_items(heap, binds_form)
    {
        let mut i = 0;
        while i + 1 < binds.len() {
            let target = binds[i];
            let rhs = binds[i + 1];
            walk(heap, rhs, &scope, namespace, out);
            if let Value::Sym(s) = target {
                let ty = expr_ty(heap, rhs, &scope);
                scope = scope.bind(s, ty);
            } else {
                // A destructuring target rebinds every name in it, to a type this
                // pass does not derive.
                scope = forget_symbols_in(heap, &scope, &[target]);
            }
            i += 2;
        }
    }
    for &b in items.get(2..).unwrap_or(&[]) {
        walk(heap, b, &scope, namespace, out);
    }
}

fn recurse_rest(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    for &it in items.iter().skip(1) {
        walk(heap, it, ctx, namespace, out);
    }
}

/// `(match scrutinee clause…)` — if the scrutinee's type is a closed record-id set, check that
/// the clauses cover it. Always recurse the scrutinee and clause bodies (for nested matches).
fn analyze_match(
    heap: &Heap,
    form: Value,
    items: &[Value],
    ctx: &Ctx,
    namespace: Option<&str>,
    out: &mut Vec<Finding>,
) {
    if let Some(&scrutinee) = items.get(1) {
        if let Some(ty) = expr_ty(heap, scrutinee, ctx)
            && let Some(ids) = record_id_set(&ty)
        {
            check_coverage(heap, &items[2..], &ids, heap.form_pos_only(form), out);
        }
        check_redundant_shapes(heap, &items[2..], out);
        walk(heap, scrutinee, ctx, namespace, out);
        for &clause in &items[2..] {
            // An empty clause (`nil`) is malformed: the compile pass reports it, so there is
            // no pattern here to scope and nothing to walk.
            if let Some(clause_items) = list_items(heap, clause).filter(|items| !items.is_empty()) {
                let guarded = matches!(clause_items.get(1),
                    Some(&Value::Keyword(k)) if value::symbol_is(k, "when"));
                let body_start = if guarded { 3 } else { 1 };
                // The pattern's binders shadow the enclosing scope for the guard and body
                // (`((record c {:x s}) (match s …))` matches the FIELD, not the outer `s`).
                let clause_scope = forget_symbols_in(heap, ctx, &clause_items[..1]);
                if guarded && let Some(&g) = clause_items.get(2) {
                    walk(heap, g, &clause_scope, namespace, out);
                }
                for &b in clause_items.get(body_start..).unwrap_or(&[]) {
                    walk(heap, b, &clause_scope, namespace, out);
                }
            }
        }
    }
}

/// **Type-based clause redundancy** (ADR-725): a clause whose pattern admits only values an
/// earlier, unguarded clause already took can never run — `((:ok _) …)` after `((:ok _) …)`,
/// `([:ok 5] …)` after `([:ok _] …)`. Each clause's pattern is read as the exact set of
/// values it matches ([`exact_pattern_ty`]); a clause is redundant when its set is inside
/// the union of the sets before it.
///
/// **Sound**, because both sides are exact or the clause is left alone: a pattern with no
/// exact type (a pin, a map, a record, a rest tail, a repeated binder, a float) neither is
/// judged nor counts as coverage, and a `:when` clause never counts (it takes its values
/// only when the guard holds — the "surely accepted" half). The union is used only while it
/// stays exact — at most [`MAX_EXACT_UNION`] terms, below the lattice's widening cap;
/// past that a clause is judged against each earlier clause on its own. A bare literal is
/// the structural duplicate lint's, and a clause after a catch-all is the match compiler's
/// compile error, so neither is reported twice.
fn check_redundant_shapes(heap: &Heap, clauses: &[Value], out: &mut Vec<Finding>) {
    let mut earlier: Vec<Ty> = Vec::new();
    for &clause in clauses {
        let Some(clause_items) = list_items(heap, clause) else {
            return;
        };
        let Some(&pattern) = clause_items.first() else {
            return;
        };
        let guarded = matches!(clause_items.get(1),
            Some(&Value::Keyword(marker)) if value::symbol_is(marker, "when"));
        let exact = exact_pattern_ty(heap, pattern, &mut std::collections::HashSet::new());
        if exact.as_ref().is_some_and(Ty::is_any) && !guarded {
            return; // a catch-all: the compiler rejects whatever follows it
        }
        let structural = matches!(
            pattern,
            Value::Keyword(_) | Value::Int(_) | Value::Str(_) | Value::Bool(_) | Value::Nil
        );
        if let Some(ty) = &exact
            && !structural
            && !earlier.is_empty()
            && covered_by(ty, &earlier)
        {
            out.push(Finding::new(
                Lint::UnreachableClause,
                heap.form_pos_only(clause),
                format!(
                    "unreachable clause: {} — every value it matches is matched by an earlier clause",
                    crate::syntax::printer::print(heap, pattern)
                ),
            ));
        }
        if !guarded && let Some(ty) = exact {
            earlier.push(ty);
        }
    }
}

/// The most terms [`covered_by`] unions before it stops trusting the union to be exact. The
/// lattice keeps four alternatives before its widening merge (`union_of_terms.rs`).
const MAX_EXACT_UNION: usize = 4;

/// Whether `ty` lies inside the union of `earlier` — read through the union only while it is
/// exact (see [`check_redundant_shapes`]), else against each earlier type alone.
fn covered_by(ty: &Ty, earlier: &[Ty]) -> bool {
    let terms: usize = earlier.iter().map(|prior| prior.terms_vec().len()).sum();
    if terms <= MAX_EXACT_UNION {
        let union = earlier
            .iter()
            .cloned()
            .reduce(Ty::union)
            .unwrap_or(Ty::NEVER);
        return ty.is_subtype(&union);
    }
    earlier.iter().any(|prior| ty.is_subtype(prior))
}

/// The EXACT set of values a `match` pattern admits, or `None` when the lattice cannot say it
/// exactly (std/prelude/match.blsp's grammar): `_` and a first-seen binder admit anything; a
/// keyword, int, string, bool or `nil` literal its singleton; `(p…)` a list of exactly those
/// elements; `[p…]` a vector of exactly those; `(or p…)` the union, when its alternatives bind
/// nothing. A repeated binder is an equality constraint between positions — not a product of
/// sets — so it makes the pattern inexact, as does every other form.
fn exact_pattern_ty(
    heap: &Heap,
    pattern: Value,
    seen: &mut std::collections::HashSet<value::Symbol>,
) -> Option<Ty> {
    match pattern {
        Value::Sym(name) if value::symbol_is(name, "_") => Some(Ty::ANY),
        Value::Sym(name) if value::symbol_is(name, "&") || value::symbol_is(name, "&optional") => {
            None
        }
        Value::Sym(binder) => seen.insert(binder).then_some(Ty::ANY),
        Value::Keyword(_) | Value::Int(_) | Value::Bool(_) | Value::Nil => {
            Some(Ty::of_value(pattern))
        }
        Value::Str(id) => Some(Ty::str_lit(heap.string(id))),
        Value::Vector(id) => {
            let elems = heap.vector(id).to_vec();
            if elems.is_empty() {
                return None;
            }
            let tys = elems
                .into_iter()
                .map(|elem| exact_pattern_ty(heap, elem, seen))
                .collect::<Option<Vec<Ty>>>()?;
            Some(Ty::tuple_of(tys))
        }
        Value::Pair(_) => {
            let items = list_items(heap, pattern)?;
            if let Some(&Value::Sym(head)) = items.first() {
                let name = value::symbol_name_ref(head);
                if name == "or" {
                    // Alternatives that bind nothing — literals and wildcards — union exactly,
                    // while the union stays below the lattice's widening cap: past it, two
                    // alternatives merge position-wise (`[:d 4] ∪ [:e 5]` → `[(:d|:e) (4|5)]`),
                    // which admits values the pattern does not match.
                    let mut alts = Vec::new();
                    for &alt in &items[1..] {
                        let mut alt_seen = std::collections::HashSet::new();
                        let ty = exact_pattern_ty(heap, alt, &mut alt_seen)?;
                        if !alt_seen.is_empty() {
                            return None;
                        }
                        alts.push(ty);
                    }
                    let terms: usize = alts.iter().map(|alt| alt.terms_vec().len()).sum();
                    if terms > MAX_EXACT_UNION {
                        return None;
                    }
                    return alts.into_iter().reduce(Ty::union);
                }
                if matches!(
                    name,
                    "quote" | "%pin" | "record" | "and" | "not" | "unquote" | "bytes"
                ) {
                    return None;
                }
            }
            let tys = items
                .into_iter()
                .map(|item| exact_pattern_ty(heap, item, seen))
                .collect::<Option<Vec<Ty>>>()?;
            Some(Ty::list_shape_of(tys))
        }
        _ => None,
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
    out: &mut Vec<Finding>,
) {
    let mut covered: BTreeSet<String> = BTreeSet::new();
    for &clause in clauses {
        let Some(clause_items) = list_items(heap, clause) else {
            return;
        };
        let Some(&pat) = clause_items.first() else {
            return;
        };
        // A guard means the clause is not guaranteed to fire; reasoning past it is unsound.
        if matches!(clause_items.get(1), Some(&Value::Keyword(k)) if value::symbol_is(k, "when")) {
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
            out.push(Finding::new(
                Lint::NonExhaustive,
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

/// The special forms and macros whose operands are evaluated in the enclosing scope — they
/// bind no name, so a scrutinee's type passes through them unchanged.
const NON_BINDING_HEADS: &[&str] = &[
    "if", "do", "when", "cond", "and", "or", "not", "->", "->>", "throw",
];

/// Does a form with this head bind nothing? A non-macro function (a global or a local) is
/// called with evaluated arguments, and the [`NON_BINDING_HEADS`] are known. Anything else —
/// a binding macro (`if-let`, `for`, `try`), an unknown head, a clause list `((s) body)` —
/// may bind, and the caller forgets the names it mentions.
fn binds_nothing(heap: &Heap, ctx: &Ctx, head: Value) -> bool {
    let Value::Sym(head) = head else {
        return false;
    };
    if NON_BINDING_HEADS
        .iter()
        .any(|&name| value::symbol_is(head, name))
    {
        return true;
    }
    if resolves_to_macro(heap, ctx, head) {
        return false;
    }
    ctx.is_local(head) || super::deps::obs_global(heap, head).is_some()
}

/// `ctx` with every symbol mentioned anywhere in `forms` re-bound with no type — the
/// sound answer to a binder this pass cannot read: a name it might rebind keeps no type,
/// rather than the OUTER binding's type (which is what manufactured the false
/// `sealed match` warnings through a clause binder, `if-let`, `for` or `catch`).
fn forget_symbols_in(heap: &Heap, ctx: &Ctx, forms: &[Value]) -> Ctx {
    let mut symbols = BTreeSet::new();
    let mut pending: Vec<Value> = forms.to_vec();
    while let Some(form) = pending.pop() {
        match form {
            Value::Sym(symbol) => {
                symbols.insert(symbol);
            }
            Value::Pair(_) | Value::Nil => {
                pending.extend(list_items(heap, form).unwrap_or_default());
            }
            Value::Vector(id) => pending.extend(heap.vector(id).iter().copied()),
            Value::Map(id) => {
                for (key, entry) in heap.map_entries(id) {
                    pending.push(key);
                    pending.push(entry);
                }
            }
            _ => {}
        }
    }
    let mut scope = ctx.clone();
    for symbol in symbols {
        // Only a name the scope knows can carry a stale type; re-binding every symbol
        // would clone the scope once per name in a large macro form.
        if scope.get(symbol).is_some() || scope.is_local(symbol) {
            scope = scope.bind(symbol, None);
        }
    }
    scope
}
