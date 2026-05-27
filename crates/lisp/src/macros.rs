//! Macro support: quasiquote expansion and `macroexpand`. Heap-threaded.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes, `~x` splices a value, `~@xs`
//! splices the elements of a sequence. Nested quasiquote is not level-tracked
//! (v0.1) — unquotes resolve at the first enclosing quasiquote.

use crate::error::{LispError, LispResult};
use crate::eval;
use crate::heap::Heap;
use crate::value::{self, EnvId, Value};

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
            if value::symbol_name(s) == name {
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
