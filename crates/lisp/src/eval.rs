//! The evaluator: the "eval" of read-eval-print.
//!
//! A tree-walking interpreter. The single most important structural choice here
//! is the `'tail: loop`: instead of recursing for forms in *tail position*
//! (the last expression of a body, an `if` branch, the call in a tail call), we
//! reassign `expr`/`env` and loop. That gives proper tail calls, so deep
//! recursion in mylisp does not grow the Rust stack.
//!
//! Special forms are recognised by the head symbol's name and handled before
//! ordinary function application. v0.1 has no macros yet (see the roadmap); the
//! handful of forms below is enough to be a real language.

use std::rc::Rc;

use crate::env::Env;
use crate::error::{LispError, LispResult};
use crate::value::{self, Closure, Symbol, Value};

/// mylisp truthiness: only `nil` and `false` are falsy. Everything else (`0`,
/// `""`, empty collections, ...) is truthy.
pub fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

pub fn eval(expr: Value, env: Rc<Env>) -> LispResult {
    let mut expr = expr;
    let mut env = env;

    'tail: loop {
        // Atoms and the two evaluating literals resolve immediately.
        match &expr {
            Value::Sym(s) => {
                return env.get(*s).ok_or_else(|| {
                    LispError::unbound(format!("unbound symbol: {}", value::symbol_name(*s)))
                });
            }
            Value::Vector(items) => {
                // A vector literal evaluates its elements.
                let mut out = Vec::with_capacity(items.len());
                for item in items.iter() {
                    out.push(eval(item.clone(), env.clone())?);
                }
                return Ok(value::vector(out));
            }
            Value::Pair(_) => {} // a combination — handled below
            _ => return Ok(expr.clone()),
        }

        let (head, rest) = match &expr {
            Value::Pair(p) => (p.0.clone(), p.1.clone()),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let Value::Sym(s) = &head {
            let name = value::symbol_name(*s);
            match name.as_str() {
                "quote" => {
                    let args = value::list_to_vec(&rest)?;
                    return Ok(args.into_iter().next().unwrap_or(Value::Nil));
                }
                "if" => {
                    let args = value::list_to_vec(&rest)?;
                    let test = eval(nth(&args, 0), env.clone())?;
                    expr = if truthy(&test) { nth(&args, 1) } else { nth(&args, 2) };
                    continue 'tail;
                }
                "when" => {
                    let args = value::list_to_vec(&rest)?;
                    let test = eval(nth(&args, 0), env.clone())?;
                    if !truthy(&test) {
                        return Ok(Value::Nil);
                    }
                    match tail_of(&args, 1, &env)? {
                        Some(last) => {
                            expr = last;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "unless" => {
                    let args = value::list_to_vec(&rest)?;
                    let test = eval(nth(&args, 0), env.clone())?;
                    if truthy(&test) {
                        return Ok(Value::Nil);
                    }
                    match tail_of(&args, 1, &env)? {
                        Some(last) => {
                            expr = last;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "cond" => {
                    // Clojure-style flat pairs: (cond test1 expr1 test2 expr2 ...).
                    // A test of `else` or `:else` always matches (the fallback).
                    let args = value::list_to_vec(&rest)?;
                    if args.len() % 2 != 0 {
                        return Err(LispError::runtime(
                            "cond: expected an even number of test/expression forms",
                        ));
                    }
                    let mut chosen: Option<Value> = None;
                    let mut i = 0;
                    while i < args.len() {
                        let test = if is_else_keyword(&args[i]) {
                            Value::Bool(true)
                        } else {
                            eval(args[i].clone(), env.clone())?
                        };
                        if truthy(&test) {
                            chosen = Some(args[i + 1].clone());
                            break;
                        }
                        i += 2;
                    }
                    match chosen {
                        Some(last) => {
                            expr = last;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "do" => match tail_of(&value::list_to_vec(&rest)?, 0, &env)? {
                    Some(last) => {
                        expr = last;
                        continue 'tail;
                    }
                    None => return Ok(Value::Nil),
                },
                "def" => {
                    let args = value::list_to_vec(&rest)?;
                    let name = as_symbol(
                        args.first().ok_or_else(|| LispError::runtime("def: missing name"))?,
                    )?;
                    let val = if args.len() > 1 {
                        eval(args[1].clone(), env.clone())?
                    } else {
                        Value::Nil
                    };
                    let val = name_value(val, name);
                    Env::root(&env).define(name, val);
                    return Ok(Value::Sym(name));
                }
                "set!" => {
                    let args = value::list_to_vec(&rest)?;
                    let name = as_symbol(
                        args.first().ok_or_else(|| LispError::runtime("set!: missing name"))?,
                    )?;
                    let val = eval(nth(&args, 1), env.clone())?;
                    if env.set_existing(name, val.clone()) {
                        return Ok(val);
                    }
                    return Err(LispError::unbound(format!(
                        "set!: cannot set undefined symbol '{}'",
                        value::symbol_name(name)
                    )));
                }
                "fn" | "lambda" => return make_closure(None, &rest, &env),
                "quasiquote" => {
                    let args = value::list_to_vec(&rest)?;
                    let template = args.into_iter().next().unwrap_or(Value::Nil);
                    return crate::macros::quasiquote(&template, &env);
                }
                "defmacro" => {
                    // (defmacro name [params] body...) — same shape as fn, but
                    // the resulting closure is tagged as a macro.
                    let parts = value::list_to_vec(&rest)?;
                    let name = as_symbol(
                        parts.first().ok_or_else(|| LispError::runtime("defmacro: missing name"))?,
                    )?;
                    let fn_form = value::list(parts[1..].to_vec());
                    let macro_val = match make_closure(Some(name), &fn_form, &env)? {
                        Value::Fn(cl) => Value::Macro(cl),
                        other => other,
                    };
                    Env::root(&env).define(name, macro_val);
                    return Ok(Value::Sym(name));
                }
                "let" | "let*" => {
                    let args = value::list_to_vec(&rest)?;
                    let binds = as_binding_vec(
                        args.first().ok_or_else(|| LispError::runtime("let: missing bindings"))?,
                    )?;
                    if binds.len() % 2 != 0 {
                        return Err(LispError::runtime("let: bindings must be name/value pairs"));
                    }
                    let scope = Env::child(&env);
                    let mut i = 0;
                    while i < binds.len() {
                        let bind_name = as_symbol(&binds[i])?;
                        // Sequential: each binding can see the ones before it.
                        let val = eval(binds[i + 1].clone(), scope.clone())?;
                        scope.define(bind_name, val);
                        i += 2;
                    }
                    match tail_of(&args, 1, &scope)? {
                        Some(last) => {
                            expr = last;
                            env = scope;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "and" => {
                    let args = value::list_to_vec(&rest)?;
                    if args.is_empty() {
                        return Ok(Value::Bool(true));
                    }
                    for form in &args[..args.len() - 1] {
                        let v = eval(form.clone(), env.clone())?;
                        if !truthy(&v) {
                            return Ok(v);
                        }
                    }
                    expr = args[args.len() - 1].clone();
                    continue 'tail;
                }
                "or" => {
                    let args = value::list_to_vec(&rest)?;
                    if args.is_empty() {
                        return Ok(Value::Nil);
                    }
                    for form in &args[..args.len() - 1] {
                        let v = eval(form.clone(), env.clone())?;
                        if truthy(&v) {
                            return Ok(v);
                        }
                    }
                    expr = args[args.len() - 1].clone();
                    continue 'tail;
                }
                "while" => {
                    let args = value::list_to_vec(&rest)?;
                    let test = nth(&args, 0);
                    let body = if args.len() > 1 { &args[1..] } else { &[] };
                    loop {
                        if !truthy(&eval(test.clone(), env.clone())?) {
                            break;
                        }
                        for form in body {
                            eval(form.clone(), env.clone())?;
                        }
                    }
                    return Ok(Value::Nil);
                }
                _ => {} // not a special form: fall through to application
            }
        }

        // --- macro expansion ---
        // If the head symbol names a macro, expand it (on the *unevaluated* arg
        // forms) and loop on the result — so the expansion is itself eligible
        // for tail-call treatment and further macro expansion.
        if let Value::Sym(s) = &head {
            if let Some(Value::Macro(m)) = env.get(*s) {
                let arg_forms = value::list_to_vec(&rest)?;
                expr = apply_closure(&m, &arg_forms)?;
                continue 'tail;
            }
        }

        // --- function application ---
        let callee = eval(head.clone(), env.clone())?;
        let arg_forms = value::list_to_vec(&rest)?;
        let mut argv = Vec::with_capacity(arg_forms.len());
        for form in arg_forms {
            argv.push(eval(form, env.clone())?);
        }
        match callee {
            Value::Native(nf) => return (nf.func)(&argv, &env),
            Value::Fn(cl) => {
                let scope = bind_params(&cl, &argv)?;
                match tail_of_vec(&cl.body, &scope)? {
                    Some(last) => {
                        expr = last;
                        env = scope;
                        continue 'tail;
                    }
                    None => return Ok(Value::Nil),
                }
            }
            other => {
                return Err(LispError::type_err(format!(
                    "cannot call non-function: {}",
                    crate::printer::print(&other)
                )))
            }
        }
    }
}

/// Apply a callable to already-evaluated arguments. Used by `apply`, `map`,
/// `reduce`, etc. (Unlike the inline path in `eval`, this does not get TCO, but
/// each body it runs still recurses into the TCO-aware `eval`.)
pub fn apply(callee: &Value, argv: &[Value], _env: &Rc<Env>) -> LispResult {
    match callee {
        Value::Native(nf) => (nf.func)(argv, _env),
        Value::Fn(cl) => apply_closure(cl, argv),
        other => Err(LispError::type_err(format!(
            "not a function: {}",
            crate::printer::print(other)
        ))),
    }
}

pub fn apply_closure(cl: &Rc<Closure>, argv: &[Value]) -> LispResult {
    let scope = bind_params(cl, argv)?;
    let mut result = Value::Nil;
    for form in &cl.body {
        result = eval(form.clone(), scope.clone())?;
    }
    Ok(result)
}

fn bind_params(cl: &Closure, argv: &[Value]) -> Result<Rc<Env>, LispError> {
    let scope = Env::child(&cl.env);
    let who = cl.name.map(value::symbol_name).unwrap_or_else(|| "fn".to_string());
    let required = cl.params.len();
    let max = if cl.rest.is_some() { usize::MAX } else { required + cl.optionals.len() };

    if argv.len() < required || argv.len() > max {
        return Err(LispError::arity(arity_message(
            &who,
            required,
            cl.optionals.len(),
            cl.rest.is_some(),
            argv.len(),
        )));
    }

    // Required parameters, positionally.
    for (param, arg) in cl.params.iter().zip(argv.iter()) {
        scope.define(*param, arg.clone());
    }

    // &optional parameters: take the next positional arg, or evaluate the
    // default in the scope built so far (left-to-right, so defaults can refer
    // to earlier parameters).
    let mut idx = required;
    for (name, default_form) in &cl.optionals {
        if idx < argv.len() {
            scope.define(*name, argv[idx].clone());
            idx += 1;
        } else {
            let value = eval(default_form.clone(), scope.clone())?;
            scope.define(*name, value);
        }
    }

    // &rest gets whatever is left.
    if let Some(rest) = cl.rest {
        scope.define(rest, value::list(argv[idx..].to_vec()));
    }

    Ok(scope)
}

fn arity_message(who: &str, required: usize, optionals: usize, has_rest: bool, got: usize) -> String {
    let expected = if has_rest {
        format!("at least {}", required)
    } else if optionals == 0 {
        format!("{}", required)
    } else {
        format!("{} to {}", required, required + optionals)
    };
    format!("{}: expected {} args, got {}", who, expected, got)
}

fn make_closure(name: Option<Symbol>, rest: &Value, env: &Rc<Env>) -> LispResult {
    let parts = value::list_to_vec(rest)?;
    let param_form =
        parts.first().ok_or_else(|| LispError::runtime("fn: missing parameter list"))?;
    let (params, optionals, rest_param) = parse_params(param_form)?;
    let body = parts[1..].to_vec();
    Ok(value::closure(Closure {
        name,
        params,
        optionals,
        rest: rest_param,
        body,
        env: env.clone(),
    }))
}

/// Parse a parameter list (a list `(x y)` or vector `[x y]`) into required
/// params, `&optional` params (with default forms), and an optional `& rest`.
/// See `docs/spec.md` §7.4 for the grammar.
fn parse_params(
    form: &Value,
) -> Result<(Vec<Symbol>, Vec<(Symbol, Value)>, Option<Symbol>), LispError> {
    let items = match form {
        Value::Vector(items) => (**items).clone(),
        Value::Pair(_) => value::list_to_vec(form)?,
        Value::Nil => Vec::new(),
        _ => return Err(LispError::type_err("parameter list must be a list (x y) or vector [x y]")),
    };

    let mut required = Vec::new();
    let mut optionals = Vec::new();
    let mut rest = None;
    let mut in_optional = false;
    let mut i = 0;

    while i < items.len() {
        // Markers (&optional, &) are recognised structurally.
        if let Value::Sym(s) = &items[i] {
            let name = value::symbol_name(*s);
            if name == "&optional" {
                if in_optional {
                    return Err(LispError::runtime("&optional may appear only once"));
                }
                in_optional = true;
                i += 1;
                continue;
            }
            if name == "&" {
                let r = items
                    .get(i + 1)
                    .ok_or_else(|| LispError::runtime("expected a symbol after & in parameter list"))?;
                rest = Some(as_symbol(r)?);
                if i + 2 < items.len() {
                    return Err(LispError::runtime("& rest must be the last parameter"));
                }
                break;
            }
            if name.starts_with('&') {
                return Err(LispError::runtime(format!(
                    "unknown parameter marker '{}'; use &optional or & (rest)",
                    name
                )));
            }
        }

        if in_optional {
            optionals.push(parse_optional(&items[i])?);
        } else {
            required.push(as_symbol(&items[i])?);
        }
        i += 1;
    }

    Ok((required, optionals, rest))
}

/// An `&optional` entry is a bare symbol (default `nil`) or `(name default)`.
fn parse_optional(form: &Value) -> Result<(Symbol, Value), LispError> {
    match form {
        Value::Sym(s) => Ok((*s, Value::Nil)),
        Value::Pair(_) | Value::Vector(_) => {
            let parts = match form {
                Value::Vector(v) => (**v).clone(),
                _ => value::list_to_vec(form)?,
            };
            let name = as_symbol(
                parts.first().ok_or_else(|| LispError::runtime("malformed &optional parameter"))?,
            )?;
            let default = parts.get(1).cloned().unwrap_or(Value::Nil);
            Ok((name, default))
        }
        other => Err(LispError::type_err(format!(
            "malformed &optional parameter: {}",
            crate::printer::print(other)
        ))),
    }
}

/// Attach a name to an anonymous closure when it's `def`'d, for nicer printing
/// and error messages.
fn name_value(val: Value, name: Symbol) -> Value {
    match val {
        Value::Fn(cl) if cl.name.is_none() => value::closure(Closure {
            name: Some(name),
            params: cl.params.clone(),
            optionals: cl.optionals.clone(),
            rest: cl.rest,
            body: cl.body.clone(),
            env: cl.env.clone(),
        }),
        other => other,
    }
}

/// Evaluate all-but-last of `items[from..]` for effect and return the last form
/// (the tail), or `None` if there is nothing to run. Lets callers hand the tail
/// back to the `'tail` loop.
fn tail_of(items: &[Value], from: usize, env: &Rc<Env>) -> Result<Option<Value>, LispError> {
    let slice = if from < items.len() { &items[from..] } else { &[] };
    tail_of_vec(slice, env)
}

fn tail_of_vec(items: &[Value], env: &Rc<Env>) -> Result<Option<Value>, LispError> {
    if items.is_empty() {
        return Ok(None);
    }
    for form in &items[..items.len() - 1] {
        eval(form.clone(), env.clone())?;
    }
    Ok(Some(items[items.len() - 1].clone()))
}

fn as_symbol(v: &Value) -> Result<Symbol, LispError> {
    match v {
        Value::Sym(s) => Ok(*s),
        _ => Err(LispError::type_err(format!("expected a symbol, got {}", crate::printer::print(v)))),
    }
}

fn as_binding_vec(v: &Value) -> Result<Vec<Value>, LispError> {
    // Bindings are code, so a list (a 1 b 2) is the idiomatic form; a vector
    // [a 1 b 2] is also accepted.
    match v {
        Value::Pair(_) => value::list_to_vec(v),
        Value::Vector(items) => Ok((**items).clone()),
        Value::Nil => Ok(Vec::new()),
        _ => Err(LispError::type_err("let bindings must be a list (a 1 b 2) or vector [a 1 b 2]")),
    }
}

fn nth(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

/// True for the `cond` fallback marker, written either as the symbol `else` or
/// the keyword `:else`.
fn is_else_keyword(v: &Value) -> bool {
    match v {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(*s) == "else",
        _ => false,
    }
}
