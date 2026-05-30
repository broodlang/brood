//! Macro support: quasiquote expansion and `macroexpand`. Heap-threaded.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes, `~x` splices a value, `~@xs`
//! splices the elements of a sequence. Nested quasiquote is not level-tracked
//! (v0.1) — unquotes resolve at the first enclosing quasiquote.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;
use std::collections::HashMap;

/// Bound on recursion depth for the quasiquote walker and the compile pass.
/// Past this, return `LispError::runtime` rather than overflowing the native
/// Rust stack — a deeply nested template, vector, or map (from a user file or
/// a misbehaving macro) should produce a clean error, not abort the process.
const MAX_DEPTH: u32 = 256;

/// Per-expansion auto-gensym table (Clojure-style `x#`). Maps a literal template
/// symbol whose name ends in `#` to a single fresh gensym, so every occurrence of
/// that name *within one backtick expansion* refers to the same fresh symbol — and
/// two expansions (or two macro uses) get distinct ones. Holds only interned
/// `Value::Sym`s and `Symbol` (`u32`) keys, both GC-immune (symbols never move and
/// ship by name), so it needs no operand-stack rooting even though quasiquote runs
/// with collection enabled (ADR-061). See `maybe_autogensym`.
type AutoGen = HashMap<Symbol, Value>;

/// Expand a quasiquote template against `env`.
pub fn quasiquote(heap: &mut Heap, template: Value, env: EnvId) -> LispResult {
    let mut autogen = AutoGen::new();
    quasiquote_depth(heap, template, env, 0, &mut autogen)
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
        let name = value::symbol_name(s);
        if name.len() > 1 && name.ends_with('#') {
            return *autogen
                .entry(s)
                .or_insert_with(|| value::gensym(&name[..name.len() - 1]));
        }
    }
    v
}

fn quasiquote_depth(
    heap: &mut Heap,
    template: Value,
    env: EnvId,
    depth: u32,
    autogen: &mut AutoGen,
) -> LispResult {
    if depth >= MAX_DEPTH {
        return Err(LispError::runtime(format!(
            "quasiquote template nested too deeply (max {} levels)",
            MAX_DEPTH
        )));
    }
    if let Some(inner) = tagged(heap, template, "unquote") {
        return eval::eval(heap, inner, env);
    }
    match template {
        Value::Pair(_) => {
            let items = heap.list_to_vec(template)?;
            let out = expand_seq(heap, &items, env, depth + 1, autogen)?;
            Ok(heap.list(out))
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out = expand_seq(heap, &items, env, depth + 1, autogen)?;
            Ok(heap.alloc_vector(out))
        }
        Value::Map(id) => {
            // Expand each key and value (no `~@` splicing into a map — ill-defined).
            // Runtime quasiquote runs with MACRO_BLOCK *off*, so an inner unquote
            // eval can collect at any depth (ADR-061); keep `env`, the unexpanded
            // entries, and the accumulated results on the operand stack.
            let entries = heap.map_entries(id);
            let n = entries.len();
            let vb = heap.roots_len();
            let eb = heap.env_roots_len();
            heap.push_env_root(env);
            for &(k, v) in &entries {
                heap.push_root(k); // vb + 2i
                heap.push_root(v); // vb + 2i + 1
            }
            let res_base = heap.roots_len();
            for i in 0..n {
                let env_now = heap.env_root_at(eb);
                let kf = heap.root_at(vb + 2 * i);
                let k = match quasiquote_depth(heap, kf, env_now, depth + 1, autogen) {
                    Ok(k) => k,
                    Err(e) => return teardown_err(heap, vb, eb, e),
                };
                heap.push_root(k);
                let env_now = heap.env_root_at(eb);
                let vf = heap.root_at(vb + 2 * i + 1);
                let v = match quasiquote_depth(heap, vf, env_now, depth + 1, autogen) {
                    Ok(v) => v,
                    Err(e) => return teardown_err(heap, vb, eb, e),
                };
                heap.push_root(v);
            }
            let mut pairs = Vec::with_capacity(n);
            for i in 0..n {
                pairs.push((heap.root_at(res_base + 2 * i), heap.root_at(res_base + 2 * i + 1)));
            }
            heap.truncate_roots(vb);
            heap.truncate_env_roots(eb);
            Ok(heap.map_from_pairs(pairs))
        }
        other => Ok(maybe_autogensym(other, autogen)),
    }
}

/// Tear down an operand-stack region and return the error (helper for the rooted
/// quasiquote loops, whose `?` would otherwise leak the pushed roots).
fn teardown_err<T>(heap: &mut Heap, vb: usize, eb: usize, e: LispError) -> Result<T, LispError> {
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    Err(e)
}

fn expand_seq(
    heap: &mut Heap,
    items: &[Value],
    env: EnvId,
    depth: u32,
    autogen: &mut AutoGen,
) -> Result<Vec<Value>, LispError> {
    // Each `~unquote` / `~@unquote-splicing` evaluates a sub-form, which can
    // collect at ANY eval depth (ADR-061) — and runtime quasiquote runs with
    // MACRO_BLOCK *off*. So the accumulated `out`, the remaining template `items`,
    // and `env` are LOCAL transients a collection would strand: keep them on the
    // operand stack and read back relocated handles, instead of the plain `Vec`s
    // (whose copies go stale, then `heap.list(out)` would store stale handles).
    let n = items.len();
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_env_root(env);
    for &it in items {
        heap.push_root(it); // vb .. vb+n : unexpanded template elements
    }
    let out_base = heap.roots_len(); // expanded results accumulate here
    for i in 0..n {
        let el = heap.root_at(vb + i);
        let env_now = heap.env_root_at(eb);
        if let Some(inner) = tagged(heap, el, "unquote-splicing") {
            let spliced = match eval::eval(heap, inner, env_now) {
                Ok(s) => s,
                Err(e) => return teardown_err(heap, vb, eb, e),
            };
            let seq = match heap.seq_items(spliced) {
                Ok(s) => s,
                Err(e) => return teardown_err(heap, vb, eb, e),
            };
            for v in seq {
                heap.push_root(v);
            }
        } else {
            match quasiquote_depth(heap, el, env_now, depth, autogen) {
                Ok(v) => heap.push_root(v),
                Err(e) => return teardown_err(heap, vb, eb, e),
            }
        }
    }
    let outn = heap.roots_len() - out_base;
    let mut out = Vec::with_capacity(outn);
    for i in 0..outn {
        out.push(heap.root_at(out_base + i));
    }
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    Ok(out)
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

/// Expand `form` by one step if its head is a macro; returns `(expanded, did_expand)`.
pub fn macroexpand_1(heap: &mut Heap, form: Value, env: EnvId) -> Result<(Value, bool), LispError> {
    if let Value::Pair(p) = form {
        let (head, tail) = heap.pair(p);
        if let Value::Sym(s) = head {
            if let Some(Value::Macro(mid)) = heap.env_get(env, s) {
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
    loop {
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
            let items = match heap.list_to_vec(form) {
                Ok(items) => items,
                Err(_) => return Ok(form), // improper list: leave it be
            };
            if let Some(Value::Sym(s)) = items.first().copied() {
                // quote/quasiquote contents are data, not calls to expand.
                if value::symbol_is(s, "quote") || value::symbol_is(s, "quasiquote") {
                    return Ok(form);
                }
                // Desugar pattern binders into the Brood `match*` engine so they
                // expand once here (fast) rather than per call. eval's `let`/`fn`
                // then only ever see plain symbol binds.
                if value::symbol_is(s, "let") || value::symbol_is(s, "let*") {
                    if let Some(lowered) = lower_let(heap, &items) {
                        return macroexpand_all_depth(heap, lowered, env, depth + 1);
                    }
                    // Ordinary let: expand binding *values* and the body, but not the
                    // binding *targets* — a bound name must not be expanded as a call.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, "letrec") {
                    // Same shape as let: even-indexed binding entries are targets
                    // (opaque), odd-indexed are values (expand). letrec disallows
                    // pattern targets in eval, so there's no `lower_let` branch.
                    return expand_let(heap, original, &items, env, depth + 1);
                } else if value::symbol_is(s, "fn") || value::symbol_is(s, "lambda") {
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
                } else if value::symbol_is(s, "defmacro") {
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
fn fn_is_arity_multi_clause(heap: &Heap, items: &[Value]) -> bool {
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
    v.push(value::sym("do"));
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
    heap.list(vec![value::sym("match*"), value::kw(ctx), valexpr, clause])
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
            heap.list(vec![value::sym("let"), bind, acc])
        } else {
            refutable_bind(heap, "let", valexpr, target, acc)
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
        .position(|&p| matches!(p, Value::Sym(s) if value::symbol_is(s, "&optional") || value::symbol_is(s, "&")))
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
            let params = heap.list(vec![value::sym("&"), g]);
            let mut mexpr = vec![value::sym("match*"), value::kw("fn"), g];
            mexpr.extend_from_slice(clauses); // fn clauses are already match* clauses
            let body = heap.list(mexpr);
            let mut lowered = vec![value::sym("fn"), params];
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
        .position(|&p| matches!(p, Value::Sym(s) if value::symbol_is(s, "&optional") || value::symbol_is(s, "&")))
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
        acc = refutable_bind(heap, "fn", g, pat, acc);
    }
    let new_param_form = match param_form {
        Value::Vector(_) => heap.alloc_vector(new_params),
        _ => heap.list(new_params),
    };
    let mut lowered = vec![value::sym("fn"), new_param_form];
    if let Some(doc) = doc {
        lowered.push(doc); // keep the docstring as the leading body form
    }
    lowered.push(acc);
    Some(heap.list(lowered))
}
