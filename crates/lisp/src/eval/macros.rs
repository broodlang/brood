//! Macro support: quasiquote expansion and `macroexpand`. Heap-threaded.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes, `~x` splices a value, `~@xs`
//! splices the elements of a sequence. Nested quasiquote is not level-tracked
//! (v0.1) — unquotes resolve at the first enclosing quasiquote.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval;

/// Expand a quasiquote template against `env`.
pub fn quasiquote(heap: &mut Heap, template: Value, env: EnvId) -> LispResult {
    if let Some(inner) = tagged(heap, template, "unquote") {
        return eval::eval(heap, inner, env);
    }
    match template {
        Value::Pair(_) => {
            let items = heap.list_to_vec(template)?;
            let out = expand_seq(heap, &items, env)?;
            Ok(heap.list(out))
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let out = expand_seq(heap, &items, env)?;
            Ok(heap.alloc_vector(out))
        }
        Value::Map(id) => {
            // Expand each key and value (no `~@` splicing into a map — ill-defined).
            let entries = heap.map(id).to_vec();
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = quasiquote(heap, k, env)?;
                let v = quasiquote(heap, v, env)?;
                pairs.push((k, v));
            }
            Ok(heap.map_from_pairs(pairs))
        }
        other => Ok(other),
    }
}

fn expand_seq(heap: &mut Heap, items: &[Value], env: EnvId) -> Result<Vec<Value>, LispError> {
    let mut out = Vec::new();
    for &el in items {
        if let Some(inner) = tagged(heap, el, "unquote-splicing") {
            let spliced = eval::eval(heap, inner, env)?;
            out.extend(heap.seq_items(spliced)?);
        } else {
            out.push(quasiquote(heap, el, env)?);
        }
    }
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
    let mut cur = form;
    loop {
        let (next, expanded) = macroexpand_1(heap, cur, env)?;
        if !expanded {
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
                        return macroexpand_all(heap, lowered, env);
                    }
                    // Ordinary let: expand binding *values* and the body, but not the
                    // binding *targets* — a bound name must not be expanded as a call.
                    return expand_let(heap, &items, env);
                } else if value::symbol_is(s, "fn") || value::symbol_is(s, "lambda") {
                    if let Some(lowered) = lower_fn(heap, &items) {
                        return macroexpand_all(heap, lowered, env);
                    }
                    // Ordinary fn: the param list (items[1]) is a binding position,
                    // not a call — expand only the body.
                    return expand_tail(heap, &items, 2, env);
                } else if value::symbol_is(s, "defmacro") {
                    // (defmacro name params body...) — name/params aren't calls.
                    return expand_tail(heap, &items, 3, env);
                }
            }
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all(heap, item, env)?);
            }
            Ok(heap.list(out))
        }
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(macroexpand_all(heap, item, env)?);
            }
            Ok(heap.alloc_vector(out))
        }
        Value::Map(id) => {
            // Walk a map literal's keys and values so macros inside them expand
            // once here. Keep it a literal map (the evaluator canonicalises it).
            let entries = heap.map(id).to_vec();
            let mut pairs = Vec::with_capacity(entries.len());
            for (k, v) in entries {
                let k = macroexpand_all(heap, k, env)?;
                let v = macroexpand_all(heap, v, env)?;
                pairs.push((k, v));
            }
            Ok(heap.alloc_map(pairs))
        }
        other => Ok(other),
    }
}

/// Rebuild a form expanding only `items[start..]` (the call's body/argument tail),
/// leaving `items[..start]` opaque. Used to skip binding positions — a fn/defmacro
/// parameter list — so a name there is never mistaken for a macro call.
fn expand_tail(heap: &mut Heap, items: &[Value], start: usize, env: EnvId) -> LispResult {
    let start = start.min(items.len());
    let mut out = items[..start].to_vec();
    for &item in &items[start..] {
        out.push(macroexpand_all(heap, item, env)?);
    }
    Ok(heap.list(out))
}

/// Expand an ordinary `let`: its binding *values* (odd positions of the binding
/// list) and its body, leaving the binding *targets* (even positions) opaque.
fn expand_let(heap: &mut Heap, items: &[Value], env: EnvId) -> LispResult {
    let Some(bindings) = items.get(1).copied() else {
        return Ok(heap.list(items.to_vec()));
    };
    let new_bindings = match form_items(heap, bindings) {
        Some(binds) => {
            let mut nb = Vec::with_capacity(binds.len());
            for (i, &x) in binds.iter().enumerate() {
                // odd index = a value expression (expand); even = a target (opaque)
                nb.push(if i % 2 == 1 {
                    macroexpand_all(heap, x, env)?
                } else {
                    x
                });
            }
            match bindings {
                Value::Vector(_) => heap.alloc_vector(nb),
                _ => heap.list(nb),
            }
        }
        None => bindings,
    };
    let mut out = vec![items[0], new_bindings];
    for &item in &items[2..] {
        out.push(macroexpand_all(heap, item, env)?);
    }
    Ok(heap.list(out))
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
        return true; // multi-clause
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
