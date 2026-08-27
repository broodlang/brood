//! Macro support: quasiquote expansion and `macroexpand`. Heap-threaded.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes, `~x` splices a value, `~@xs`
//! splices the elements of a sequence. Nested quasiquote is not level-tracked
//! (v0.1) — unquotes resolve at the first enclosing quasiquote.
//!
//! Quasiquote is a **compile-time / eval-time code transform**, not a runtime
//! walker: [`expand_quasiquote`] rewrites a template into *builder code*
//! (`` `(a ~b ~@c) `` → `(append (list 'a) (list b) c)`), and the normal
//! evaluator runs that. The transform never re-enters `eval`, so — unlike the
//! old walker, which evaluated unquotes inline while accumulating LOCAL
//! transients in Rust (the GC-rooting hazard, ADR-084) — it hits no safepoint
//! and needs no operand-stack rooting; the unquoted sub-forms are rooted by the
//! evaluator as ordinary `list`/`append` operands.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, ClosureId, EnvId, Symbol, Value, ValueRef};
use crate::error::{LispError, LispResult};
use std::collections::{HashMap, HashSet};

/// Bound on recursion depth for the quasiquote walker and the compile pass.
/// Past this, return `LispError::runtime` rather than overflowing the native
/// Rust stack — a deeply nested template, vector, or map (from a user file or
/// a misbehaving macro) should produce a clean error, not abort the process.
const MAX_DEPTH: u32 = 256;

/// Bound on `macroexpand`'s head-fixpoint *rounds* — a different quantity from
/// the nesting depth above (it counts successive whole-form rewrites, not
/// recursion), kept as its own constant so tuning one never silently retunes
/// the other. The prelude's Brood-level `macroexpand` mirrors this value
/// (`macroexpand--max-rounds` in `std/prelude.blsp`).
const MAX_EXPAND_ROUNDS: u32 = 256;

/// Per-expansion auto-gensym table (Clojure-style `x#`). Maps a literal template
/// symbol whose name ends in `#` to a single fresh gensym, so every occurrence of
/// that name *within one backtick expansion* refers to the same fresh symbol — and
/// two expansions (or two macro uses) get distinct ones. Holds only interned
/// `Value::Sym`s and `Symbol` (`u32`) keys, both GC-immune (symbols never move and
/// ship by name), so it needs no operand-stack rooting even though quasiquote runs
/// with collection enabled (ADR-061). See `maybe_autogensym`.
type AutoGen = HashMap<Symbol, Value>;

/// Expand a quasiquote template into **builder code** — a pure structural
/// transform that never re-enters `eval`. `` `(a ~b ~@c) `` becomes
/// `(append (list 'a) (list b) c)`; evaluating that builder code reconstructs
/// the template with `~unquote` values inlined and `~@unquote-splicing`
/// sequences spliced. Replaces the old runtime walker (`quasiquote_depth`),
/// which evaluated unquotes inline while holding LOCAL transients in Rust — the
/// GC-rooting hazard. Here the unquoted forms become operands of `list`/`append`
/// that the normal evaluator roots, and this transform itself touches no
/// safepoint (it calls no `eval`), so its own transients are stable without
/// rooting.
///
/// Auto-gensym (`x#`) resolves to a fresh symbol here, once per template symbol
/// per expansion. The enclosing macro body is re-evaluated on every application,
/// so each expansion gets distinct gensyms — Clojure-style binding hygiene.
pub fn expand_quasiquote(heap: &mut Heap, template: Value) -> LispResult {
    // Automatic binding hygiene (ADR-066 amendment): alpha-rename the template's own
    // `let`/`fn` binders to fresh gensyms so they can't capture spliced caller code.
    // A no-op for a binder-free template. Runs before `qq_elem` builds the template.
    let template = hygiene_rename(heap, template);
    let mut autogen = AutoGen::new();
    qq_elem(heap, template, 0, &mut autogen)
}

/// Clojure-style auto-gensym: a literal template symbol whose name ends in `#`
/// (e.g. `tmp#`) becomes a fresh gensym, consistently for every occurrence within
/// one backtick expansion (tracked in `autogen`). This is opt-in binding hygiene —
/// a macro-introduced binding named `tmp#` can neither capture nor be captured by
/// the caller's `tmp`. Only *literal* template symbols reach here; symbols inside
/// `~unquote` go through `eval` instead, so a user's `x#` in unquoted code is left
/// alone. A bare `#` (no prefix) is not rewritten.
fn maybe_autogensym(v: Value, autogen: &mut AutoGen) -> Value {
    if let ValueRef::Sym(s) = v.unpack() {
        let name = value::symbol_name_ref(s);
        if name.len() > 1 && name.ends_with('#') {
            return *autogen
                .entry(s)
                .or_insert_with(|| value::gensym(&name[..name.len() - 1]));
        }
    }
    v
}

/// A symbol Value for a builder-code head the transform emits (`list`,
/// `append`, `vector`, `hash-map`, `apply`). Interning dedups, so this is cheap.
fn sym(name: &str) -> Value {
    Value::symbol(value::intern(name))
}

/// `(quote v)` — the builder form that reproduces a literal symbol/atom datum.
fn quote_form(heap: &mut Heap, v: Value) -> Value {
    heap.list(vec![sym(kw::QUOTE), v])
}

/// Builder code for one template position. `~x` becomes `x` (evaluated in place
/// by the normal evaluator); a list/vector/map recurses; a literal symbol is
/// quoted (after auto-gensym rewriting `x#`); a self-evaluating atom is emitted
/// verbatim.
fn qq_elem(heap: &mut Heap, v: Value, depth: u32, autogen: &mut AutoGen) -> LispResult {
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "quasiquote template nested too deeply (math/max {} levels)",
            MAX_DEPTH
        )));
    }
    // ~x → x : the unquoted form is evaluated in place when the builder runs.
    if let Some(inner) = tagged(heap, v, kw::UNQUOTE) {
        return Ok(inner);
    }
    // ~@x at a non-sequence position has nothing to splice into — `qq_seq`
    // handles splices inline, so reaching here means a top-level `~@`. Reject it
    // rather than silently mis-building `(list 'unquote-splicing x)`.
    if tagged(heap, v, kw::UNQUOTE_SPLICING).is_some() {
        return Err(LispError::runtime(
            "unquote-splicing (~@) outside a list/vector context",
        ));
    }
    // A **nested** quasiquote in template position. Levels are not tracked, so an
    // `~unquote` inside the inner template would be expanded at the OUTER level —
    // `` `(a `(b ~(+ 1 2))) `` silently produced `(a (quasiquote (b 3)))` where the
    // standard reading leaves `(+ 1 2)` unevaluated at level 2. Reject it rather
    // than quietly compute the wrong thing; level tracking can land later without
    // breaking anything this accepts (ADR-011: defer the power feature).
    //
    // Only *template* position: a `` ` `` inside an `~unquote` is ordinary code
    // (the unquote returns above, before this walk descends), so the common
    // macro-writing-a-macro spelling — unquoting a helper that builds the inner
    // template — keeps working.
    if tagged(heap, v, kw::QUASIQUOTE).is_some() {
        return Err(LispError::runtime(
            "nested quasiquote (a ` inside a ` template) is not supported",
        )
        .with_hint(
            "quasiquote levels are not tracked, so an inner `~x` would be expanded at \
             the outer level. Build the inner template from the outside instead — put \
             the ` in an unquoted position, e.g. `` `(a ~(inner-template x)) ``, where \
             `inner-template` is a fn/macro that returns the inner form — or assemble \
             it with `list`/`quote`.",
        ));
    }
    match v.unpack() {
        ValueRef::Pair(_) => {
            let items = heap.list_to_vec(v)?;
            qq_seq(heap, &items, false, depth + 1, autogen)
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            qq_seq(heap, &items, true, depth + 1, autogen)
        }
        ValueRef::Map(id) => {
            // No `~@` splicing into a map (ill-defined); expand each key/value.
            let entries = heap.map_entries(id);
            let mut out = Vec::with_capacity(entries.len() * 2 + 1);
            out.push(sym("hash-map"));
            for (k, val) in entries {
                out.push(qq_elem(heap, k, depth + 1, autogen)?);
                out.push(qq_elem(heap, val, depth + 1, autogen)?);
            }
            Ok(heap.list(out))
        }
        // A set template builds a `(%set e…)` — the set counterpart of the map
        // arm above. Without this arm `#{…}` fell through to the verbatim
        // catch-all, so the *evaluator* evaluated the set's elements instead of
        // the template quoting them: `` `#{a} `` silently gave `#{5}` where
        // `` `(a) ``/`` `[a] `` correctly give the symbol, and `` `#{~x} `` died
        // with "unbound symbol: unquote". No `~@` splicing into a set, same as a
        // map — `qq_elem` rejects a stray `~@` for us.
        ValueRef::Set(id) => {
            let elems = heap.map_entries(id);
            let mut out = Vec::with_capacity(elems.len() + 1);
            out.push(sym("%set"));
            for (e, _) in elems {
                out.push(qq_elem(heap, e, depth + 1, autogen)?);
            }
            Ok(heap.list(out))
        }
        // A literal symbol is data — quote it (auto-gensym `x#` first).
        ValueRef::Sym(_) => {
            let sv = maybe_autogensym(v, autogen);
            Ok(quote_form(heap, sv))
        }
        // Self-evaluating atoms (int/float/string/keyword/bool/nil) emit verbatim.
        other => Ok(other),
    }
}

/// Builder code for a sequence template (`is_vector` chooses list vs vector).
/// With no `~@` splice it is a flat `(list e…)` / `(vector e…)`. With a splice
/// it is `(append (list e) <spliced-seq> …)`, and for a vector that assembled
/// list is turned back into a vector with `(apply vector …)`. `append` is the
/// seq-generic concatenation, so a spliced vector/list/map flattens uniformly,
/// exactly as the old walker's `seq_items` did.
fn qq_seq(
    heap: &mut Heap,
    items: &[Value],
    is_vector: bool,
    depth: u32,
    autogen: &mut AutoGen,
) -> LispResult {
    let has_splice = items
        .iter()
        .any(|&it| tagged(heap, it, kw::UNQUOTE_SPLICING).is_some());
    if !has_splice {
        let mut out = Vec::with_capacity(items.len() + 1);
        out.push(if is_vector {
            sym("vector")
        } else {
            sym("list")
        });
        for &it in items {
            out.push(qq_elem(heap, it, depth, autogen)?);
        }
        return Ok(heap.list(out));
    }
    let mut segs = Vec::with_capacity(items.len() + 1);
    segs.push(sym("append"));
    for &it in items {
        if let Some(inner) = tagged(heap, it, kw::UNQUOTE_SPLICING) {
            segs.push(inner); // splice the sequence's elements in place
        } else {
            let e = qq_elem(heap, it, depth, autogen)?;
            let one = heap.list(vec![sym("list"), e]);
            segs.push(one);
        }
    }
    let appended = heap.list(segs);
    if is_vector {
        Ok(heap.list(vec![sym("apply"), sym("vector"), appended]))
    } else {
        Ok(appended)
    }
}

/// If `v` is a two-element list `(name x)` with the given head symbol, return `x`.
fn tagged(heap: &Heap, v: Value, name: &str) -> Option<Value> {
    if let ValueRef::Pair(p) = v.unpack() {
        let (head, tail) = heap.pair(p);
        if let ValueRef::Sym(s) = head.unpack() {
            if value::symbol_is(s, name) {
                if let ValueRef::Pair(p2) = tail.unpack() {
                    return Some(heap.car(p2));
                }
            }
        }
    }
    None
}

// ============================================================================
// Automatic binding hygiene (ADR-066 amendment — "Option A")
// ============================================================================
//
// A quasiquote template's OWN lexical binders — the `let`/`letrec`/`fn` binders it
// introduces as *literal* symbols — are alpha-renamed to fresh gensyms before the
// template is built, so a template binder can neither capture nor be captured by
// caller code spliced in via `~`/`~@`. Hygiene is thus the DEFAULT: a macro no
// longer needs `x#` or `(gensym)` for a safe temp binding.
//
// - Free-reference hygiene (a template's `helper`/`map` resolving to the *defining*
//   namespace) is already handled by the auto-qualifying resolver (ADR-065 §7); this
//   closes the remaining half — *introduced-binding* capture (ADR-066 concern #2) —
//   without the per-symbol lexical context (fat `Value::Sym`) full Scheme hygiene
//   needs, which ADR-066 rejected on ship-by-name/homoiconicity/GC grounds.
// - Scope-aware: a binder renames only the references it actually binds, so a
//   same-named prelude reference elsewhere in the template is untouched (correct
//   even when a binder shadows a prelude name). Hence a scope map, not `#`'s flat
//   name→gensym table.
// - Fresh per expansion: a template that introduces a renamable binder takes the
//   runtime expand path (like `#`), so two nested expansions of one macro —
//   `(m (m x))` — get distinct binders. `template_introduces_binder` gates the
//   static-quasiquote optimisation accordingly.
// - Opt-out for intentional anaphora (a name the template deliberately exposes to
//   the caller, e.g. `it` in an `aif`, or `defseq`'s `item`/`acc`): write `~'it` —
//   an unquoted quoted symbol, which lands in a `~unquote` hole the rename never
//   descends, emitting a literal `it`.
//
// v1 scope: only `let`/`letrec`/`fn` PLAIN-SYMBOL binders are renamed. Destructuring
// binders, `match*` pattern binders, and computed (`~params`/`~bindings`) or `defn`
// binders inside a template stay literal — a sound under-approximation (leaving a
// binder un-renamed only preserves the pre-change capturable-but-explicit behaviour;
// it never miscompiles a real macro), opt into `#`/`(gensym)` there as before. The
// one documented non-soundness is the pathological case of an outer template binder
// shadowed by a *computed* (`~params`/`~bindings`) inner binder of the same name.

/// A hygiene scope frame: original binder symbol → its replacement in the output.
/// A replacement equal to the original means "bound here, but not renamed" (a
/// non-plain-symbol or `#`/`_`/`&`/qualified binder) — it still SHADOWS an outer
/// rename so an inner reference stays literal.
type HygScope = Vec<(value::Symbol, value::Symbol)>;

/// Look up `s` (innermost — last — binding wins).
fn hyg_lookup(scope: &[(value::Symbol, value::Symbol)], s: value::Symbol) -> Option<value::Symbol> {
    scope
        .iter()
        .rev()
        .find(|(orig, _)| *orig == s)
        .map(|(_, t)| *t)
}

/// A plain binder we rename: not `_`, not a `&`-marker, not `#`-suffixed
/// (auto-gensym owns those), not already qualified.
fn hyg_renamable(s: value::Symbol) -> bool {
    let n = value::symbol_name_ref(s);
    !(n == "_" || n.starts_with('&') || n.contains('/') || (n.len() > 1 && n.ends_with('#')))
}

/// A fresh replacement symbol for a binder named like `orig`.
fn hyg_fresh(orig: value::Symbol) -> value::Symbol {
    let name = value::symbol_name(orig); // owned, so gensym can re-enter the interner
    match value::gensym(&name).unpack() {
        ValueRef::Sym(g) => g,
        _ => orig, // gensym always yields a Sym
    }
}

/// True if `v` is `(unquote …)` / `(unquote-splicing …)` — a computed hole whose
/// binders (`(let ~bindings …)` / `(fn ~params …)`) aren't literal template
/// structure, so we can't rename them.
fn hyg_is_unquote(heap: &Heap, v: Value) -> bool {
    tagged(heap, v, kw::UNQUOTE).is_some() || tagged(heap, v, kw::UNQUOTE_SPLICING).is_some()
}

/// The hygiene pre-pass: alpha-rename a template's introduced binders. A no-op
/// (returning the template unchanged, no allocation) when it introduces none — the
/// common static case. GC-blocked otherwise because it builds a parallel template
/// tree, exactly like `resolve`.
fn hygiene_rename(heap: &mut Heap, template: Value) -> Value {
    if !template_introduces_binder(heap, template) {
        return template;
    }
    let _gc = crate::process::GcBlockGuard::enter();
    let _macro = crate::process::MacroBlockGuard::enter();
    hyg_walk(heap, template, &[])
}

fn hyg_walk(heap: &mut Heap, v: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || hyg_walk_inner(heap, v, scope))
}

fn hyg_walk_inner(heap: &mut Heap, v: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    match v.unpack() {
        ValueRef::Sym(s) => match hyg_lookup(scope, s) {
            Some(target) => Value::symbol(target),
            None => v,
        },
        ValueRef::Pair(_) => hyg_list(heap, v, scope),
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(hyg_walk(heap, it, scope));
            }
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, val) in entries {
                let k = hyg_walk(heap, k, scope);
                let val = hyg_walk(heap, val, scope);
                pairs.push((k, val));
            }
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items {
                out.push(hyg_walk(heap, e, scope));
            }
            heap.set_from_elems(out)
        }
        _ => v,
    }
}

fn hyg_list(heap: &mut Heap, form: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return form, // improper list — leave verbatim
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
        // Data / holes — never descend for renaming: `quote` is data;
        // `unquote`/`unquote-splicing` are caller code resolved in the caller's
        // scope (also where `~'it` — the anaphora opt-out — lands, emitting a
        // literal `it`); a nested `quasiquote` is another template (rejected later).
        if value::symbol_is(h, kw::QUOTE)
            || value::symbol_is(h, kw::UNQUOTE)
            || value::symbol_is(h, kw::UNQUOTE_SPLICING)
            || value::symbol_is(h, kw::QUASIQUOTE)
        {
            return form;
        }
        if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
            return hyg_let(heap, form, &items, scope);
        }
        if value::symbol_is(h, kw::FN) {
            return hyg_fn(heap, form, &items, scope);
        }
    }
    // Generic: rename references in every position (head included) under the same
    // scope. A `match*`/`def`/`defn` etc. rides here — a nested binder that matches
    // an enclosing rename stays consistent (shadowing preserved); one with no
    // enclosing rename stays literal (v1 under-approx).
    hyg_generic(heap, form, &items, scope)
}

fn hyg_generic(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let mut out = Vec::with_capacity(items.len());
    for &it in items {
        out.push(hyg_walk(heap, it, scope));
    }
    rebuild_list(heap, form, out)
}

/// Add scope entries for a binder position (a `let`/`fn` binder).
fn hyg_bind(heap: &Heap, target: Value, scope: &mut HygScope) {
    match target.unpack() {
        ValueRef::Sym(s) if hyg_renamable(s) => scope.push((s, hyg_fresh(s))),
        ValueRef::Sym(s) => scope.push((s, s)), // `_`/`&rest`/`x#`/qualified — shadow, keep literal
        _ => {
            // A pattern binder (vector/map/list). Shadow every name it binds so an
            // inner reference stays literal, but don't rename it (v1 under-approx).
            let mut names = Vec::new();
            collect_all_syms(heap, target, &mut names);
            for s in names {
                scope.push((s, s));
            }
        }
    }
}

/// The binder position for the output: a renamed symbol for a plain binder, else
/// the pattern verbatim (v1 doesn't rewrite pattern internals).
fn hyg_binder_out(target: Value, scope: &[(value::Symbol, value::Symbol)]) -> Value {
    match target.unpack() {
        ValueRef::Sym(s) => Value::symbol(hyg_lookup(scope, s).unwrap_or(s)),
        _ => target,
    }
}

/// `(let/letrec (b0 v0 …) body…)` — rename plain-symbol binders. `letrec` binders
/// are all in scope for every RHS and the body; plain `let` is sequential (a binder
/// scopes only the *later* RHSs and the body — matching `resolve_let`).
fn hyg_let(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let letrec = matches!(items.first().map(|v| v.unpack()),
        Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::LETREC));
    let binds_form = items.get(1).copied().unwrap_or(Value::nil());
    // A computed binding list (`(let ~bindings …)`) — can't see its binders; recurse
    // generically (the `~bindings` hole is left verbatim, the body under `scope`).
    if hyg_is_unquote(heap, binds_form) {
        return hyg_generic(heap, form, items, scope);
    }
    let binds = match form_items(heap, binds_form) {
        Some(b) if b.len() % 2 == 0 => b,
        _ => return hyg_generic(heap, form, items, scope),
    };
    let mut new_scope = scope.to_vec();
    if letrec {
        for &t in binds.iter().step_by(2) {
            hyg_bind(heap, t, &mut new_scope);
        }
    }
    let mut new_binds = Vec::with_capacity(binds.len());
    let mut i = 0;
    while i < binds.len() {
        let target = binds[i];
        let rhs = hyg_walk(heap, binds[i + 1], &new_scope);
        if !letrec {
            hyg_bind(heap, target, &mut new_scope); // sequential: bind AFTER the RHS
        }
        new_binds.push(hyg_binder_out(target, &new_scope));
        new_binds.push(rhs);
        i += 2;
    }
    let new_bind_form = rebuild_seq_like(heap, binds_form, new_binds);
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]);
    out.push(new_bind_form);
    for &b in items.get(2..).unwrap_or(&[]) {
        out.push(hyg_walk(heap, b, &new_scope));
    }
    rebuild_list(heap, form, out)
}

/// `(fn …)` — single-arity `(params body…)` or multi-arity `(doc? (params body…)…)`.
/// Params bind together in their body. Mirrors `resolve_fn`'s dispatch.
fn hyg_fn(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let parts = &items[1..];
    let (has_doc, clause_start) = match parts.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if parts.len() > 1 => (true, 1),
        _ => (false, 0),
    };
    let clauses = &parts[clause_start..];
    let multi = !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // fn head
    if multi {
        if has_doc {
            out.push(parts[0]);
        }
        for &clause in clauses {
            out.push(hyg_arity_clause(heap, clause, scope));
        }
    } else {
        let params = parts.first().copied().unwrap_or(Value::nil());
        let mut inner = scope.to_vec();
        out.push(hyg_param_list(heap, params, &mut inner));
        for &b in parts.get(1..).unwrap_or(&[]) {
            out.push(hyg_walk(heap, b, &inner));
        }
    }
    rebuild_list(heap, form, out)
}

fn hyg_arity_clause(
    heap: &mut Heap,
    clause: Value,
    scope: &[(value::Symbol, value::Symbol)],
) -> Value {
    let cparts = match heap.list_to_vec(clause) {
        Ok(c) if !c.is_empty() => c,
        _ => return clause,
    };
    let mut inner = scope.to_vec();
    let new_params = hyg_param_list(heap, cparts[0], &mut inner);
    let mut out = Vec::with_capacity(cparts.len());
    out.push(new_params);
    for &b in &cparts[1..] {
        out.push(hyg_walk(heap, b, &inner));
    }
    rebuild_list(heap, clause, out)
}

/// Rename plain-symbol params, extending `inner` with them (all bound together for
/// the body). A computed param list (`~params`) is left verbatim.
fn hyg_param_list(heap: &mut Heap, params: Value, inner: &mut HygScope) -> Value {
    if hyg_is_unquote(heap, params) {
        return params; // `(fn ~params …)` — binders not visible
    }
    let elems = match form_items(heap, params) {
        Some(e) => e,
        None => return params,
    };
    let mut out = Vec::with_capacity(elems.len());
    for p in elems {
        hyg_bind(heap, p, inner);
        out.push(hyg_binder_out(p, inner));
    }
    rebuild_seq_like(heap, params, out)
}

/// True if `v` contains a `let`/`letrec`/`fn` that introduces a plain-symbol binder
/// the hygiene pass would rename — so the static-quasiquote optimisation must defer
/// to the runtime expand path (fresh gensyms per expansion). Skips `quote`/`unquote`/
/// `quasiquote` subtrees (a `let` in caller code or quoted data isn't the template's).
fn template_introduces_binder(heap: &Heap, v: Value) -> bool {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        template_introduces_binder_inner(heap, v)
    })
}

fn template_introduces_binder_inner(heap: &Heap, v: Value) -> bool {
    match v.unpack() {
        ValueRef::Pair(_) => {
            let items = match heap.list_to_vec(v) {
                Ok(i) => i,
                Err(_) => return false,
            };
            if let Some(ValueRef::Sym(h)) = items.first().map(|x| x.unpack()) {
                if value::symbol_is(h, kw::QUOTE)
                    || value::symbol_is(h, kw::UNQUOTE)
                    || value::symbol_is(h, kw::UNQUOTE_SPLICING)
                    || value::symbol_is(h, kw::QUASIQUOTE)
                {
                    return false;
                }
                if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
                    let binds_form = items.get(1).copied().unwrap_or(Value::nil());
                    if !hyg_is_unquote(heap, binds_form) {
                        if let Some(binds) = form_items(heap, binds_form) {
                            if binds.iter().step_by(2).any(hyg_binder_renamable) {
                                return true;
                            }
                        }
                    }
                }
                if value::symbol_is(h, kw::FN) && fn_has_renamable_param(heap, &items) {
                    return true;
                }
            }
            items.iter().any(|&it| template_introduces_binder(heap, it))
        }
        ValueRef::Vector(id) => heap
            .vector(id)
            .to_vec()
            .iter()
            .any(|&it| template_introduces_binder(heap, it)),
        ValueRef::Map(id) => heap.map_entries(id).iter().any(|(k, val)| {
            template_introduces_binder(heap, *k) || template_introduces_binder(heap, *val)
        }),
        ValueRef::Set(id) => heap
            .set_elems(id)
            .iter()
            .any(|&e| template_introduces_binder(heap, e)),
        _ => false,
    }
}

/// A binder position that hyg would rename: a plain renamable symbol.
fn hyg_binder_renamable(t: &Value) -> bool {
    matches!(t.unpack(), ValueRef::Sym(s) if hyg_renamable(s))
}

/// Does any clause of this `fn` form have a renamable plain-symbol param?
fn fn_has_renamable_param(heap: &Heap, items: &[Value]) -> bool {
    let parts = &items[1..];
    let clause_start = matches!(parts.first().map(|v| v.unpack()), Some(ValueRef::Str(_)) if parts.len() > 1)
        as usize;
    let clauses = &parts[clause_start..];
    if !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f)) {
        clauses.iter().any(|&c| {
            heap.list_to_vec(c)
                .ok()
                .and_then(|cp| cp.first().copied())
                .is_some_and(|params| param_list_has_renamable(heap, params))
        })
    } else {
        parts
            .first()
            .copied()
            .is_some_and(|params| param_list_has_renamable(heap, params))
    }
}

fn param_list_has_renamable(heap: &Heap, params: Value) -> bool {
    if hyg_is_unquote(heap, params) {
        return false;
    }
    form_items(heap, params).is_some_and(|elems| elems.iter().any(hyg_binder_renamable))
}

// ============================================================================
// Namespace resolution (ADR-065)
// ============================================================================
//
// Rewrite a *macroexpanded* top-level form against the current namespace
// (`heap.compile_ns`): qualify definition heads and free references to `ns/name`.
// Runs after `macroexpand_all`, before `eval`. At root (`compile_ns == None`) it
// is an identity no-op (one branch) — so the prelude and all non-namespaced code
// are untouched; only a file that opened `(ns …)` pays for the walk.
//
// Safety invariant: NEVER rewrite a binder/param/pattern position. Over-qualifying
// a local (treating a bound name as free) is a *silent* miscompile; under-qualifying
// a genuine reference is at worst a loud unbound error. So when the binder shape is
// uncertain (e.g. `match*` patterns, `&optional` defaults) we over-approximate the
// bound set and leave those positions verbatim — safe, occasionally incomplete.
// Data is inviolate: `quote`/`quasiquote` are skipped wholesale (a quoted symbol is
// a message tag / map key that travels by name across processes — ADR-034).

/// RAII scope for the per-file namespace-compilation state on the `Heap` — the four
/// fields that together decide how a bare reference resolves: `compile_ns`,
/// `ns_known_names`, `imports`, and `ns_assume_own`. A file loader (`load`,
/// `reload`, the file runner, `%load-string`) creates one before evaluating a file's
/// forms; **dropping it restores all four to the caller's values on every exit path —
/// normal return, an early `?`, or a panic unwinding through the loader** — so
/// namespace state can never leak across a file boundary and the four fields can never
/// fall out of sync (the hand-written save/restore this replaces bracketed only three,
/// leaking `ns_assume_own` out of `load`/`reload`/the file runner — a nested load then
/// wrongly inherited an interactive `eval`'s assume-own). It **owns** the `&mut Heap`
/// for its lifetime, so it is sound with no `unsafe`; reach the heap through
/// [`heap`](NsLoadScope::heap).
pub struct NsLoadScope<'a> {
    heap: &'a mut Heap,
    compile_ns: Option<Symbol>,
    known: HashSet<Symbol>,
    known_by_module: std::collections::HashMap<Symbol, HashSet<Symbol>>,
    imports: std::collections::HashMap<Symbol, crate::core::heap::ImportEntry>,
    assume_own: bool,
}

impl<'a> NsLoadScope<'a> {
    /// Save the caller's ns-state, then reset into a freshly-`load`ed file's scope: the
    /// ROOT namespace, an empty import table, this file's forward-reference pre-scan
    /// (only when the file opens a namespace — cheap otherwise), and `ns_assume_own`
    /// **off** (a loaded file carries a real pre-scan; a nested load must not inherit an
    /// interactive `eval`'s assume-own fallback). The saved state is restored on drop.
    pub fn enter(heap: &'a mut Heap, forms: &[Value]) -> Self {
        // The per-module forward-ref pre-scan (ADR-223). The *active* `ns_known_names`
        // starts empty: `compile_ns` is `None` until a `defmodule`'s `%in-ns` runs, and
        // resolution is a no-op at root, so no form is resolved before its region is
        // activated — `%in-ns` then switches the active set to the module it opens.
        let by_module = if file_opens_ns(heap, forms) {
            scan_regions(heap, forms)
        } else {
            std::collections::HashMap::new()
        };
        let compile_ns = heap.set_compile_ns(None);
        let known = heap.set_ns_known_names(HashSet::new());
        let known_by_module = heap.set_ns_known_by_module(by_module);
        let imports = heap.set_imports(std::collections::HashMap::new());
        let assume_own = heap.set_ns_assume_own(false);
        Self {
            heap,
            compile_ns,
            known,
            known_by_module,
            imports,
            assume_own,
        }
    }

    /// The heap this scope owns for the duration of the load.
    #[inline]
    pub fn heap(&mut self) -> &mut Heap {
        self.heap
    }
}

impl Drop for NsLoadScope<'_> {
    fn drop(&mut self) {
        self.heap.set_compile_ns(self.compile_ns);
        self.heap
            .set_ns_known_names(std::mem::take(&mut self.known));
        self.heap
            .set_ns_known_by_module(std::mem::take(&mut self.known_by_module));
        self.heap.set_imports(std::mem::take(&mut self.imports));
        self.heap.set_ns_assume_own(self.assume_own);
    }
}

/// The compile pass for one top-level form: expand macros, then resolve
/// namespaces. Every loader/driver runs forms through here before `eval` so the
/// runtime evaluator never sees an unexpanded macro or an unqualified namespaced
/// reference. At root (`compile_ns == None`) the resolve step is a no-op.
pub fn compile(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // Module privacy is REAL (ADR-146): from inside a module, a hand-written
    // qualified reference to another module's `--` name is a compile error,
    // unless that module was granted with `(:use-internals mod)` (the
    // @testable seam). Enforced on the PRE-expansion source deliberately: a
    // module's macros may expand to its own private helpers inside any file
    // (the test framework's `describe`/`test` → `test/test--run` pattern) —
    // privacy governs what an author can *type*, and macro templates already
    // live behind `quasiquote`, which the walk skips. Top-level / REPL code
    // (no namespace) stays unrestricted — the live-hacking hatch hot reload
    // depends on.
    if let Some(ns) = heap.compile_ns() {
        enforce_private_refs(heap, form, &value::symbol_name(ns), None, 0)?;
    }
    let expanded = macroexpand_all(heap, form, env)?;
    // Inferred requires from qualified references (ADR-227 follow-up): arm recording so
    // `resolve` notes each qualified reference's module, then require them — before eval,
    // so `mod/name` is bound. At the root region (script/REPL) `resolve` is identity, so
    // additionally scan the form for qualified references there. (A qualified macro/call
    // head is already loaded eagerly during macroexpand.)
    let resolved = {
        let _recording = crate::eval::derive::RecordingScope::enter();
        let resolved = resolve(heap, expanded);
        if heap.compile_ns().is_none() {
            crate::eval::derive::scan_root_refs(heap, resolved);
        }
        resolved
    };
    // A bare name used in this form that is `(:use …)`-imported from two or more modules is
    // a use-site clash (ADR-235) — raise it now, at the point of use, naming the candidates.
    if let Some(error) = crate::eval::derive::take_ambiguous_error() {
        return Err(error);
    }
    // `drain_pending` loads any inferred modules, which collects — so `resolved`
    // (a LOCAL handle we still need for the quasiquote pass below) must be rooted
    // across it, or it goes stale (use-after-GC). The nested loads push and truncate
    // their own roots back to this base, so our slot stays valid throughout.
    let roots_base = heap.roots_len();
    let resolved_root = heap.root(resolved);
    if let Err(error) = crate::eval::derive::drain_pending(heap, env) {
        heap.truncate_roots(roots_base);
        return Err(error);
    }
    let resolved = heap.read_root(resolved_root);
    // Final step: expand auto-gensym-FREE `quasiquote`s into builder code so the VM
    // can compile arms that use them (a raw `quasiquote` special form otherwise
    // defers the WHOLE arm to the tree-walker — the dominant cost of macro-heavy
    // work, e.g. the advisory checker expanding every `defn`, ADR-119). Runs after
    // `resolve`, so a namespaced template's free refs are already qualified; produces
    // exactly what the runtime `expand_quasiquote` would, so behaviour is unchanged —
    // only the timing moves (once, at compile) for a template whose expansion is
    // deterministic. A `#`-autogensym template is LEFT for the runtime path (its
    // gensyms must be fresh per invocation, which a once-at-compile expansion freezes).
    let out = expand_static_quasiquotes(heap, resolved);
    heap.truncate_roots(roots_base);
    Ok(out)
}

/// The import-table key under which `(:use-internals mod)` records its grant —
/// the `%alias` trick: a leading `/` cannot arise from any real qualified
/// reference, so the key can never collide with a genuine import.
pub(crate) fn internals_grant_key(mod_name: &str) -> value::Symbol {
    value::intern(&format!("/internals/{mod_name}"))
}

/// Enforce module privacy (ADR-146) over a resolved form: an error for any
/// **evaluated** qualified reference `m/name` where `name` is recorded
/// module-private in `m` (a `defn-`/`def-` definition) and `m` is neither the
/// current namespace nor a module granted via `(:use-internals m)`. `pos` tracks
/// the nearest enclosing form's position; `level` is the quasiquote nesting depth
/// (0 = an evaluated context). A symbol is a reference only at level 0 — inside a
/// `` `quasiquote `` template it is data, UNLESS an `~unquote` brings it back to
/// level 0 (that IS an evaluated reference, so `` `(~other/secret) `` is still
/// caught). `quote` is always data. A macro template referencing its OWN module's
/// private is fine (`m == cur_ns`), so this doesn't false-flag them.
///
/// Since a private name is now spelled identically to a public one (privacy is a
/// def-site fact, not a name marker), privacy is judged against the **record**
/// (`Heap::is_private`), which requires `m` to be loaded — a reference into a
/// module that was never loaded is not recorded, so it falls through to the normal
/// unbound-reference error at eval rather than a privacy message.
fn enforce_private_refs(
    heap: &Heap,
    form: Value,
    cur_ns: &str,
    pos: Option<crate::error::Pos>,
    level: u32,
) -> Result<(), LispError> {
    match form.unpack() {
        ValueRef::Sym(s) if level == 0 => {
            let name = value::symbol_name_ref(s);
            if let Some(slash) = name.rfind('/') {
                let (m, bare) = (&name[..slash], &name[slash + 1..]);
                // Resolve a module alias (`(:alias mod :as short)`) so the rule keys
                // on the REAL module, matching how the reference will resolve.
                let alias_key = value::intern(&format!("{m}/"));
                let real_m: String = match heap.import_of(alias_key) {
                    Some(target) => value::symbol_name(target),
                    // Not an alias: root it (ADR-070). `cur_ns` is the ROOTED namespace
                    // (`%in-ns` roots what `(defmodule tutor)` declares, so inside project
                    // `bedit` it is `bedit/tutor`), so an intra-project reference must root
                    // before the comparison — otherwise `tutor/tutor-line-face`, a module
                    // reaching its OWN helper, reads as a foreign private access. Alias
                    // resolution stays first, mirroring `resolve_reference`'s order; this
                    // walk sees the pre-rewrite form, hence the rooting here as well.
                    None => match heap.root_qualified_ref(s) {
                        Some(rooted) => {
                            let rooted = value::symbol_name(rooted);
                            rooted[..rooted.rfind('/').unwrap_or(rooted.len())].to_string()
                        }
                        None => m.to_string(),
                    },
                };
                let m = real_m.as_str();
                // A reference to the current module, or one this file was granted
                // internals for, is never a foreign-private access — exit before the
                // record lookup (also the cheap common case, no `is_private` call).
                if m.is_empty() || m == cur_ns || heap.import_of(internals_grant_key(m)).is_some() {
                    return Ok(());
                }
                // Foreign module: privacy is the recorded fact (ADR-146). The name is
                // spelled identically public-or-private, so consult the record for the
                // resolved `m/name`. An unloaded `m` has no record → not rejected here
                // (it becomes a normal unbound reference at eval).
                let resolved = value::intern(&format!("{m}/{bare}"));
                if heap.is_private(resolved) {
                    let mut e = LispError::runtime(format!(
                        "`{m}/{bare}` is module-private to `{m}` (ADR-146). Call it from \
                         `{m}`, make it public (`defn`/`def` rather than `defn-`/`def-`), \
                         or — for a test/tool that genuinely needs the internals — grant \
                         access with (:use-internals {m}) in this module's header."
                    ));
                    if let Some(p) = pos {
                        e = e.with_pos(p);
                    }
                    return Err(e);
                }
            }
            Ok(())
        }
        ValueRef::Sym(_) => Ok(()), // level > 0: template data, not a reference
        ValueRef::Pair(p) => {
            let (car, cdr) = heap.pair(p);
            // Quote/quasiquote/unquote adjust the evaluated-vs-data context.
            if let ValueRef::Sym(h) = car.unpack() {
                match value::symbol_name_ref(h) {
                    // `(quote X)` at the evaluated level — X is inert data, never
                    // a reference. But INSIDE a `` `quasiquote `` (level > 0) a
                    // `(quote …)` is just a 2-element list template: the reader's
                    // quasiquote still splices any `~unquote` nested within it
                    // (`` `(quote ~(m/priv--x)) `` evaluates the unquote), so it
                    // must NOT short-circuit there — fall through and keep walking
                    // at the same level so the nested unquote is still checked.
                    "quote" if level == 0 => return Ok(()),
                    // `` `(quasiquote X) `` — X is one level deeper (more data).
                    "quasiquote" => {
                        return enforce_private_refs(heap, cdr, cur_ns, pos, level + 1);
                    }
                    // `~X` / `~@X` — one level shallower; at level 1 this returns
                    // to the evaluated context, so X's refs ARE checked.
                    "unquote" | "unquote-splicing" => {
                        return enforce_private_refs(
                            heap,
                            cdr,
                            cur_ns,
                            pos,
                            level.saturating_sub(1),
                        );
                    }
                    _ => {}
                }
            }
            let here = heap.form_pos_only(form).or(pos);
            enforce_private_refs(heap, car, cur_ns, here, level)?;
            enforce_private_refs(heap, cdr, cur_ns, here, level)
        }
        ValueRef::Vector(id) => {
            let here = heap.form_pos_only(form).or(pos);
            for &item in heap.vector(id).to_vec().iter() {
                enforce_private_refs(heap, item, cur_ns, here, level)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Does `form` contain a `#`-suffixed (auto-gensym) symbol anywhere?
fn has_autogensym(heap: &Heap, form: Value) -> bool {
    match form.unpack() {
        ValueRef::Sym(s) => {
            let n = value::symbol_name_ref(s);
            n.len() > 1 && n.ends_with('#')
        }
        ValueRef::Pair(p) => {
            let (car, cdr) = heap.pair(p);
            has_autogensym(heap, car) || has_autogensym(heap, cdr)
        }
        ValueRef::Vector(id) => heap
            .vector(id)
            .to_vec()
            .iter()
            .any(|&it| has_autogensym(heap, it)),
        ValueRef::Map(id) => heap
            .map_entries(id)
            .iter()
            .any(|(k, v)| has_autogensym(heap, *k) || has_autogensym(heap, *v)),
        // A set's elements are template positions too — skipping them meant an
        // `x#` inside `` `#{x#} `` never took the runtime (fresh-gensym) path.
        ValueRef::Set(id) => heap
            .map_entries(id)
            .iter()
            .any(|(k, _)| has_autogensym(heap, *k)),
        _ => false,
    }
}

/// See [`compile`]: rewrite every auto-gensym-free `quasiquote` into builder code.
fn expand_static_quasiquotes(heap: &mut Heap, form: Value) -> Value {
    expand_qq_rec(heap, form).0
}

/// Returns `(rewritten, changed)` — `changed` avoids rebuilding an unchanged list
/// (Value has no cheap identity compare), so a quasiquote-free tree is returned as-is.
fn expand_qq_rec(heap: &mut Heap, form: Value) -> (Value, bool) {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return (form, false), // atom / improper list — nothing to walk
    };
    if items.is_empty() {
        return (form, false);
    }
    if let ValueRef::Sym(h) = items[0].unpack() {
        if value::symbol_is(h, kw::QUOTE) {
            return (form, false); // pure data — never touch
        }
        if value::symbol_is(h, kw::QUASIQUOTE) {
            // `(quasiquote a b)` is malformed — the evaluator rejects it with an
            // arity error. Leave the form alone so it gets there: expanding only
            // `a` here would silently drop the tail (the same reasoning as the
            // compiler's `(quote a b)` deferral).
            if items.len() != 2 {
                return (form, false);
            }
            let template = items[1];
            if has_autogensym(heap, template) || template_introduces_binder(heap, template) {
                // Runtime-only: fresh gensyms per invocation — a `#` auto-gensym, or a
                // template whose own `let`/`fn` binder the hygiene pass renames (so two
                // nested expansions of one macro get distinct binders).
                return (form, false);
            }
            return match expand_quasiquote(heap, template) {
                // Recurse into the builder code so a quasiquote in an unquoted
                // sub-form is expanded too; on expander error keep the runtime form.
                Ok(builder) => (expand_static_quasiquotes(heap, builder), true),
                Err(_) => (form, false),
            };
        }
    }
    let mut out = Vec::with_capacity(items.len());
    let mut changed = false;
    for it in items {
        let (e, c) = expand_qq_rec(heap, it);
        changed |= c;
        out.push(e);
    }
    if changed {
        (heap.list(out), true)
    } else {
        (form, false)
    }
}

/// Does any top-level form open a namespace (head `ns`)? Cheap gate so the
/// forward-reference pre-scan only runs for namespaced files.
pub fn file_opens_ns(heap: &Heap, forms: &[Value]) -> bool {
    file_ns(heap, forms).is_some()
}

/// The namespace symbol a file declares via a top-level `(defmodule NAME …)`, or
/// `None`. One per file, so the first such form wins. Used by the advisory checker
/// to resolve qualified references without evaluating the header.
pub fn file_ns(heap: &Heap, forms: &[Value]) -> Option<Symbol> {
    forms.iter().find_map(|&f| defmodule_form_name(heap, f))
}

/// Every module a file declares, in source order (ADR-223: a file may open more than one
/// `(defmodule …)`). The bare names as written — `%in-ns` roots them. `file_ns` is the
/// first of these; the checker/LSP/indexer iterate the whole list.
pub fn file_modules(heap: &Heap, forms: &[Value]) -> Vec<Symbol> {
    forms
        .iter()
        .filter_map(|&f| defmodule_form_name(heap, f))
        .collect()
}

/// If `form` is a top-level `(defmodule NAME …)`, its `NAME`; else `None`.
pub(crate) fn defmodule_form_name(heap: &Heap, form: Value) -> Option<Symbol> {
    let items = heap.list_to_vec(form).ok()?;
    match (items.first()?.unpack(), items.get(1).map(|v| v.unpack())) {
        (ValueRef::Sym(h), Some(ValueRef::Sym(name))) if value::symbol_is(h, kw::DEFMODULE) => {
            Some(name)
        }
        _ => None,
    }
}

/// Pre-scan UNEXPANDED top-level forms for the bare names a file will define
/// (`def`/`defn`/`defmacro`/`defdyn` heads, recursively), so the resolver can
/// qualify a *forward* reference to a same-namespace name defined later in the
/// file. Skips `quote`/`quasiquote` data.
pub fn scan_def_names(heap: &Heap, forms: &[Value]) -> HashSet<Symbol> {
    scan_regions(heap, forms).into_values().flatten().collect()
}

/// Pre-scan UNEXPANDED top-level forms into **per-module regions** (ADR-223). Each
/// top-level `(defmodule M …)` opens a region that runs to the next `defmodule` or EOF;
/// the returned map gives, per module `M` (keyed by the **bare** name it was declared with
/// — what `%in-ns` receives), the bare def-heads defined in `M`'s region. This is the
/// region-aware core: with one module per file the sole entry equals the old whole-file
/// set (which is why [`scan_def_names`] is just its union). Names `def`ined BEFORE the
/// first `defmodule` are the **root region** — they `def` at root (`compile_ns` is still
/// `None` there) and belong to no module, so they never enter any region's set and a bare
/// reference to one correctly falls through to root instead of misqualifying to `M/name`.
pub fn scan_regions(heap: &Heap, forms: &[Value]) -> HashMap<Symbol, HashSet<Symbol>> {
    let mut by_module: HashMap<Symbol, HashSet<Symbol>> = HashMap::new();
    // `defdyn`-declared names are ambient (never namespaced), so they must not be a known
    // ns-local name in ANY region — even when a region also `def`s the knob. Collected
    // file-wide and subtracted at the end, so declaration order is irrelevant (a module
    // may read a knob near its top, declare it in the middle, and set it near the bottom).
    let mut ambient: HashSet<Symbol> = HashSet::new();
    let mut current: Option<Symbol> = None;
    for &form in forms {
        if let Some(m) = defmodule_form_name(heap, form) {
            current = Some(m);
            by_module.entry(m).or_default(); // an entry even if the region has no defs
            continue; // the `defmodule` form itself declares no module-level names
        }
        if let Some(m) = current {
            scan_def_form(heap, form, by_module.entry(m).or_default(), &mut ambient);
        }
        // else: a root-region form (before the first `defmodule`) — tracked by no module.
    }
    for names in by_module.values_mut() {
        for a in &ambient {
            names.remove(a);
        }
    }
    by_module
}

fn scan_def_form(
    heap: &Heap,
    form: Value,
    names: &mut HashSet<Symbol>,
    ambient: &mut HashSet<Symbol>,
) {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return,
    };
    let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) else {
        return;
    };
    if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
        return;
    }
    let hn = value::symbol_name_ref(h);
    if matches!(
        hn,
        kw::DEF | kw::DEF_PRIVATE | kw::DEFN | kw::DEFN_PRIVATE | kw::DEFMACRO | kw::DEFDYN
    ) {
        if let Some(ValueRef::Sym(name)) = items.get(1).map(|v| v.unpack()) {
            // An AMBIENT name is never namespaced, so it must not be pre-recorded as
            // a namespace-local name — that would qualify every reference in the file
            // to `ns/*name*` and lose the single root binding. Two ways to be
            // ambient: this form is the `defdyn` that declares it (the scan runs
            // before evaluation, so the runtime mark isn't set yet), or the name was
            // already declared dynamic elsewhere — e.g. `(def *load-path* …)` in a
            // module, rebinding the prelude's knob.
            if hn == kw::DEFDYN || value::is_dynamic(name) {
                ambient.insert(name);
            } else if !value::symbol_name_ref(name).contains('/') {
                // Only bare names get pre-recorded; an already-qualified def head
                // needs no forward-ref help.
                names.insert(name);
            }
        }
    }
    // Recurse so a def nested in a top-level `(do …)`/`(when …)` is still found.
    for &it in &items[1..] {
        scan_def_form(heap, it, names, ambient);
    }
}

/// Resolve `form` against `heap.compile_ns`. Identity when at root.
pub fn resolve(heap: &mut Heap, form: Value) -> Value {
    let ns = match heap.compile_ns() {
        Some(ns) => ns,
        None => return form,
    };
    // Bounded compile walk — block the safepoint so the partially-built output tree
    // and the Rust-local Vecs aren't relocated/swept mid-walk (resolve allocates a
    // parallel tree, like `macroexpand_all`; it re-enters neither eval nor expand).
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    let ns_name = value::symbol_name_ref(ns);
    resolve_walk(heap, form, ns_name, &[])
}

/// Resolve a single **reference** symbol `s` against the heap's current namespace
/// context (`compile_ns` + `(:use …)` imports + `ns_known_names`), exactly as the
/// compile pass's reference resolution does. This is the shared entry point the
/// **LSP** uses (ADR-065 §4) so "what does this name mean here" can never disagree
/// with the runtime: bare `observe` in a `(:use observer)` file → `observer/observe`,
/// an own-namespace def → `ns/observe`, a prelude/root or unknown name → unchanged.
/// Identity at root (`compile_ns == None`). Read-only (no allocation).
pub fn resolve_reference(heap: &Heap, s: value::Symbol) -> value::Symbol {
    match heap.compile_ns() {
        Some(ns) => resolve_sym(heap, s, value::symbol_name_ref(ns), &[]),
        None => s,
    }
}

/// An **ambient** name — one that is never namespaced, so a `def` of it in any
/// module rebinds the single *root* binding (what makes `(def *load-path* …)` from
/// anywhere change the path the loader reads).
///
/// Ambient status comes from a **declaration**, not a spelling: the name must have
/// been declared with `defdyn`. It used to be any `*earmuffed*` name (ADR-065),
/// which made an ordinary module-local constant silently global — two modules that
/// each wrote `(def *width* …)` shared one binding, and the second load clobbered
/// the first with no diagnostic. Earmuffs remain the *convention* for a knob (and
/// the checker still reads them as one), but they no longer change scoping: an
/// undeclared `(def *width* 10)` inside a module is `mod/*width*`, like every other
/// definition.
fn is_ambient(sym: value::Symbol) -> bool {
    value::is_dynamic(sym)
}

/// Qualify a definition head: `bar` -> `ns/bar`; an already-`/`-qualified name, or
/// an ambient (`defdyn`-declared) name, is taken as-is. Shared with
/// `Heap::def_form_name` so def-site keys match.
pub fn qualify_name(ns_name: &str, name: value::Symbol) -> value::Symbol {
    let spelling = value::symbol_name_ref(name);
    if spelling.contains('/') || is_ambient(name) {
        name
    } else {
        value::intern(&format!("{}/{}", ns_name, spelling))
    }
}

/// Is `s` a special form or core-macro keyword — syntax rather than a reference?
///
/// These are the names that are meaningful yet bound in **no** environment, so the
/// "bound nowhere ⇒ it must be ours" inference in [`resolve_sym`] would happily
/// rewrite `if` to `mod/if`. (Most of the list is prelude macros, which the root
/// check already covers; `if`/`fn`/`let`/… are the ones that genuinely need this.
/// Macro *calls* are gone by resolve time — `macroexpand_all` runs first — but a
/// special-form head survives expansion by definition.)
fn is_syntax_keyword(s: value::Symbol) -> bool {
    static SYNTAX: std::sync::LazyLock<crate::core::heap::SymbolMap<()>> =
        std::sync::LazyLock::new(|| {
            crate::builtins::SPECIAL_FORMS
                .iter()
                .map(|n| (value::intern(n), ()))
                .collect()
        });
    SYNTAX.contains_key(&s)
}

/// Resolve one free reference symbol. Qualify only with positive evidence the name
/// belongs to this namespace (already a `ns/name` global, or pre-scanned as a def
/// head this file will create); otherwise leave bare for root/prelude fall-through.
fn resolve_sym(
    heap: &Heap,
    s: value::Symbol,
    ns_name: &str,
    locals: &[value::Symbol],
) -> value::Symbol {
    if locals.contains(&s) {
        return s;
    }
    let name = value::symbol_name_ref(s);
    if let Some(slash) = name.find('/') {
        // `/name` — the addressable ROOT/prelude namespace: an EMPTY module prefix
        // (leading `/`) resolves to the bare root binding, so a module that shadows a
        // prelude name (`(defn map …)` → `mod/map`) can still reach the original as
        // `/map` (Elixir's `Kernel.foo`). A bare `/` — the division operator, empty
        // `rest` — is left alone. An empty prefix can never denote a real module (no
        // module has an empty name), so this collides with nothing.
        if slash == 0 && name.len() > 1 {
            return value::intern(&name[1..]);
        }
        // A qualified `prefix/rest`. If `prefix` is a module alias from `(:alias …)`
        // — stored in the import table under the slash-suffixed key `prefix/` so it
        // rides the same per-file lifecycle — rewrite to the real module path:
        // `conn/build` → `web/conn/build`. Otherwise it's already fully qualified.
        let alias_key = value::intern(&format!("{}/", &name[..slash]));
        if let Some(target) = heap.import_of(alias_key) {
            return value::intern(&format!(
                "{}/{}",
                value::symbol_name_ref(target),
                &name[slash + 1..]
            ));
        }
        // ADR-070: an intra-package qualified reference roots to the active package, just
        // as `%in-ns` roots the `(defmodule …)` it declares and `%refer` roots a `(:use …)`
        // target — `commands/cmd-open` inside project `bedit` becomes
        // `bedit/commands/cmd-open`. Rooting is *implied*, so the qualified spelling has to
        // root too: otherwise the bare names import fine while every explicit `mod/name`
        // goes unbound, and the `--` privacy rule below compares a rooted `cur_ns` against
        // an unrooted `m` and rejects a module's reference to its OWN helper. Rooting here,
        // in the expand pass, bakes the rooted spelling into the compiled code — a std or
        // other-package name is left bare, and an already-rooted one is unchanged.
        if let Some(rooted) = heap.root_qualified_ref(s) {
            // An intra-package qualified reference infers its (rooted) module's require.
            crate::eval::derive::record_qualified(value::symbol_name_ref(rooted), ns_name);
            return rooted;
        }
        // A qualified reference `mod/name` infers `(require 'mod)` — no explicit require
        // needed (ADR-227 follow-up). Record it so `compile` loads the module before
        // eval (for a value in argument position; a call head is loaded eagerly during
        // macroexpand). Deferred because `resolve_sym` runs under GC/macro blocks.
        crate::eval::derive::record_qualified(name, ns_name);
        return s; // already qualified, no alias
    }
    if is_ambient(s) {
        return s; // declared with `defdyn` — ambient/root, never namespaced
    }
    // Own namespace first (a same-named local def shadows an import), then a
    // `(:use …)` import, then root/prelude fall-through (left bare).
    let qsym = value::intern(&format!("{}/{}", ns_name, name));
    if heap.ns_knows_name(s) || heap.env_get(value::EnvId::GLOBAL, qsym).is_some() {
        qsym
    } else if let Some(candidates) = heap.ambiguous_import_of(s) {
        // Imported bare from two or more modules (ADR-235). A same-namespace def or a
        // local (checked above / at the top) would have shadowed it; reaching here means
        // the bare name is genuinely ambiguous. Record a use-site clash error — raised by
        // the driver after `resolve`, so it points at the form that used the name. The
        // returned symbol is irrelevant (compilation is about to be abandoned); leave it
        // bare so nothing spuriously resolves before the error surfaces.
        crate::eval::derive::record_ambiguous(s, candidates);
        s
    } else if let Some(imported) = heap.import_of(s) {
        imported
    } else if heap.ns_assume_own()
        && !is_syntax_keyword(s)
        && heap.env_get(value::EnvId::GLOBAL, s).is_none()
    {
        // No pre-scan behind this form (a runtime `eval`), so "will this namespace
        // define it?" has no positive evidence to consult — see `set_ns_assume_own`.
        // Root/prelude still wins when it actually binds the name, so this only
        // catches a name bound NOWHERE: either this namespace's, defined by a later
        // `eval` (the case worth fixing), or a typo, which now reports the qualified
        // spelling. Deliberately after the import branch, so `(:use …)` still wins.
        qsym
    } else {
        s
    }
}

fn resolve_walk(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    // Same deep-form stack safety as macroexpand_all_depth above.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        resolve_walk_inner(heap, form, ns_name, locals)
    })
}

fn resolve_walk_inner(
    heap: &mut Heap,
    form: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    match form.unpack() {
        ValueRef::Sym(s) => Value::symbol(resolve_sym(heap, s, ns_name, locals)),
        ValueRef::Pair(_) => resolve_list(heap, form, ns_name, locals),
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            heap.alloc_vector(out)
        }
        ValueRef::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = resolve_walk(heap, k, ns_name, locals);
                let v = resolve_walk(heap, v, ns_name, locals);
                pairs.push((k, v));
            }
            heap.map_from_pairs(pairs)
        }
        ValueRef::Set(id) => {
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items {
                out.push(resolve_walk(heap, e, ns_name, locals));
            }
            heap.set_from_elems(out)
        }
        other => other,
    }
}

fn resolve_list(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return form, // improper list — leave verbatim
    };
    if let Some(ValueRef::Sym(h)) = items.first().map(|v| v.unpack()) {
        if value::symbol_is(h, kw::QUOTE) {
            return form; // pure data — never descend (ADR-034)
        }
        if value::symbol_is(h, kw::QUASIQUOTE) {
            // α (ADR-065 §7): descend the template so a macro's *free* references
            // qualify to the **defining** namespace — frozen here at macro-def time,
            // so the expansion resolves in any consumer. `~unquote`/`~@` contents
            // resolve as code (the macro's params are in `locals`); a nested `quote`
            // stays data; `#` auto-gensyms and template-local binders stay bare
            // (not known ns names). At root (`compile_ns == None`) the whole resolver
            // is a no-op, so prelude macro templates are untouched.
            let mut out = Vec::with_capacity(items.len());
            out.push(items[0]); // the `quasiquote` head itself
            for &it in &items[1..] {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            return rebuild_list(heap, form, out);
        }
        if value::symbol_is(h, kw::DEF) || value::symbol_is(h, kw::DEFMACRO) {
            return resolve_def(heap, form, &items, ns_name, locals);
        }
        if value::symbol_is(h, kw::FN) {
            return resolve_fn(heap, form, &items, ns_name, locals);
        }
        if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
            return resolve_let(heap, form, &items, ns_name, locals);
        }
        if value::symbol_is(h, kw::MATCH_STAR) {
            return resolve_match(heap, form, &items, ns_name, locals);
        }
    }
    // Generic: resolve every element (the head too — a call head resolves).
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        out.push(resolve_walk(heap, it, ns_name, locals));
    }
    rebuild_list(heap, form, out)
}

/// `(def NAME value)` / `(defmacro NAME params body…)` — qualify NAME; resolve the
/// value (def) or the body with params bound (defmacro). Params left verbatim.
fn resolve_def(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let is_defmacro = matches!(items.first().map(|v| v.unpack()), Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::DEFMACRO));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // def / defmacro head, verbatim
    match items.get(1).map(|v| v.unpack()) {
        Some(ValueRef::Sym(name)) => {
            // Register the (bare) name as known before resolving the value, so a
            // self-reference in the body — e.g. the recursion in a `defprocess`-
            // generated receive loop — qualifies to the same `ns/name` the head
            // gets. `scan_def_names` misses macro-defined names (it scans the raw,
            // unexpanded form), so without this a `(defn counter … (counter …))`
            // that came from a macro would bind `ns/counter` but recurse on bare
            // `counter` → unbound. Harmless for an already-qualified name.
            // …but never for an ambient (`defdyn`-declared) name: it stays bare, so
            // registering it would qualify the rest of the file's references to a
            // namespace-local name that is never defined.
            if !value::symbol_name_ref(name).contains('/') && !is_ambient(name) {
                heap.add_ns_known_name(name);
            }
            out.push(Value::symbol(qualify_name(ns_name, name)));
        }
        Some(other) => out.push(other), // not a symbol — leave (eval will complain)
        None => return rebuild_list(heap, form, out),
    }
    if is_defmacro {
        let params = items.get(2).copied().unwrap_or(Value::nil());
        out.push(params); // params verbatim
        let mut inner = locals.to_vec();
        collect_param_syms(heap, params, &mut inner);
        for &b in items.get(3..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, b, ns_name, &inner));
        }
    } else {
        for &v in items.get(2..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, v, ns_name, locals));
        }
    }
    rebuild_list(heap, form, out)
}

/// `(fn …)` — single-arity `(params body…)` or multi-arity
/// `(doc? (params body…)…)`. Params bind in their body; param lists left verbatim.
fn resolve_fn(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let parts = &items[1..];
    let (has_doc, clause_start) = match parts.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if parts.len() > 1 => (true, 1),
        _ => (false, 0),
    };
    let clauses = &parts[clause_start..];
    let multi = !clauses.is_empty() && clauses.iter().all(|&f| is_arity_clause(heap, f));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // fn / lambda head
    if multi {
        if has_doc {
            out.push(parts[0]);
        }
        for &clause in clauses {
            out.push(resolve_arity_clause(heap, clause, ns_name, locals));
        }
    } else {
        let params = parts.first().copied().unwrap_or(Value::nil());
        // Binders verbatim, `&optional` defaults resolved.
        out.push(resolve_param_defaults(heap, params, ns_name, locals));
        let mut inner = locals.to_vec();
        collect_param_syms(heap, params, &mut inner);
        for &b in parts.get(1..).unwrap_or(&[]) {
            out.push(resolve_walk(heap, b, ns_name, &inner));
        }
    }
    rebuild_list(heap, form, out)
}

/// One `(params body…)` arity clause: params bind in the body.
fn resolve_arity_clause(
    heap: &mut Heap,
    clause: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let cparts = match heap.list_to_vec(clause) {
        Ok(c) if !c.is_empty() => c,
        _ => return clause,
    };
    let mut inner = locals.to_vec();
    collect_param_syms(heap, cparts[0], &mut inner);
    let mut out = Vec::with_capacity(cparts.len());
    out.push(resolve_param_defaults(heap, cparts[0], ns_name, locals));
    for &b in &cparts[1..] {
        out.push(resolve_walk(heap, b, ns_name, &inner));
    }
    rebuild_list(heap, clause, out)
}

/// `(let/let*/letrec (s1 v1 …) body…)` — simple symbol binders post-expand
/// (patterns lowered to `match*`). Binders left verbatim; RHSs and body resolved
/// with binders in scope (sequential — a safe over-approximation for plain `let`).
fn resolve_let(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let letrec = matches!(items.first().map(|v| v.unpack()), Some(ValueRef::Sym(h)) if value::symbol_is(h, kw::LETREC));
    let binds = match items.get(1).and_then(|&b| form_items(heap, b)) {
        Some(b) if b.len() % 2 == 0 => b,
        _ => return generic_resolve(heap, form, items, ns_name, locals),
    };
    let mut scope = locals.to_vec();
    if letrec {
        for &t in binds.iter().step_by(2) {
            if let ValueRef::Sym(s) = t.unpack() {
                scope.push(s);
            }
        }
    }
    let mut new_binds = Vec::with_capacity(binds.len());
    let mut i = 0;
    while i < binds.len() {
        let target = binds[i];
        let rhs_r = resolve_walk(heap, binds[i + 1], ns_name, &scope);
        new_binds.push(target); // binder verbatim
        new_binds.push(rhs_r);
        if !letrec {
            if let ValueRef::Sym(s) = target.unpack() {
                scope.push(s);
            }
        }
        i += 2;
    }
    let new_bind_form = rebuild_seq_like(heap, items[1], new_binds);
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]);
    out.push(new_bind_form);
    for &b in items.get(2..).unwrap_or(&[]) {
        out.push(resolve_walk(heap, b, ns_name, &scope));
    }
    rebuild_list(heap, form, out)
}

/// `(match* :ctx valexpr (pattern body…) …)` — resolve `valexpr` and each clause
/// body with the clause pattern's symbols treated as bound (over-approximation:
/// all symbols anywhere in the pattern are collected, so a binder is never
/// qualified; a pinned reference there is left bare — safe, occasionally lossy).
fn resolve_match(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    if items.len() < 3 {
        return generic_resolve(heap, form, items, ns_name, locals);
    }
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // match*
    out.push(items[1]); // :ctx keyword
    out.push(resolve_walk(heap, items[2], ns_name, locals)); // value expression
    for &clause in &items[3..] {
        let cparts = match heap.list_to_vec(clause) {
            Ok(c) if c.len() >= 2 => c,
            _ => {
                out.push(clause);
                continue;
            }
        };
        let mut scope = locals.to_vec();
        collect_all_syms(heap, cparts[0], &mut scope);
        let mut cout = Vec::with_capacity(cparts.len());
        cout.push(cparts[0]); // pattern verbatim
        for &b in &cparts[1..] {
            cout.push(resolve_walk(heap, b, ns_name, &scope));
        }
        out.push(rebuild_list(heap, clause, cout));
    }
    rebuild_list(heap, form, out)
}

/// Resolve every element of a list and rebuild (the fallback for binder forms whose
/// shape didn't match — never over-qualifies because it adds no bound names).
fn generic_resolve(
    heap: &mut Heap,
    form: Value,
    items: &[Value],
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let mut out = Vec::with_capacity(items.len());
    for &it in items {
        out.push(resolve_walk(heap, it, ns_name, locals));
    }
    rebuild_list(heap, form, out)
}

/// Resolve the **default expressions** inside a param list, leaving every binder name
/// alone. `(x &optional (n *lim*))` keeps `x` and `n` verbatim but resolves `*lim*` to
/// `ns/*lim*`.
///
/// Param lists used to be passed through verbatim, which left a default expression as
/// the only place in a module where a reference to the module's own global was never
/// qualified — so it resolved at call time in whatever namespace the CALLER was in and
/// raised `unbound symbol`. Earmuffs masked it (they were ambient pre-ADR-151, so the
/// usual `(&optional (n *knob*))` happened to work); a plain `(&optional (n limit))`
/// was already broken.
///
/// Earlier binders are in scope for a later default, so they accumulate as locals as the
/// list is walked. A destructuring `[a b]` param is a pattern, not a reference, and stays
/// verbatim.
fn resolve_param_defaults(
    heap: &mut Heap,
    params: Value,
    ns_name: &str,
    locals: &[value::Symbol],
) -> Value {
    let items = match form_items(heap, params) {
        Some(i) => i,
        None => return params,
    };
    let mut inner = locals.to_vec();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item.unpack() {
            // `(name default…)` — the binder stays, the defaults resolve.
            ValueRef::Pair(_) => {
                let parts = form_items(heap, item).unwrap_or_default();
                match parts.first().map(|v| v.unpack()) {
                    Some(ValueRef::Sym(binder)) => {
                        let mut rebuilt = Vec::with_capacity(parts.len());
                        rebuilt.push(parts[0]);
                        for &d in &parts[1..] {
                            rebuilt.push(resolve_walk(heap, d, ns_name, &inner));
                        }
                        inner.push(binder);
                        out.push(rebuild_list(heap, item, rebuilt));
                    }
                    _ => out.push(item),
                }
            }
            ValueRef::Sym(s) => {
                if !value::symbol_is(s, kw::AMP)
                    && !value::symbol_is(s, kw::AMP_OPTIONAL)
                    && !value::symbol_is(s, kw::AMP_REST)
                {
                    inner.push(s);
                }
                out.push(item);
            }
            _ => out.push(item),
        }
    }
    rebuild_list(heap, params, out)
}

/// Collect parameter-binder symbols from a param list (mirrors `fn_params` /
/// `parse_params`): plain symbols, `(name default)` optionals' names; skips the
/// `&`/`&optional`/`&rest` markers. Appends to `out`.
fn collect_param_syms(heap: &Heap, params: Value, out: &mut Vec<value::Symbol>) {
    let items = match form_items(heap, params) {
        Some(i) => i,
        None => return,
    };
    for item in items {
        match item.unpack() {
            ValueRef::Sym(s) => {
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                out.push(s);
            }
            ValueRef::Pair(_) | ValueRef::Vector(_) => {
                // `(name default)` — the binder is the first element.
                let inner = form_items(heap, item).unwrap_or_default();
                if let Some(ValueRef::Sym(s)) = inner.first().map(|v| v.unpack()) {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
}

/// Collect every symbol appearing anywhere in `v` (used to over-approximate a
/// pattern's bound names — see `resolve_match`).
fn collect_all_syms(heap: &Heap, v: Value, out: &mut Vec<value::Symbol>) {
    match v.unpack() {
        ValueRef::Sym(s) => out.push(s),
        ValueRef::Pair(_) => {
            if let Ok(items) = heap.list_to_vec(v) {
                for it in items {
                    collect_all_syms(heap, it, out);
                }
            }
        }
        ValueRef::Vector(id) => {
            for it in heap.vector(id).to_vec() {
                collect_all_syms(heap, it, out);
            }
        }
        ValueRef::Map(id) => {
            for (k, val) in heap.map_entries(id) {
                collect_all_syms(heap, k, out);
                collect_all_syms(heap, val, out);
            }
        }
        ValueRef::Set(id) => {
            for e in heap.set_elems(id) {
                collect_all_syms(heap, e, out);
            }
        }
        _ => {}
    }
}

/// Rebuild a binding container preserving list-vs-vector shape (and position).
fn rebuild_seq_like(heap: &mut Heap, original: Value, items: Vec<Value>) -> Value {
    match original.unpack() {
        ValueRef::Vector(_) => heap.alloc_vector(items),
        _ => rebuild_list(heap, original, items),
    }
}

/// If `sym` (a combination head, resolved in `env`) names a **macro**, return its
/// closure id. Resolves the head the way the eval-time dispatch and the `resolve`
/// pass do, so a bare `(:use mod)`-imported macro (or a same-namespace `ns/name`
/// macro) is recognised — not only a directly-bound one (ADR-065). Used by
/// `macroexpand_1` to expand, and by the compiling VM (`eval::compile`) to **defer**
/// a closure whose body still contains an unexpanded (forward-referenced) macro
/// call — both must agree on what "is a macro head" means.
pub(crate) fn macro_head_id(heap: &Heap, env: EnvId, sym: value::Symbol) -> Option<ClosureId> {
    match heap.env_get(env, sym).map(|v| v.unpack()) {
        Some(ValueRef::Macro(mid)) => Some(mid),
        // Directly bound to a non-macro (a local, or a non-macro global): it
        // shadows — never reinterpret it as an imported macro.
        Some(_) => None,
        // Unbound directly: a bare reference that may name an imported /
        // same-namespace macro. Resolve it as the `resolve` pass does.
        None => {
            let q = match heap.compile_ns() {
                Some(ns) => resolve_sym(heap, sym, value::symbol_name_ref(ns), &[]),
                None => heap.import_of(sym).unwrap_or(sym),
            };
            match (
                q != sym,
                heap.env_get(value::EnvId::GLOBAL, q).map(|v| v.unpack()),
            ) {
                (true, Some(ValueRef::Macro(mid))) => Some(mid),
                _ => None,
            }
        }
    }
}

/// Expand `form` by one step if its head is a macro; returns `(expanded, did_expand)`.
pub fn macroexpand_1(heap: &mut Heap, form: Value, env: EnvId) -> Result<(Value, bool), LispError> {
    let ValueRef::Pair(p) = form.unpack() else {
        return Ok((form, false));
    };
    let ValueRef::Sym(s) = heap.pair(p).0.unpack() else {
        return Ok((form, false));
    };
    // A qualified call head into a not-yet-loaded module may be a macro that
    // must expand at compile time (the compile-time require-for-macros rule) —
    // infer the require from the qualified name so the module loads before this lookup.
    // A no-op for a bare head or an already-loaded one (ADR-227 follow-up).
    let mut form = form;
    let mut env = env;
    if macro_head_id(heap, env, s).is_none() {
        // The require LOADS a module — arbitrary eval, therefore a collection that
        // relocates `form` and the env chain. Root both across it and re-read: the
        // `(macroexpand-1 …)` / `(macroexpand …)` BUILTINS reach here with
        // MACRO_BLOCK *off* (unlike the compile pass, which holds a
        // `MacroBlockGuard`), so a handle read before this call and used after it is
        // a live use-after-GC — an epoch-tripwire abort in debug and silent heap
        // corruption in release (it walked relocated memory and reported a garbage
        // list length as an arity error).
        let rb = heap.roots_len();
        let eb = heap.env_roots_len();
        heap.push_root(form);
        heap.push_env_root(env);
        let r = crate::eval::derive::require_qualified_head(heap, env, s);
        form = heap.root_at(rb);
        env = heap.env_root_at(eb);
        heap.truncate_roots(rb);
        heap.truncate_env_roots(eb);
        r?;
    }
    let Some(mid) = macro_head_id(heap, env, s) else {
        return Ok((form, false));
    };
    // Re-derive the tail from the (possibly relocated) form rather than reusing the
    // one read before the require.
    let ValueRef::Pair(p) = form.unpack() else {
        return Ok((form, false));
    };
    let tail = heap.pair(p).1;
    let args = heap.list_to_vec(tail)?;
    // Run the expander through the ACTIVE ENGINE (VM when enabled), not the
    // tree-walker. Paired with the compile pass expanding a macro's
    // autogensym-free `quasiquote` body to builder code (see `compile`), a
    // hot macro (`defn`, …) now compiles once and expands as bytecode/native
    // — macro expansion dominated the advisory checker (ADR-119). Same result
    // as `apply_closure`; the VM is the default engine for all other calls.
    let expanded = crate::eval::compile::apply_engine(heap, Value::Fn(mid), &args, env)?;
    Ok((expanded, true))
}

/// Repeatedly expand `form` until its head is no longer a macro.
pub fn macroexpand(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    // `macroexpand_1` applies the expander, which can collect at ANY depth
    // (ADR-061) — and the `(macroexpand …)` builtin reaches this at runtime with
    // MACRO_BLOCK *off* — so `env` must survive across iterations. Root it and
    // re-read; `cur` is the expander's fresh (current-epoch) result each round, or
    // the initial `form` before any eval, so it needs no slot.
    let eb = heap.env_roots_len();
    heap.push_env_root(env);
    let mut cur = form;
    // Bounded fixpoint (kernel audit): a macro that forever expands to another
    // macro call (`(defmacro m (x) `(m ~x))`) otherwise hard-hangs the expander
    // — mitigated only by green-process preemption, and not at all on a
    // no-deadline root-thread expansion. Same cap as the recursion guards.
    let mut rounds = 0u32;
    loop {
        if rounds >= MAX_EXPAND_ROUNDS {
            heap.truncate_env_roots(eb);
            return Err(LispError::runtime(format!(
                "macro expansion did not reach a fixpoint after {} rounds \
                 (a macro that expands to itself?)",
                MAX_EXPAND_ROUNDS
            )));
        }
        rounds += 1;
        let env_now = heap.env_root_at(eb);
        let (next, expanded) = match macroexpand_1(heap, cur, env_now) {
            Ok(r) => r,
            Err(e) => {
                heap.truncate_env_roots(eb);
                return Err(e);
            }
        };
        if !expanded {
            heap.truncate_env_roots(eb);
            return Ok(next);
        }
        cur = next;
    }
}

/// The compile pass: recursively expand *every* macro call in `form` (a code
/// walk), so the result contains no macro invocations and can be evaluated
/// without expanding again. Run once at each top-level / definition boundary
/// (`eval_str`, `load`, `require`, `eval`, and the prelude loader), so a macro
/// in a function body — notably `match` — is expanded ONCE rather than on every
/// call. The evaluator still expands macros lazily as a fallback, which covers
/// a macro defined and used within the same top-level form (not yet defined
/// when the walk ran).
///
/// `quote` and `quasiquote` are left opaque: their contents are data, not calls
/// to expand. Code inside a `~unquote` still expands when the quasiquote runs.
pub fn macroexpand_all(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    macroexpand_all_depth(heap, form, env, 0)
}

fn macroexpand_all_depth(heap: &mut Heap, form: Value, env: EnvId, depth: u32) -> LispResult {
    // Expansion recurses per nesting level of the form; a deeply-nested-but-
    // legal form (30k+ levels) would blow the native stack. Grow it in
    // heap-backed segments instead (the stacker remedy the deep-VALUE walkers
    // got on 2026-07-20, extended to the code walkers) — the fast path is a
    // couple of compares, and depth stays structurally bounded by the form.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        macroexpand_all_depth_inner(heap, form, env, depth)
    })
}

fn macroexpand_all_depth_inner(heap: &mut Heap, form: Value, env: EnvId, depth: u32) -> LispResult {
    // Block GC during the expansion: this walk holds partially-built LOCAL forms
    // in Rust locals and recurses into macro applications via `eval`, whose
    // safepoint would otherwise sweep them. The runtime evaluator roots its
    // transients on the operand stack so its safepoint fires at any depth
    // (ADR-061) — but the compile pass opts out instead: `MacroBlockGuard` keeps
    // `MACRO_BLOCK > 0` for the expansion, and the safepoint skips collection
    // while that holds. Expansion is bounded per form, so memory grows briefly
    // (reclaimed at the next runtime safepoint). The `GcBlockGuard` is kept too,
    // purely for the stack-depth accounting it feeds. See `docs/memory-model.md`.
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "macro expansion nested too deeply (math/max {} levels)",
            MAX_DEPTH
        )));
    }
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    let original = form;
    let form = macroexpand(heap, form, env)?;
    match form.unpack() {
        ValueRef::Pair(_) => {
            let items = match heap.list_to_vec(form) {
                Ok(items) => items,
                Err(_) => return Ok(form), // improper list: leave it be
            };
            if let Some(ValueRef::Sym(head)) = items.first().copied().map(|v| v.unpack()) {
                // quote/quasiquote contents are data, not calls to expand.
                if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                    return Ok(form);
                }
                // (`lambda` used to be canonicalised to `fn` here — the alias was
                // retired in ADR-162; `fn` is the only spelling.)
                let s = head;
                // Desugar pattern binders into the Brood `match*` engine so they
                // expand once here (fast) rather than per call. eval's `let`/`fn`
                // then only ever see plain symbol binds.
                if value::symbol_is(s, kw::LET) || value::symbol_is(s, kw::LETREC) {
                    // A **vector** bindings container — `(let [a 1] …)`, and with it
                    // the Clojure `(let [[a 1] [b 2]] …)` shape that used to
                    // destructure `[a 1]` against `[b 2]` and report a bewildering
                    // `unbound symbol: b`. Lists for code (ADR-010): reject it here,
                    // in the pass both engines share.
                    if matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Vector(_))) {
                        let who = if value::symbol_is(s, kw::LET) {
                            "let"
                        } else {
                            "letrec"
                        };
                        return Err(crate::eval::vector_binding_container_error(who));
                    }
                }
                if value::symbol_is(s, kw::LET) {
                    // A nested-Scheme binding form with literal values —
                    // `(let ((a 1) (b 2)) …)` — would otherwise lower into a
                    // refutable pattern match on `(b 2)` and die with a confusing
                    // "unbound symbol". Catch it here (the compile pass every engine
                    // shares, so both the VM and tree-walker report it) with the
                    // flatten hint. The literal-value guard keeps a genuine bare-list
                    // destructure like `(let ((a b) '(1 2)) …)` untouched.
                    if let Some(binds) = items.get(1).and_then(|&b| form_items(heap, b)) {
                        if crate::eval::even_bindings_look_scheme(heap, &binds) {
                            return Err(crate::eval::scheme_binding_error(heap, "let", &binds));
                        }
                    }
                    if let Some(lowered) = lower_let(heap, &items) {
                        return macroexpand_all_depth(heap, lowered, env, depth + 1);
                    }
                    // Ordinary let: expand binding *values* and the body, but not the
                    // binding *targets* — a bound name must not be expanded as a call.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, kw::LETREC) {
                    // Same shape as let: even-indexed binding entries are targets
                    // (opaque), odd-indexed are values (expand). letrec disallows
                    // pattern targets in eval, so there's no `lower_let` branch.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, kw::FN) {
                    if let Some(lowered) = lower_fn(heap, &items)? {
                        return macroexpand_all_depth(heap, lowered, env, depth + 1);
                    }
                    // `lower_fn` declined: this is either a single-clause fn (its
                    // param list at items[1]) or an arity-only *multi*-clause fn
                    // (each remaining form is a `(param-list body…)` clause, built
                    // into `ClosureArm`s by the evaluator). For multi-clause, expand
                    // each clause's BODY while leaving its param list opaque; for
                    // single-clause, expand only the body after the param list.
                    if fn_is_arity_multi_clause(heap, &items) {
                        return expand_fn_clauses(heap, original, &items, env, depth + 1);
                    }
                    return expand_tail(heap, original, &items, 2, env, depth + 1);
                } else if value::symbol_is(s, kw::DEFMACRO) {
                    // (defmacro name params body...) — name/params aren't calls.
                    return expand_tail(heap, original, &items, 3, env, depth + 1);
                }
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all_depth(heap, item, env, depth + 1)?);
            }
            // Wrapper-split a linear immutable-map fold into an in-place table loop
            // (docs/linear-map-accumulator.md); applies to a qualifying
            // `(def NAME (fn …))` that passes the `linmap_probe` reachability gate.
            // On by default; opt out with `BROOD_LINMAP=0`.
            if let Some(split) = linmap_split_def(heap, &out) {
                return Ok(split);
            }
            Ok(rebuild_list(heap, original, out))
        }
        ValueRef::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all_depth(heap, item, env, depth + 1)?);
            }
            Ok(heap.alloc_vector(out))
        }
        ValueRef::Map(id) => {
            // Walk a map literal's keys and values so macros inside them expand
            // once here. Keep it a literal map (the evaluator canonicalises it).
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = macroexpand_all_depth(heap, k, env, depth + 1)?;
                let v = macroexpand_all_depth(heap, v, env, depth + 1)?;
                pairs.push((k, v));
            }
            Ok(heap.map_from_pairs(pairs))
        }
        ValueRef::Set(id) => {
            // Walk a set literal's elements so macros inside them expand once here.
            // Keep it a literal set (the evaluator evaluates + dedups it).
            let items = heap.set_elems(id);
            let mut out = Vec::with_capacity(items.len());
            for e in items {
                out.push(macroexpand_all_depth(heap, e, env, depth + 1)?);
            }
            Ok(heap.set_from_elems(out))
        }
        other => Ok(other),
    }
}

/// Rebuild `items` into a fresh list, copying the source position of the
/// `original` pair (if any). The compile pass goes through this on every list
/// it expands, so source positions survive macroexpansion — diagnostics from
/// inside a nested combination still point at the original line, not at the
/// enclosing top-level form's start. No-op for non-LOCAL originals (see
/// [`Heap::form_pos`](crate::core::heap::Heap::form_pos)).
fn rebuild_list(heap: &mut Heap, original: Value, items: Vec<Value>) -> Value {
    let pos = heap.form_pos_only(original);
    let new_list = heap.list(items);
    if let Some(p) = pos {
        heap.set_form_pos(new_list, p);
    }
    new_list
}

/// Wrapper-split a **linear immutable-map fold** (docs/linear-map-accumulator.md).
/// Given a fully-expanded `(def NAME (fn (P…) BODY…))` whose accumulator is provably
/// linear (checked by `linmap_probe`), rewrite it to
///
/// ```text
/// (do (def INNER (fn (P…) BODY'))                ; in-place table loop
///     (def NAME  (fn (P…) (table-snapshot (INNER … (%table-from-map ACC) …)))))
/// ```
///
/// where `BODY'` swaps the self-recursion head `NAME→INNER` and every whitelisted
/// map op on the accumulator to its in-place `table-*` op. The inner loop builds a
/// private table (seeded once by the wrapper, frozen once on return), so the
/// per-update path-copy is gone with no per-iteration cost. Returns the replacement,
/// or `None` to leave the `def` unchanged. `items` is the expanded `def` form.
fn linmap_split_def(heap: &mut Heap, items: &[Value]) -> Option<Value> {
    if items.len() != 3 {
        return None;
    }
    if !matches!(items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::DEF)) {
        return None;
    }
    let name_sym = match items[1].unpack() {
        ValueRef::Sym(s) => s,
        _ => return None,
    };
    let fn_items = heap.list_to_vec(items[2]).ok()?;
    if fn_items.len() < 3
        || !matches!(fn_items[0].unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::FN))
    {
        return None;
    }
    let param_form = fn_items[1];
    let param_vals = form_items(heap, param_form)?;
    let mut params: Vec<value::Symbol> = Vec::with_capacity(param_vals.len());
    for &pv in &param_vals {
        match pv.unpack() {
            ValueRef::Sym(s)
                if !value::symbol_is(s, kw::AMP) && !value::symbol_is(s, kw::AMP_OPTIONAL) =>
            {
                params.push(s)
            }
            _ => return None, // &optional / & rest / pattern param → bail
        }
    }
    let body = &fn_items[2..];
    // A leading docstring complicates the split; skip it conservatively for now.
    if body.len() > 1 && matches!(body[0].unpack(), ValueRef::Str(_)) {
        return None;
    }
    // A `quasiquote` anywhere in the body declines the split. The probe runs on the *Node*
    // IR, where an unquote's contents are ordinary code, so an `acc` use inside one counts
    // toward the linearity proof — but `linmap_rewrite_form` cannot safely rewrite inside a
    // quasiquote (it would have to distinguish quoted structure from unquoted code), and
    // leaving it alone would let a `%map-get` reach the in-place `Table`. Plain `quote` needs
    // no gate: it is inert data the probe sees as a `Const`, and the rewriter now passes it
    // through untouched.
    if body.iter().any(|&f| form_has_quasiquote(heap, f, 0)) {
        return None;
    }
    // Soundness gate: reuse the Node-level reachability analysis as a probe.
    let idx = crate::eval::compile::linmap_probe(heap, name_sym, &params, body)?;

    let inner_val = value::gensym(&format!("{}/linmap-loop", value::symbol_name(name_sym)));
    let inner_body: Vec<Value> = body
        .iter()
        .map(|&f| linmap_rewrite_form(heap, f, name_sym, inner_val, params[idx]))
        .collect();
    let mut inner_fn = vec![value::sym(kw::FN), param_form];
    inner_fn.extend(inner_body);
    let inner_fn = heap.list(inner_fn);
    let inner_def = heap.list(vec![value::sym(kw::DEF), inner_val, inner_fn]);

    let mut call = vec![inner_val];
    for (i, &pv) in param_vals.iter().enumerate() {
        if i == idx {
            call.push(heap.list(vec![value::sym("%table-from-map"), pv]));
        } else {
            call.push(pv);
        }
    }
    let inner_call = heap.list(call);
    // Snapshot the loop's result back to an immutable map **only when it is the table**.
    // The accumulator is what the linearity proof licenses rewriting in place, but the base
    // case is free to return something else entirely — `(%map-count acc)`, `(%map-get acc :a)`,
    // or a plain constant — and an unconditional `table-snapshot` then failed on a value it
    // was never given: `table-snapshot: expected table, got int (3)`. `linmap_linear` admits
    // those returns deliberately (a whitelisted *read* of the accumulator, or a `Const`), so
    // the wrapper, not the proof, is what has to account for them.
    //
    // `(= :table (type-of r))` rather than a predicate call: `type-of` is a total PrimOp1
    // (never deopts) and the comparison is against an interned keyword, so this costs one
    // tag read on the way out of a fold — and only on the way out, not per element.
    let r_val = value::gensym("linmap-out");
    let type_of = heap.list(vec![value::sym("type-of"), r_val]);
    let is_table = heap.list(vec![value::sym("="), value::kw("table"), type_of]);
    let snap_call = heap.list(vec![value::sym(kw::TABLE_SNAPSHOT), r_val]);
    let cond = heap.list(vec![value::sym(kw::IF), is_table, snap_call, r_val]);
    let bind = heap.list(vec![r_val, inner_call]);
    let snap = heap.list(vec![value::sym(kw::LET), bind, cond]);
    let wrapper_fn = heap.list(vec![value::sym(kw::FN), param_form, snap]);
    let wrapper_def = heap.list(vec![value::sym(kw::DEF), items[1], wrapper_fn]);
    Some(heap.list(vec![value::sym(kw::DO), inner_def, wrapper_def]))
}

/// Does `form` contain a `quasiquote` anywhere? Guards the linmap wrapper-split (see the
/// call site). Depth-bounded so a pathological datum can't recurse the compiler thread.
fn form_has_quasiquote(heap: &Heap, form: Value, depth: usize) -> bool {
    if depth > 64 {
        return true; // too deep to prove clean — decline the split
    }
    match heap.list_to_vec(form) {
        Ok(items) => items.iter().enumerate().any(|(i, &it)| {
            (i == 0
                && matches!(it.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::QUASIQUOTE)))
                || form_has_quasiquote(heap, it, depth + 1)
        }),
        Err(_) => false,
    }
}

/// Source rewrite for the inner loop of a wrapper-split: turn the self-recursion
/// head `name → inner`, and each whitelisted map op whose first arg is the
/// accumulator `acc` into its in-place table op (updates yield the table via a
/// `do`; reads pass straight through). Must stay in lockstep with `linmap_probe`'s
/// whitelist — the probe only admits a body whose every `acc` use is one of these.
fn linmap_rewrite_form(
    heap: &mut Heap,
    form: Value,
    name: value::Symbol,
    inner: Value,
    acc: value::Symbol,
) -> Value {
    let items = match form.unpack() {
        ValueRef::Pair(_) => match heap.list_to_vec(form) {
            Ok(v) if !v.is_empty() => v,
            _ => return form,
        },
        _ => return form, // atom: symbols (incl. a bare `acc` return) pass through
    };
    if let ValueRef::Sym(h) = items[0].unpack() {
        let second_is_acc =
            matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Sym(s)) if s == acc);
        if second_is_acc {
            let update = if value::symbol_is(h, kw::MAP_INT_ADD) {
                Some(kw::TABLE_INCR)
            } else if value::symbol_is(h, kw::MAP_DISSOC) {
                Some(kw::TABLE_DELETE)
            } else {
                None
            };
            if let Some(t) = update {
                // (op acc rest…) → (do (table-op acc rest…) acc) — mutate, yield table.
                let mut c = vec![value::sym(t), items[1]];
                for &a in &items[2..] {
                    c.push(linmap_rewrite_form(heap, a, name, inner, acc));
                }
                let mutate = heap.list(c);
                return heap.list(vec![value::sym(kw::DO), mutate, items[1]]);
            }
            let read = if value::symbol_is(h, kw::MAP_GET) {
                Some(kw::TABLE_GET)
            } else if value::symbol_is(h, kw::MAP_COUNT) {
                Some(kw::TABLE_COUNT)
            } else {
                None
            };
            if let Some(t) = read {
                let mut c = vec![value::sym(t), items[1]];
                for &a in &items[2..] {
                    c.push(linmap_rewrite_form(heap, a, name, inner, acc));
                }
                return heap.list(c);
            }
        }
        if h == name {
            // Self-recursion → call the inner loop instead.
            let mut c = vec![inner];
            for &a in &items[1..] {
                c.push(linmap_rewrite_form(heap, a, name, inner, acc));
            }
            return heap.list(c);
        }
        // `(quote …)` is inert DATA, never evaluated — rewriting inside it corrupts the
        // datum instead of the program. `(io/puts '(%map-get acc 1))` printed
        // `(table-get acc 1)`. The probe cannot catch this: a quoted form compiles to a
        // single `Node::Const`, so the linearity analysis sees no `acc` use at all and
        // passes, while the source rewrite walks straight through the quote.
        if value::symbol_is(h, kw::QUOTE) {
            return form;
        }
    }
    let out: Vec<Value> = items
        .iter()
        .map(|&it| linmap_rewrite_form(heap, it, name, inner, acc))
        .collect();
    heap.list(out)
}

/// Rebuild a form expanding only `items[start..]` (the call's body/argument tail),
/// leaving `items[..start]` opaque. Used to skip binding positions — a fn/defmacro
/// parameter list — so a name there is never mistaken for a macro call.
fn expand_tail(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    start: usize,
    env: EnvId,
    depth: u32,
) -> LispResult {
    let start = start.min(items.len());
    let mut out = items[..start].to_vec();
    for &item in &items[start..] {
        out.push(macroexpand_all_depth(heap, item, env, depth)?);
    }
    Ok(rebuild_list(heap, original, out))
}

/// Does this (post-`lower_fn`) `fn`/`lambda` form's body consist entirely of
/// `(param-list body…)` clauses — i.e. is it an arity-only multi-clause fn? (A
/// leading docstring is allowed.) Pattern multi-clause fns were already lowered
/// to `match*`, so by here "all clauses" implies arity-only.
pub(crate) fn fn_is_arity_multi_clause(heap: &Heap, items: &[Value]) -> bool {
    let forms = &items[1..];
    let forms = match forms.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    !forms.is_empty() && forms.iter().all(|&f| is_clause(heap, f))
}

/// Expand an arity-only multi-clause `fn`: each clause is `(param-list body…)`.
/// Leave each clause's param list opaque (a binding position — a name there must
/// not be expanded as a call) and macroexpand each clause's body forms. A leading
/// docstring is passed through untouched.
fn expand_fn_clauses(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    env: EnvId,
    depth: u32,
) -> LispResult {
    let mut out = vec![items[0]]; // the `fn`/`lambda` head
    let mut i = 1;
    if matches!(items.get(1).map(|v| v.unpack()), Some(ValueRef::Str(_))) && items.len() > 2 {
        out.push(items[1]); // leading docstring
        i = 2;
    }
    for &clause in &items[i..] {
        match form_items(heap, clause) {
            Some(parts) if !parts.is_empty() => {
                let mut co = vec![parts[0]]; // param list: opaque
                for &b in &parts[1..] {
                    co.push(macroexpand_all_depth(heap, b, env, depth)?);
                }
                out.push(rebuild_list(heap, clause, co));
            }
            _ => out.push(clause),
        }
    }
    Ok(rebuild_list(heap, original, out))
}

/// Expand an ordinary `let`: its binding *values* (odd positions of the binding
/// list) and its body, leaving the binding *targets* (even positions) opaque.
fn expand_let(
    heap: &mut Heap,
    original: Value,
    items: &[Value],
    env: EnvId,
    depth: u32,
) -> LispResult {
    let Some(bindings) = items.get(1).copied() else {
        return Ok(rebuild_list(heap, original, items.to_vec()));
    };
    let new_bindings = match form_items(heap, bindings) {
        Some(binds) => {
            let mut nb = Vec::with_capacity(binds.len());
            for (i, &x) in binds.iter().enumerate() {
                // odd index = a value expression (expand); even = a target (opaque)
                nb.push(if i % 2 == 1 {
                    macroexpand_all_depth(heap, x, env, depth)?
                } else {
                    x
                });
            }
            match bindings.unpack() {
                ValueRef::Vector(_) => heap.alloc_vector(nb),
                _ => rebuild_list(heap, bindings, nb),
            }
        }
        None => bindings,
    };
    let mut out = vec![items[0], new_bindings];
    for &item in &items[2..] {
        out.push(macroexpand_all_depth(heap, item, env, depth)?);
    }
    Ok(rebuild_list(heap, original, out))
}

// ---- pattern-binder lowering (the compile pass desugars these to `match*`) ----

/// List/vector/`()` -> its element forms; anything else isn't a binding/param list.
fn form_items(heap: &Heap, v: Value) -> Option<Vec<Value>> {
    match v.unpack() {
        ValueRef::Nil => Some(Vec::new()),
        ValueRef::Pair(_) => heap.list_to_vec(v).ok(),
        ValueRef::Vector(id) => Some(heap.vector(id).to_vec()),
        _ => None,
    }
}

fn is_sym(v: Value) -> bool {
    matches!(v.unpack(), ValueRef::Sym(_))
}

fn make_do(heap: &mut Heap, body: &[Value]) -> Value {
    let mut v = Vec::with_capacity(body.len() + 1);
    v.push(value::sym(kw::DO));
    v.extend_from_slice(body);
    heap.list(v)
}

/// `(match* :ctx valexpr (pattern inner))` — a single-clause refutable bind.
fn refutable_bind(
    heap: &mut Heap,
    ctx: &str,
    valexpr: Value,
    pattern: Value,
    inner: Value,
) -> Value {
    let clause = heap.list(vec![pattern, inner]);
    heap.list(vec![
        value::sym(kw::MATCH_STAR),
        value::kw(ctx),
        valexpr,
        clause,
    ])
}

/// Lower a `let` whose bindings include a non-symbol (pattern) target into
/// nested symbol-`let` / refutable `match*` binds (sequential, so each sees the
/// previous). Returns `None` for an all-symbol or malformed `let` (left as-is).
fn lower_let(heap: &mut Heap, items: &[Value]) -> Option<Value> {
    let bindings = *items.get(1)?;
    let binds = form_items(heap, bindings)?;
    if binds.len() % 2 != 0 {
        return None; // malformed: let eval report it
    }
    if !binds.iter().step_by(2).any(|&t| !is_sym(t)) {
        return None; // all targets are plain symbols — ordinary let
    }
    let body = &items[2..];
    let mut acc = make_do(heap, body);
    let mut i = binds.len();
    while i >= 2 {
        let (target, valexpr) = (binds[i - 2], binds[i - 1]);
        acc = if is_sym(target) {
            let bind = heap.list(vec![target, valexpr]);
            heap.list(vec![value::sym(kw::LET), bind, acc])
        } else {
            refutable_bind(heap, kw::LET, valexpr, target, acc)
        };
        i -= 2;
    }
    Some(acc)
}

/// A multi-clause `fn` clause is `(param-list body...)` where the param-list is
/// itself a list (or `()`). A vector head is *not* a clause (param lists are
/// lists, ADR-010) — that disambiguates a single tuple param from a clause.
fn is_clause(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => matches!(heap.car(p).unpack(), ValueRef::Pair(_) | ValueRef::Nil),
        _ => false,
    }
}

/// Like [`is_clause`], but *also* true for a **vector**-headed form. Recognised
/// only so it can be rejected: `(defn f ([x] :one) ([x y] :two))` is Clojure's
/// multi-arity spelling, and reading it as Brood would silently produce one
/// 2-parameter function with an empty body (`[x]` and `:one` as two patterns) —
/// a different program, diagnosed only later as a misleading arity error at the
/// call site. See [`crate::eval::vector_binding_container_error`].
fn is_clause_shaped(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => matches!(
            heap.car(p).unpack(),
            ValueRef::Pair(_) | ValueRef::Nil | ValueRef::Vector(_)
        ),
        _ => false,
    }
}

/// Does this clause head carry an `&optional` / `&` **arity** marker? A head that
/// is dispatched as a *pattern* matches those markers as ordinary literal symbols,
/// so a clause like `((x &optional (y 5)) …)` silently stops being variadic and
/// only ever matches a literal `&optional` argument. The two mechanisms don't
/// combine (`&optional` controls arity, patterns control shape), so mixing them in
/// one `fn` is rejected rather than reinterpreted.
fn head_has_amp_marker(heap: &Heap, head: Value) -> bool {
    form_items(heap, head).is_some_and(|items| {
        items.iter().any(|&p| {
            matches!(p.unpack(), ValueRef::Sym(s)
                if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP))
        })
    })
}

/// Does any clause head of an all-clause-shaped `fn` body use the vector
/// spelling? (`clauses` is every form after `fn` and an optional docstring.)
fn has_vector_clause_head(heap: &Heap, clauses: &[Value]) -> bool {
    !clauses.is_empty()
        && clauses.iter().all(|&f| is_clause_shaped(heap, f))
        && clauses.iter().any(|&f| match f.unpack() {
            ValueRef::Pair(p) => matches!(heap.car(p).unpack(), ValueRef::Vector(_)),
            _ => false,
        })
}

/// Is `param_form` an *arity* parameter list — only plain symbols (params) and
/// the `&optional`/`&` markers, with no literal or destructuring *patterns*?
/// Arity clauses dispatch by argument count via native multi-arity arms
/// (`ClosureArm`, cheap — direct bind); a clause with any non-symbol parameter is
/// a *pattern* clause and must go through the `match*` engine instead.
pub(crate) fn is_arity_param_list(heap: &Heap, param_form: Value) -> bool {
    match form_items(heap, param_form) {
        Some(items) => items.iter().all(|&p| is_sym(p)),
        None => false,
    }
}

/// A clause whose parameter list is an arity list (see [`is_arity_param_list`]).
pub(crate) fn is_arity_clause(heap: &Heap, f: Value) -> bool {
    match f.unpack() {
        ValueRef::Pair(p) => {
            let head = heap.car(p);
            matches!(head.unpack(), ValueRef::Pair(_) | ValueRef::Nil)
                && is_arity_param_list(heap, head)
        }
        _ => false,
    }
}

/// Does this clause carry a `:when` guard — is the form right after its
/// parameter-list head the `:when` keyword (`(params :when guard body…)`)?
/// A guarded clause must dispatch through `match*` (which evaluates the guard);
/// the native arity path binds by argument count and cannot, so it would silently
/// ignore the guard. See [`lower_fn`] (ADR-226).
pub(crate) fn clause_has_when_guard(heap: &Heap, clause: Value) -> bool {
    heap.list_to_vec(clause)
        .ok()
        .and_then(|parts| parts.get(1).copied())
        .is_some_and(
            |v| matches!(v.unpack(), ValueRef::Keyword(s) if value::symbol_name_ref(s) == "when"),
        )
}

/// Cheap predicate: does this `fn`/`lambda` form need pattern lowering — i.e. is
/// it multi-clause, or single-clause with a pattern in a required parameter?
/// Mirrors [`lower_fn`]'s dispatch. Used by the evaluator as a fallback for `fn`
/// forms that reached it without the compile pass (built by a quasiquote, or a
/// macro expanded lazily within its defining form); an ordinary `fn` returns
/// `false` here and takes the normal `make_closure` path.
pub(crate) fn fn_needs_lowering(heap: &Heap, fn_form: Value) -> bool {
    let items = match heap.list_to_vec(fn_form) {
        Ok(items) => items,
        Err(_) => return false,
    };
    let forms = &items[1..];
    // Peel a leading docstring (matches `lower_fn`), so a multi-clause fn with a
    // docstring is still recognised as needing lowering.
    let forms = match forms.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    if forms.is_empty() {
        return false;
    }
    if forms.iter().all(|&f| is_clause(heap, f)) {
        // Multi-clause. Arity-only clauses dispatch natively (`make_closure`
        // builds `ClosureArm`s), so they DON'T need `match*` lowering; only a
        // clause carrying a literal/destructuring pattern — or a `:when` guard,
        // which the native path ignores (ADR-226) — does.
        return !forms.iter().all(|&f| is_arity_clause(heap, f))
            || forms.iter().any(|&f| clause_has_when_guard(heap, f));
    }
    // single-clause: a pattern in a required slot (before &optional / & rest)?
    let params = match form_items(heap, forms[0]) {
        Some(p) => p,
        None => return false,
    };
    let required_end = params
        .iter()
        .position(|&p| matches!(p.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    params[..required_end].iter().any(|&p| !is_sym(p))
}

/// Lower a `fn` that is multi-clause, or single-clause with pattern(s) in its
/// required parameters, into a plain `fn` plus the Brood `match*` engine.
/// `Ok(None)` for an ordinary single-clause `fn` (left as-is); an error for a
/// **vector** where a parameter list belongs (ADR-010, item 1 of the syntax
/// finalisation) — the one shape that used to be misread instead of rejected.
fn lower_fn(heap: &mut Heap, items: &[Value]) -> Result<Option<Value>, LispError> {
    let forms = &items[1..];

    // Multi-clause: an optional leading docstring, then every form a clause. The
    // docstring sits *before* the clauses here (a single-clause fn's docstring
    // sits after the param list and is peeled below); keep it as the lowered
    // fn's leading body form so `make_closure` still finds it.
    {
        let (doc, clauses): (Option<Value>, &[Value]) = match forms.first().map(|v| v.unpack()) {
            Some(ValueRef::Str(_)) if forms.len() > 1 => (Some(forms[0]), &forms[1..]),
            _ => (None, forms),
        };
        // Clojure's vector-headed clauses — `([x] :one) ([x y] :two)`. Caught
        // *before* the list-headed check so it can't fall through to the
        // single-clause path and be read as one long pattern parameter list.
        if has_vector_clause_head(heap, clauses) {
            return Err(crate::eval::vector_binding_container_error("fn"));
        }
        if !clauses.is_empty() && clauses.iter().all(|&f| is_clause(heap, f)) {
            // This IS a multi-clause fn — never fall through to the single-clause
            // path below (which would misread the first clause as a param list).
            if clauses.iter().all(|&f| is_arity_clause(heap, f))
                && !clauses.iter().any(|&f| clause_has_when_guard(heap, f))
            {
                // Arity-only AND guard-free: dispatches natively (the evaluator's
                // `make_closure` builds one `ClosureArm` per clause, bound by argument
                // count — no rest-list, no `match*`). Leave it un-lowered. A `:when`
                // guard forces the `match*` path below, which evaluates it (ADR-226).
                return Ok(None);
            }
            // At least one literal/destructuring *pattern* clause, or a `:when`
            // *guard* clause (ADR-226) → lower the whole dispatch to the `match*`
            // engine. An `&optional`/`&` marker in ANY head is now an error rather
            // than a literal-symbol pattern: the clause would silently stop being
            // variadic, and `(f 1 2)` would fail with a match-error listing
            // `(x &optional (y 5))` as a *pattern*.
            let guarded = clauses.iter().any(|&c| clause_has_when_guard(heap, c));
            for &clause in clauses {
                let head = match clause.unpack() {
                    ValueRef::Pair(p) => heap.car(p),
                    _ => continue,
                };
                if head_has_amp_marker(heap, head) {
                    let shown = crate::syntax::printer::print(heap, head);
                    let dispatch = if guarded { "guard" } else { "pattern" };
                    return Err(LispError::runtime(format!(
                        "fn: `&optional`/`&` in the clause {shown} of a \
                         {dispatch}-dispatched fn"
                    ))
                    .with_hint(
                        "these axes don't combine: `&optional`/`&` control ARITY, while \
                         patterns and `:when` guards route the fn through the match engine, \
                         where a head is a pattern and the marker would be a literal symbol. \
                         Use one mechanism per fn — arity clauses `((x) …) ((x y) …)`, or \
                         pattern/guard clauses plus a `match` on the optional argument in the \
                         body.",
                    ));
                }
            }
            let g = value::gensym("args");
            let params = heap.list(vec![value::sym(kw::AMP), g]);
            let mut mexpr = vec![value::sym(kw::MATCH_STAR), value::kw("fn"), g];
            mexpr.extend_from_slice(clauses); // fn clauses are already match* clauses
            let body = heap.list(mexpr);
            let mut lowered = vec![value::sym(kw::FN), params];
            if let Some(d) = doc {
                lowered.push(d);
            }
            lowered.push(body);
            return Ok(Some(heap.list(lowered)));
        }
    }

    // Single-clause: forms[0] is the parameter list, forms[1..] the body.
    let param_form = match forms.first() {
        Some(&f) => f,
        None => return Ok(None),
    };
    // `(fn [x y] …)` — a vector parameter list. Rejected here rather than accepted
    // as a second spelling: lists-for-code is the rule, and tolerating the vector
    // is what let `(let [[a 1] [b 2]] …)`-shaped mistakes destructure instead of
    // erroring (ADR-010).
    if matches!(param_form.unpack(), ValueRef::Vector(_)) {
        return Err(crate::eval::vector_binding_container_error("fn"));
    }
    let body = &forms[1..];
    let params = match form_items(heap, param_form) {
        Some(p) => p,
        None => return Ok(None),
    };

    // Peel a leading docstring (a string literal with more body after it) so it
    // stays the *first* form of the lowered `fn` — otherwise `make_closure`'s
    // docstring detection misses it once the body is wrapped in the refutable
    // bind + `do`. (`(fn ([x y]) "doc" body)` would lose its doc otherwise.)
    let (doc, body) = match body.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(_)) if body.len() > 1 => (Some(body[0]), &body[1..]),
        _ => (None, body),
    };

    // Patterns are allowed only in required slots (before &optional / & rest).
    let required_end = params
        .iter()
        .position(|&p| matches!(p.unpack(), ValueRef::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    if !params[..required_end].iter().any(|&p| !is_sym(p)) {
        return Ok(None); // no pattern in the required params — ordinary fn
    }

    // Replace each required pattern slot with a fresh symbol; bind it refutably.
    let mut new_params = params.clone();
    let mut binds: Vec<(Value, Value)> = Vec::new();
    for (idx, &p) in params[..required_end].iter().enumerate() {
        if !is_sym(p) {
            let g = value::gensym("arg");
            new_params[idx] = g;
            binds.push((g, p));
        }
    }
    let mut acc = make_do(heap, body);
    for &(g, pat) in binds.iter().rev() {
        acc = refutable_bind(heap, kw::FN, g, pat, acc);
    }
    let new_param_form = heap.list(new_params);
    let mut lowered = vec![value::sym(kw::FN), new_param_form];
    if let Some(doc) = doc {
        lowered.push(doc); // keep the docstring as the leading body form
    }
    lowered.push(acc);
    Ok(Some(heap.list(lowered)))
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::syntax::reader;
    use crate::Interp;

    /// Resolve `form_src` in namespace `ns`, after evaluating each `defs` line to
    /// set up globals. Returns the printed resolved form.
    fn resolved(defs: &[&str], ns: &str, form_src: &str) -> String {
        let mut interp = Interp::new();
        for d in defs {
            interp.eval_str(d).expect("setup def");
        }
        let nssym = value::intern(ns);
        interp.heap.set_compile_ns(Some(nssym));
        let form = reader::read_one(&mut interp.heap, form_src).expect("parse");
        let out = resolve(&mut interp.heap, form);
        crate::syntax::printer::print(&interp.heap, out)
    }

    #[test]
    fn free_ref_qualifies_when_ns_global_exists() {
        assert_eq!(resolved(&["(def foo/bar 1)"], "foo", "(bar)"), "(foo/bar)");
    }

    #[test]
    fn unknown_free_ref_stays_bare() {
        // `baz` is neither an existing `foo/baz` global nor pre-scanned — root
        // fall-through, left bare (would be unbound at worst, never miscompiled).
        assert_eq!(resolved(&[], "foo", "(baz)"), "(baz)");
    }

    #[test]
    fn root_prelude_name_stays_bare() {
        // `map` is a prelude global; there is no `foo/map`, so it stays root.
        assert_eq!(resolved(&[], "foo", "(map f xs)"), "(map f xs)");
    }

    #[test]
    fn definition_head_is_qualified() {
        assert_eq!(resolved(&[], "foo", "(def bar 1)"), "(def foo/bar 1)");
    }

    /// The bare def-heads scan_regions assigns to module `m`, sorted.
    fn region(regions: &HashMap<Symbol, HashSet<Symbol>>, m: &str) -> Vec<String> {
        let mut v: Vec<String> = regions
            .get(&value::intern(m))
            .map(|s| s.iter().map(|&n| value::symbol_name(n)).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn scan_regions_partitions_defs_by_module_boundary() {
        // ADR-223: two modules in one file — each def head belongs to its own region.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defmodule a) (defn x () 1) (defn y () 2) (defmodule b) (defn z () 3)",
        )
        .expect("parse");
        let regions = scan_regions(&interp.heap, &forms);
        assert_eq!(region(&regions, "a"), vec!["x", "y"]);
        assert_eq!(region(&regions, "b"), vec!["z"]);
    }

    #[test]
    fn scan_regions_excludes_pre_module_defs() {
        // A def before the first defmodule is a ROOT def — in no module's region.
        let mut interp = Interp::new();
        let forms = reader::read_all(
            &mut interp.heap,
            "(defn pre () 0) (defmodule a) (defn x () 1)",
        )
        .expect("parse");
        let regions = scan_regions(&interp.heap, &forms);
        assert_eq!(region(&regions, "a"), vec!["x"]); // `pre` is root, not a's

        // scan_def_names is the union of regions — also excludes the pre-module def.
        let flat = scan_def_names(&interp.heap, &forms);
        assert!(flat.contains(&value::intern("x")));
        assert!(!flat.contains(&value::intern("pre")));
    }

    #[test]
    fn local_binding_shadows_and_is_not_qualified() {
        // `foo/x` exists, but the `let`-bound `x` is local — must NOT qualify.
        assert_eq!(
            resolved(&["(def foo/x 1)"], "foo", "(let (x 1) x)"),
            "(let (x 1) x)"
        );
    }

    #[test]
    fn fn_param_is_not_qualified_but_free_body_ref_is() {
        // `x` is a param (local); `bar` is a free ref to a ns global → qualified.
        assert_eq!(
            resolved(&["(def foo/bar 1)"], "foo", "(fn (x) (bar x))"),
            "(fn (x) (foo/bar x))"
        );
    }

    #[test]
    fn quoted_symbol_is_never_qualified() {
        // Data: even though `foo/bar` exists, a quoted `bar` is untouched (ADR-034).
        assert_eq!(
            resolved(&["(def foo/bar 1)"], "foo", "(quote bar)"),
            "(quote bar)"
        );
    }

    #[test]
    fn already_qualified_symbol_passes_through() {
        assert_eq!(
            resolved(&["(def other/bar 1)"], "foo", "(other/bar)"),
            "(other/bar)"
        );
    }

    #[test]
    fn imported_macro_expands_in_the_compile_walk() {
        // The `defprocess` checker bug (ADR-065): a `(:use mod)`-imported macro must
        // expand during macroexpand/compile, not only a directly-bound one. Without
        // this, the compile pass (and the advisory checker) walks the macro's raw
        // body and flags its clause keywords / pattern vars as unbound.
        let mut interp = Interp::new();
        interp
            .eval_str("(defmacro m/double (x) (list (quote +) x x))")
            .unwrap();
        // Simulate a file that did `(defmodule u (:use m))`: compile in `u` with
        // `double` imported as `m/double`.
        interp
            .heap
            .add_import(value::intern("double"), value::intern("m/double"));
        interp.heap.set_compile_ns(Some(value::intern("u")));
        let g = interp.heap.global();
        let form = reader::read_one(&mut interp.heap, "(double 5)").unwrap();
        let out = macroexpand(&mut interp.heap, form, g).unwrap();
        assert_eq!(crate::syntax::printer::print(&interp.heap, out), "(+ 5 5)");
    }

    #[test]
    fn bare_unimported_macro_is_left_unexpanded() {
        // No `(:use)` import and not directly bound → resolution is positive-evidence
        // only, so the bare head stays a raw call (never a false expansion).
        let mut interp = Interp::new();
        interp
            .eval_str("(defmacro m/double (x) (list (quote +) x x))")
            .unwrap();
        interp.heap.set_compile_ns(Some(value::intern("u")));
        let g = interp.heap.global();
        let form = reader::read_one(&mut interp.heap, "(double 5)").unwrap();
        let out = macroexpand(&mut interp.heap, form, g).unwrap();
        assert_eq!(
            crate::syntax::printer::print(&interp.heap, out),
            "(double 5)"
        );
    }

    #[test]
    fn root_namespace_is_identity() {
        // No `(ns …)` active → compile_ns is None → resolve is a no-op even for a
        // name that would otherwise look qualifiable.
        let mut interp = Interp::new();
        interp.eval_str("(def foo/bar 1)").unwrap();
        // compile_ns left as None (root)
        let form = reader::read_one(&mut interp.heap, "(bar)").unwrap();
        let out = resolve(&mut interp.heap, form);
        assert_eq!(crate::syntax::printer::print(&interp.heap, out), "(bar)");
    }

    #[test]
    fn letrec_binders_visible_in_every_rhs() {
        // Mutually-referenced letrec names are local, never qualified, even with a
        // same-named ns global present.
        // (printer renders an empty param list `()` as `nil`)
        assert_eq!(
            resolved(
                &["(def foo/a 9)"],
                "foo",
                "(letrec (a (fn () (b)) b (fn () (a))) (a))"
            ),
            "(letrec (a (fn nil (b)) b (fn nil (a))) (a))"
        );
    }

    #[test]
    fn quasiquote_template_free_refs_qualify_to_defining_ns() {
        // α: a macro template's free ref to a same-namespace name is frozen
        // qualified at definition time; a prelude name (`map`) stays bare; the
        // macro param (`x`, unquoted) stays bare.
        assert_eq!(
            resolved(
                &["(def foo/helper 1)"],
                "foo",
                "(defmacro m (x) `(helper (map ~x)))"
            ),
            "(defmacro foo/m (x) (quasiquote (foo/helper (map (unquote x)))))"
        );
    }

    #[test]
    fn quasiquote_autogensym_and_quoted_stay_bare() {
        // A `#` auto-gensym binder and a quoted symbol inside a template are left
        // bare (not qualified), even with same-named ns globals present.
        assert_eq!(
            resolved(
                &["(def foo/tmp 1)", "(def foo/k 2)"],
                "foo",
                "(defmacro m () `(let (tmp# 1) (quote k)))"
            ),
            "(defmacro foo/m nil (quasiquote (let (tmp# 1) (quote k))))"
        );
    }

    #[test]
    fn match_pattern_binders_are_not_qualified() {
        // The `match*` clause pattern binds `n`; the body ref to `n` must stay local
        // even though `foo/n` exists, while a free `bar` qualifies.
        assert_eq!(
            resolved(
                &["(def foo/n 1)", "(def foo/bar 2)"],
                "foo",
                "(match* :match v (n (bar n)))"
            ),
            "(match* :match v (n (foo/bar n)))"
        );
    }
}
