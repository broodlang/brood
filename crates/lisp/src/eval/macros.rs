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
use crate::core::value::{self, ClosureId, EnvId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;
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
    if let Value::Sym(s) = v {
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
    Value::Sym(value::intern(name))
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
            "quasiquote template nested too deeply (max {} levels)",
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
    match v {
        Value::Pair(_) => {
            let items = heap.list_to_vec(v)?;
            qq_seq(heap, &items, false, depth + 1, autogen)
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            qq_seq(heap, &items, true, depth + 1, autogen)
        }
        Value::Map(id) => {
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
        // A literal symbol is data — quote it (auto-gensym `x#` first).
        Value::Sym(_) => {
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
    if let Value::Pair(p) = v {
        let (head, tail) = heap.pair(p);
        if let Value::Sym(s) = head {
            if value::symbol_is(s, name) {
                if let Value::Pair(p2) = tail {
                    return Some(heap.car(p2));
                }
            }
        }
    }
    None
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

/// The compile pass for one top-level form: expand macros, then resolve
/// namespaces. Every loader/driver runs forms through here before `eval` so the
/// runtime evaluator never sees an unexpanded macro or an unqualified namespaced
/// reference. At root (`compile_ns == None`) the resolve step is a no-op.
pub fn compile(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    let expanded = macroexpand_all(heap, form, env)?;
    Ok(resolve(heap, expanded))
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
    for &f in forms {
        if let Ok(items) = heap.list_to_vec(f) {
            if let Some(&Value::Sym(h)) = items.first() {
                if value::symbol_is(h, kw::DEFMODULE) {
                    if let Some(&Value::Sym(name)) = items.get(1) {
                        return Some(name);
                    }
                }
            }
        }
    }
    None
}

/// Pre-scan UNEXPANDED top-level forms for the bare names a file will define
/// (`def`/`defn`/`defmacro`/`defdyn` heads, recursively), so the resolver can
/// qualify a *forward* reference to a same-namespace name defined later in the
/// file. Skips `quote`/`quasiquote` data.
pub fn scan_def_names(heap: &Heap, forms: &[Value]) -> HashSet<Symbol> {
    let mut names = HashSet::new();
    for &form in forms {
        scan_def_form(heap, form, &mut names);
    }
    names
}

fn scan_def_form(heap: &Heap, form: Value, names: &mut HashSet<Symbol>) {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return,
    };
    let Some(&Value::Sym(h)) = items.first() else {
        return;
    };
    if value::symbol_is(h, kw::QUOTE) || value::symbol_is(h, kw::QUASIQUOTE) {
        return;
    }
    let hn = value::symbol_name_ref(h);
    if matches!(hn, kw::DEF | kw::DEFN | kw::DEFMACRO | kw::DEFDYN) {
        if let Some(&Value::Sym(name)) = items.get(1) {
            // Only bare names get pre-recorded; an already-qualified def head needs
            // no forward-ref help.
            if !value::symbol_name_ref(name).contains('/') {
                names.insert(name);
            }
        }
    }
    // Recurse so a def nested in a top-level `(do …)`/`(when …)` is still found.
    for &it in &items[1..] {
        scan_def_form(heap, it, names);
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

/// An "earmuffed" `*foo*` name — by Lisp convention a special/dynamic/ambient
/// global. These are never namespaced (ADR-065): a `(def *load-path* …)` in any
/// namespace rebinds the *root* `*load-path*` the loader reads, rather than a
/// namespace-local shadow — likewise `*features*`, `*project-*`, `defdyn` vars.
fn is_ambient(name: &str) -> bool {
    name.len() > 2 && name.starts_with('*') && name.ends_with('*')
}

/// Qualify a definition head: `bar` -> `ns/bar`; an already-`/`-qualified name, or
/// an ambient `*earmuffed*` name, is taken as-is. Shared with `Heap::def_form_name`
/// so def-site keys match.
pub fn qualify_name(ns_name: &str, name: value::Symbol) -> value::Symbol {
    let spelling = value::symbol_name_ref(name);
    if spelling.contains('/') || is_ambient(spelling) {
        name
    } else {
        value::intern(&format!("{}/{}", ns_name, spelling))
    }
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
        return s; // already qualified, no alias
    }
    if is_ambient(name) {
        return s; // earmuffed `*foo*` — ambient/root by convention (ADR-065)
    }
    // Own namespace first (a same-named local def shadows an import), then a
    // `(:use …)` import, then root/prelude fall-through (left bare).
    let qsym = value::intern(&format!("{}/{}", ns_name, name));
    if heap.ns_knows_name(s) || heap.env_get(value::EnvId::GLOBAL, qsym).is_some() {
        qsym
    } else if let Some(imported) = heap.import_of(s) {
        imported
    } else {
        s
    }
}

fn resolve_walk(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    match form {
        Value::Sym(s) => Value::Sym(resolve_sym(heap, s, ns_name, locals)),
        Value::Pair(_) => resolve_list(heap, form, ns_name, locals),
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(resolve_walk(heap, it, ns_name, locals));
            }
            heap.alloc_vector(out)
        }
        Value::Map(id) => {
            let entries = heap.map_entries(id);
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = resolve_walk(heap, k, ns_name, locals);
                let v = resolve_walk(heap, v, ns_name, locals);
                pairs.push((k, v));
            }
            heap.map_from_pairs(pairs)
        }
        other => other,
    }
}

fn resolve_list(heap: &mut Heap, form: Value, ns_name: &str, locals: &[value::Symbol]) -> Value {
    let items = match heap.list_to_vec(form) {
        Ok(i) => i,
        Err(_) => return form, // improper list — leave verbatim
    };
    if let Some(&Value::Sym(h)) = items.first() {
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
    let is_defmacro =
        matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, kw::DEFMACRO));
    let mut out = Vec::with_capacity(items.len());
    out.push(items[0]); // def / defmacro head, verbatim
    match items.get(1) {
        Some(&Value::Sym(name)) => {
            // Register the (bare) name as known before resolving the value, so a
            // self-reference in the body — e.g. the recursion in a `defprocess`-
            // generated receive loop — qualifies to the same `ns/name` the head
            // gets. `scan_def_names` misses macro-defined names (it scans the raw,
            // unexpanded form), so without this a `(defn counter … (counter …))`
            // that came from a macro would bind `ns/counter` but recurse on bare
            // `counter` → unbound. Harmless for an already-qualified name.
            if !value::symbol_name_ref(name).contains('/') {
                heap.add_ns_known_name(name);
            }
            out.push(Value::Sym(qualify_name(ns_name, name)));
        }
        Some(&other) => out.push(other), // not a symbol — leave (eval will complain)
        None => return rebuild_list(heap, form, out),
    }
    if is_defmacro {
        let params = items.get(2).copied().unwrap_or(Value::Nil);
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
    let (has_doc, clause_start) = match parts.first() {
        Some(Value::Str(_)) if parts.len() > 1 => (true, 1),
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
        let params = parts.first().copied().unwrap_or(Value::Nil);
        out.push(params); // verbatim
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
    out.push(cparts[0]); // params verbatim
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
    let letrec = matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, kw::LETREC));
    let binds = match items.get(1).and_then(|&b| form_items(heap, b)) {
        Some(b) if b.len() % 2 == 0 => b,
        _ => return generic_resolve(heap, form, items, ns_name, locals),
    };
    let mut scope = locals.to_vec();
    if letrec {
        for &t in binds.iter().step_by(2) {
            if let Value::Sym(s) = t {
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
            if let Value::Sym(s) = target {
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

/// Collect parameter-binder symbols from a param list (mirrors `fn_params` /
/// `parse_params`): plain symbols, `(name default)` optionals' names; skips the
/// `&`/`&optional`/`&rest` markers. Appends to `out`.
fn collect_param_syms(heap: &Heap, params: Value, out: &mut Vec<value::Symbol>) {
    let items = match form_items(heap, params) {
        Some(i) => i,
        None => return,
    };
    for item in items {
        match item {
            Value::Sym(s) => {
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                out.push(s);
            }
            Value::Pair(_) | Value::Vector(_) => {
                // `(name default)` — the binder is the first element.
                let inner = form_items(heap, item).unwrap_or_default();
                if let Some(&Value::Sym(s)) = inner.first() {
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
    match v {
        Value::Sym(s) => out.push(s),
        Value::Pair(_) => {
            if let Ok(items) = heap.list_to_vec(v) {
                for it in items {
                    collect_all_syms(heap, it, out);
                }
            }
        }
        Value::Vector(id) => {
            for it in heap.vector(id).to_vec() {
                collect_all_syms(heap, it, out);
            }
        }
        Value::Map(id) => {
            for (k, val) in heap.map_entries(id) {
                collect_all_syms(heap, k, out);
                collect_all_syms(heap, val, out);
            }
        }
        _ => {}
    }
}

/// Rebuild a binding container preserving list-vs-vector shape (and position).
fn rebuild_seq_like(heap: &mut Heap, original: Value, items: Vec<Value>) -> Value {
    match original {
        Value::Vector(_) => heap.alloc_vector(items),
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
    match heap.env_get(env, sym) {
        Some(Value::Macro(mid)) => Some(mid),
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
            match (q != sym, heap.env_get(value::EnvId::GLOBAL, q)) {
                (true, Some(Value::Macro(mid))) => Some(mid),
                _ => None,
            }
        }
    }
}

/// Expand `form` by one step if its head is a macro; returns `(expanded, did_expand)`.
pub fn macroexpand_1(heap: &mut Heap, form: Value, env: EnvId) -> Result<(Value, bool), LispError> {
    if let Value::Pair(p) = form {
        let (head, tail) = heap.pair(p);
        if let Value::Sym(s) = head {
            if let Some(mid) = macro_head_id(heap, env, s) {
                let args = heap.list_to_vec(tail)?;
                let expanded = eval::apply_closure(heap, mid, &args)?;
                return Ok((expanded, true));
            }
        }
    }
    Ok((form, false))
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
    // Block GC during the expansion: this walk holds partially-built LOCAL forms
    // in Rust locals and recurses into macro applications via `eval`, whose
    // safepoint would otherwise sweep them. The runtime evaluator roots its
    // transients on the operand stack so its safepoint fires at any depth
    // (ADR-061) — but the compile pass opts out instead: `MacroBlockGuard` keeps
    // `MACRO_BLOCK > 0` for the expansion, and the safepoint skips collection
    // while that holds. Expansion is bounded per form, so memory grows briefly
    // (reclaimed at the next runtime safepoint). The `GcBlockGuard` is kept too,
    // purely for the stack-depth accounting it feeds. See `docs/memory-model.md`.
    let _gc_block = crate::process::GcBlockGuard::enter();
    let _macro_block = crate::process::MacroBlockGuard::enter();
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "macro expansion nested too deeply (max {} levels)",
            MAX_DEPTH
        )));
    }
    let original = form;
    let form = macroexpand(heap, form, env)?;
    match form {
        Value::Pair(_) => {
            let mut items = match heap.list_to_vec(form) {
                Ok(items) => items,
                Err(_) => return Ok(form), // improper list: leave it be
            };
            if let Some(Value::Sym(head)) = items.first().copied() {
                // quote/quasiquote contents are data, not calls to expand.
                if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
                    return Ok(form);
                }
                // `lambda` / `let*` are synonyms for `fn` / `let`. Canonicalise the
                // head now — *after* the quote guard (so quoted data keeps its
                // spelling) and *before* lowering — so the whole downstream pipeline
                // (pattern lowering here, the VM compile pass, and the tree-walker's
                // lowering re-entry) only ever sees `fn` / `let`. `let*` aliases `let`
                // because Brood's `let` is already sequential.
                let s = if value::symbol_is(head, kw::LAMBDA) {
                    items[0] = value::sym(kw::FN);
                    value::intern(kw::FN)
                } else if value::symbol_is(head, kw::LET_STAR) {
                    items[0] = value::sym(kw::LET);
                    value::intern(kw::LET)
                } else {
                    head
                };
                // Desugar pattern binders into the Brood `match*` engine so they
                // expand once here (fast) rather than per call. eval's `let`/`fn`
                // then only ever see plain symbol binds.
                if value::symbol_is(s, kw::LET) {
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
                    if let Some(lowered) = lower_fn(heap, &items) {
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
            Ok(rebuild_list(heap, original, out))
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all_depth(heap, item, env, depth + 1)?);
            }
            Ok(heap.alloc_vector(out))
        }
        Value::Map(id) => {
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
    let pos = heap.form_pos(original);
    let new_list = heap.list(items);
    if let Some(p) = pos {
        heap.set_form_pos(new_list, p);
    }
    new_list
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
    let forms = match forms.first() {
        Some(&Value::Str(_)) if forms.len() > 1 => &forms[1..],
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
    if matches!(items.get(1), Some(&Value::Str(_))) && items.len() > 2 {
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
            match bindings {
                Value::Vector(_) => heap.alloc_vector(nb),
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
    match v {
        Value::Nil => Some(Vec::new()),
        Value::Pair(_) => heap.list_to_vec(v).ok(),
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        _ => None,
    }
}

fn is_sym(v: Value) -> bool {
    matches!(v, Value::Sym(_))
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
    match f {
        Value::Pair(p) => matches!(heap.car(p), Value::Pair(_) | Value::Nil),
        _ => false,
    }
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
    match f {
        Value::Pair(p) => {
            let head = heap.car(p);
            matches!(head, Value::Pair(_) | Value::Nil) && is_arity_param_list(heap, head)
        }
        _ => false,
    }
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
    let forms = match forms.first() {
        Some(&Value::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    if forms.is_empty() {
        return false;
    }
    if forms.iter().all(|&f| is_clause(heap, f)) {
        // Multi-clause. Arity-only clauses dispatch natively (`make_closure`
        // builds `ClosureArm`s), so they DON'T need `match*` lowering; only a
        // clause carrying a literal/destructuring pattern does.
        return !forms.iter().all(|&f| is_arity_clause(heap, f));
    }
    // single-clause: a pattern in a required slot (before &optional / & rest)?
    let params = match form_items(heap, forms[0]) {
        Some(p) => p,
        None => return false,
    };
    let required_end = params
        .iter()
        .position(|&p| matches!(p, Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    params[..required_end].iter().any(|&p| !is_sym(p))
}

/// Lower a `fn` that is multi-clause, or single-clause with pattern(s) in its
/// required parameters, into a plain `fn` plus the Brood `match*` engine.
/// Returns `None` for an ordinary single-clause `fn` (left as-is).
fn lower_fn(heap: &mut Heap, items: &[Value]) -> Option<Value> {
    let forms = &items[1..];

    // Multi-clause: an optional leading docstring, then every form a clause. The
    // docstring sits *before* the clauses here (a single-clause fn's docstring
    // sits after the param list and is peeled below); keep it as the lowered
    // fn's leading body form so `make_closure` still finds it.
    {
        let (doc, clauses): (Option<Value>, &[Value]) = match forms.first() {
            Some(&Value::Str(_)) if forms.len() > 1 => (Some(forms[0]), &forms[1..]),
            _ => (None, forms),
        };
        if !clauses.is_empty() && clauses.iter().all(|&f| is_clause(heap, f)) {
            // This IS a multi-clause fn — never fall through to the single-clause
            // path below (which would misread the first clause as a param list).
            if clauses.iter().all(|&f| is_arity_clause(heap, f)) {
                // Arity-only: dispatches natively (the evaluator's `make_closure`
                // builds one `ClosureArm` per clause, bound by argument count — no
                // rest-list, no `match*`). Leave it un-lowered.
                return None;
            }
            // At least one literal/destructuring *pattern* clause → lower the whole
            // dispatch to the `match*` engine.
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
            return Some(heap.list(lowered));
        }
    }

    // Single-clause: forms[0] is the parameter list, forms[1..] the body.
    let param_form = *forms.first()?;
    let body = &forms[1..];
    let params = form_items(heap, param_form)?;

    // Peel a leading docstring (a string literal with more body after it) so it
    // stays the *first* form of the lowered `fn` — otherwise `make_closure`'s
    // docstring detection misses it once the body is wrapped in the refutable
    // bind + `do`. (`(fn ([x y]) "doc" body)` would lose its doc otherwise.)
    let (doc, body) = match body.first() {
        Some(&Value::Str(_)) if body.len() > 1 => (Some(body[0]), &body[1..]),
        _ => (None, body),
    };

    // Patterns are allowed only in required slots (before &optional / & rest).
    let required_end = params
        .iter()
        .position(|&p| matches!(p, Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) || value::symbol_is(s, kw::AMP)))
        .unwrap_or(params.len());
    if !params[..required_end].iter().any(|&p| !is_sym(p)) {
        return None; // no pattern in the required params — ordinary fn
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
    let new_param_form = match param_form {
        Value::Vector(_) => heap.alloc_vector(new_params),
        _ => heap.list(new_params),
    };
    let mut lowered = vec![value::sym(kw::FN), new_param_form];
    if let Some(doc) = doc {
        lowered.push(doc); // keep the docstring as the leading body form
    }
    lowered.push(acc);
    Some(heap.list(lowered))
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
