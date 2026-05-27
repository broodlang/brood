//! Macro support: quasiquote template expansion and `macroexpand`.
//!
//! A macro is a [`Value::Macro`](crate::value::Value) — a closure invoked on the
//! *unevaluated* argument forms at expansion time, whose result is then
//! evaluated in place (see `eval.rs`). Quasiquote is the template language used
//! to build that result.
//!
//! Syntax (Clojure-style): `` `tmpl `` quotes a template, `~x` splices the value
//! of `x` into it, and `~@xs` splices the *elements* of the sequence `xs`.
//!
//! Limitation (v0.1): nested quasiquote is not level-tracked. Unquotes are
//! resolved at the first enclosing quasiquote. This is enough for ordinary
//! macros; full nesting can come later.

use std::rc::Rc;

use crate::env::Env;
use crate::error::{LispError, LispResult};
use crate::eval;
use crate::value::{self, Value};

/// Expand a quasiquote template against `env`.
pub fn quasiquote(template: &Value, env: &Rc<Env>) -> LispResult {
    // `~x` at the top level: evaluate and return it directly.
    if let Some(inner) = tagged(template, "unquote") {
        return eval::eval(inner, env.clone());
    }
    match template {
        Value::Pair(_) => {
            let items = value::list_to_vec(template)?;
            Ok(value::list(expand_seq(&items, env)?))
        }
        Value::Vector(items) => Ok(Value::Vector(Rc::new(expand_seq(items, env)?))),
        other => Ok(other.clone()),
    }
}

/// Build the elements of a quasiquoted list/vector, honouring `~@` splices.
fn expand_seq(items: &[Value], env: &Rc<Env>) -> Result<Vec<Value>, LispError> {
    let mut out = Vec::new();
    for el in items {
        if let Some(inner) = tagged(el, "unquote-splicing") {
            let spliced = eval::eval(inner, env.clone())?;
            out.extend(seq_to_vec(&spliced)?);
        } else {
            out.push(quasiquote(el, env)?);
        }
    }
    Ok(out)
}

/// If `v` is a two-element list `(name x)` with the given head symbol, return `x`.
fn tagged(v: &Value, name: &str) -> Option<Value> {
    if let Value::Pair(p) = v {
        if let Value::Sym(s) = &p.0 {
            if value::symbol_name(*s) == name {
                if let Value::Pair(rest) = &p.1 {
                    return Some(rest.0.clone());
                }
            }
        }
    }
    None
}

fn seq_to_vec(v: &Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Pair(_) => value::list_to_vec(v),
        Value::Vector(items) => Ok((**items).clone()),
        _ => Err(LispError::type_err(format!(
            "unquote-splicing (~@) expects a list or vector, got {}",
            crate::printer::print(v)
        ))),
    }
}

/// Expand `form` by one step if its head is a macro; returns `(expanded, did_expand)`.
pub fn macroexpand_1(form: &Value, env: &Rc<Env>) -> Result<(Value, bool), LispError> {
    if let Value::Pair(p) = form {
        if let Value::Sym(s) = &p.0 {
            if let Some(Value::Macro(m)) = env.get(*s) {
                let args = value::list_to_vec(&p.1)?;
                let expanded = eval::apply_closure(&m, &args)?;
                return Ok((expanded, true));
            }
        }
    }
    Ok((form.clone(), false))
}

/// Repeatedly expand `form` until its head is no longer a macro.
pub fn macroexpand(form: &Value, env: &Rc<Env>) -> LispResult {
    let mut cur = form.clone();
    loop {
        let (next, expanded) = macroexpand_1(&cur, env)?;
        if !expanded {
            return Ok(next);
        }
        cur = next;
    }
}
