//! The evaluator: a tree-walker with proper tail calls. Now heap-threaded — it
//! takes `&mut Heap` and addresses values by handle / `EnvId`.
//!
//! `macros` (quasiquote + the macroexpand compile pass) lives alongside it here:
//! the two are mutually recursive — the compile pass lowers the `fn`/`let`
//! pattern surfaces the evaluator runs, and the evaluator falls back to it.

pub mod macros;

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::error::{LispError, LispResult};
use crate::core::heap::Heap;
use crate::core::value::{self, Closure, ClosureId, EnvId, NativeId, Symbol, Value};

/// Truthiness: only `nil` and `false` are falsy.
pub fn truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// The special-form keywords, mapped to their canonical `&'static str`. The
/// evaluator dispatches on the head symbol's interned id (a `u32`) via this
/// table, then matches the returned name — so the hot path (every combination)
/// avoids `symbol_name`'s global-interner lock and `String` allocation. A symbol
/// that isn't a special form returns `""`, which falls through to macro/function
/// application.
const SPECIAL_NAMES: &[&str] = &[
    "quote",
    "if",
    "do",
    "def",
    "set!",
    "fn",
    "lambda",
    "quasiquote",
    "defmacro",
    "let",
    "let*",
    "while",
];

static SPECIAL_IDS: LazyLock<HashMap<Symbol, &'static str>> = LazyLock::new(|| {
    SPECIAL_NAMES
        .iter()
        .map(|&n| (value::intern(n), n))
        .collect()
});

#[inline]
fn special_name(s: Symbol) -> &'static str {
    SPECIAL_IDS.get(&s).copied().unwrap_or("")
}

pub fn eval(heap: &mut Heap, expr: Value, env: EnvId) -> LispResult {
    let mut expr = expr;
    let mut env = env;

    'tail: loop {
        match expr {
            Value::Sym(s) => {
                return heap.env_get(env, s).ok_or_else(|| {
                    LispError::unbound(format!("unbound symbol: {}", value::symbol_name(s)))
                });
            }
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(eval(heap, item, env)?);
                }
                return Ok(heap.alloc_vector(out));
            }
            Value::Pair(_) => {} // combination, handled below
            _ => return Ok(expr),
        }

        let (head, rest) = match expr {
            Value::Pair(p) => heap.pair(p),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let Value::Sym(s) = head {
            match special_name(s) {
                "quote" => {
                    let args = heap.list_to_vec(rest)?;
                    return Ok(args.into_iter().next().unwrap_or(Value::Nil));
                }
                "if" => {
                    let args = heap.list_to_vec(rest)?;
                    let test = eval(heap, nth(&args, 0), env)?;
                    expr = if truthy(test) {
                        nth(&args, 1)
                    } else {
                        nth(&args, 2)
                    };
                    continue 'tail;
                }
                "do" => {
                    let args = heap.list_to_vec(rest)?;
                    match tail_of(heap, &args, 0, env)? {
                        Some(last) => {
                            expr = last;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "def" => {
                    let args = heap.list_to_vec(rest)?;
                    let name = as_symbol(
                        args.first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("def: missing name"))?,
                    )?;
                    let val = if args.len() > 1 {
                        eval(heap, args[1], env)?
                    } else {
                        Value::Nil
                    };
                    let val = name_value(heap, val, name);
                    let root = heap.env_root(env);
                    heap.env_define(root, name, val);
                    return Ok(Value::Sym(name));
                }
                "set!" => {
                    let args = heap.list_to_vec(rest)?;
                    let name = as_symbol(
                        args.first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("set!: missing name"))?,
                    )?;
                    let val = eval(heap, nth(&args, 1), env)?;
                    if heap.env_set(env, name, val) {
                        return Ok(val);
                    }
                    return Err(LispError::unbound(format!(
                        "set!: cannot set undefined symbol '{}'",
                        value::symbol_name(name)
                    )));
                }
                "fn" | "lambda" => {
                    // Fallback: a multi-clause / pattern-parameter `fn` normally
                    // lowers to `match*` in the compile pass, but can reach eval
                    // unlowered (built by a quasiquote, or a macro expanded lazily
                    // within its defining form). Lower it here and re-enter. The
                    // common case is detected away cheaply, so this never touches
                    // an ordinary `fn`.
                    if crate::eval::macros::fn_needs_lowering(heap, expr) {
                        expr = crate::eval::macros::macroexpand_all(heap, expr, env)?;
                        continue 'tail;
                    }
                    return make_closure(heap, None, rest, env);
                }
                "quasiquote" => {
                    let args = heap.list_to_vec(rest)?;
                    let template = args.into_iter().next().unwrap_or(Value::Nil);
                    return crate::eval::macros::quasiquote(heap, template, env);
                }
                "defmacro" => {
                    let parts = heap.list_to_vec(rest)?;
                    let name = as_symbol(
                        parts
                            .first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("defmacro: missing name"))?,
                    )?;
                    let fn_rest = heap.list(parts[1..].to_vec());
                    let macro_val = match make_closure(heap, Some(name), fn_rest, env)? {
                        Value::Fn(id) => Value::Macro(id),
                        other => other,
                    };
                    let root = heap.env_root(env);
                    heap.env_define(root, name, macro_val);
                    return Ok(Value::Sym(name));
                }
                "let" | "let*" => {
                    let args = heap.list_to_vec(rest)?;
                    let binds = as_binding_vec(
                        heap,
                        args.first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("let: missing bindings"))?,
                    )?;
                    if binds.len() % 2 != 0 {
                        return Err(LispError::runtime("let: bindings must be name/value pairs"));
                    }
                    // Fallback: a pattern (non-symbol) binding target reached eval
                    // unlowered — same paths as `fn` above. Lower via the compile
                    // pass and re-enter; the common all-symbol `let` skips this.
                    if binds
                        .iter()
                        .step_by(2)
                        .any(|&b| !matches!(b, Value::Sym(_)))
                    {
                        expr = crate::eval::macros::macroexpand_all(heap, expr, env)?;
                        continue 'tail;
                    }
                    let scope = heap.new_env(Some(env));
                    let mut i = 0;
                    while i < binds.len() {
                        let bind_name = as_symbol(binds[i])?;
                        let val = eval(heap, binds[i + 1], scope)?;
                        heap.env_define(scope, bind_name, val);
                        i += 2;
                    }
                    match tail_of(heap, &args, 1, scope)? {
                        Some(last) => {
                            expr = last;
                            env = scope;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "while" => {
                    let args = heap.list_to_vec(rest)?;
                    let test = nth(&args, 0);
                    let body: Vec<Value> = if args.len() > 1 {
                        args[1..].to_vec()
                    } else {
                        Vec::new()
                    };
                    loop {
                        if !truthy(eval(heap, test, env)?) {
                            break;
                        }
                        for &f in &body {
                            eval(heap, f, env)?;
                        }
                    }
                    return Ok(Value::Nil);
                }
                _ => {}
            }
        }

        // --- macro expansion ---
        if let Value::Sym(s) = head {
            if let Some(Value::Macro(mid)) = heap.env_get(env, s) {
                let arg_forms = heap.list_to_vec(rest)?;
                expr = apply_closure(heap, mid, &arg_forms)?;
                continue 'tail;
            }
        }

        // --- function application ---
        let callee = eval(heap, head, env)?;
        let arg_forms = heap.list_to_vec(rest)?;
        let mut argv = Vec::with_capacity(arg_forms.len());
        for form in arg_forms {
            argv.push(eval(heap, form, env)?);
        }
        match callee {
            Value::Native(id) => return call_native(heap, id, &argv, env),
            Value::Fn(id) => {
                let scope = bind_params(heap, id, &argv)?;
                let body_len = heap.closure(id).body.len();
                if body_len == 0 {
                    return Ok(Value::Nil);
                }
                for i in 0..body_len - 1 {
                    let form = heap.closure(id).body[i];
                    eval(heap, form, scope)?;
                }
                expr = heap.closure(id).body[body_len - 1];
                env = scope;
                continue 'tail;
            }
            other => {
                let shown = crate::syntax::printer::print(heap, other);
                return Err(LispError::type_err(format!(
                    "cannot call non-function: {}",
                    shown
                )));
            }
        }
    }
}

pub fn apply(heap: &mut Heap, callee: Value, argv: &[Value], env: EnvId) -> LispResult {
    match callee {
        Value::Native(id) => call_native(heap, id, argv, env),
        Value::Fn(id) => apply_closure(heap, id, argv),
        other => {
            let shown = crate::syntax::printer::print(heap, other);
            Err(LispError::type_err(format!("not a function: {}", shown)))
        }
    }
}

pub fn apply_closure(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> LispResult {
    let scope = bind_params(heap, cl, argv)?;
    let body_len = heap.closure(cl).body.len();
    let mut result = Value::Nil;
    for i in 0..body_len {
        let form = heap.closure(cl).body[i];
        result = eval(heap, form, scope)?;
    }
    Ok(result)
}

fn bind_params(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> Result<EnvId, LispError> {
    // A global-capturing closure (env == None) resolves to this process's global.
    let cl_env = heap.closure(cl).env.unwrap_or_else(|| heap.global());
    let required = heap.closure(cl).params.len();
    let n_opt = heap.closure(cl).optionals.len();
    let has_rest = heap.closure(cl).rest.is_some();
    let max = if has_rest {
        usize::MAX
    } else {
        required + n_opt
    };

    if argv.len() < required || argv.len() > max {
        let who = heap
            .closure(cl)
            .name
            .map(value::symbol_name)
            .unwrap_or_else(|| "fn".to_string());
        let max = if has_rest { None } else { Some(required + n_opt) };
        return Err(LispError::arity(arity_message(
            &who,
            required,
            max,
            argv.len(),
        )));
    }

    let scope = heap.new_env(Some(cl_env));
    for (i, &arg) in argv.iter().enumerate().take(required) {
        let param = heap.closure(cl).params[i];
        heap.env_define(scope, param, arg);
    }
    let mut idx = required;
    for j in 0..n_opt {
        let (name, default_form) = heap.closure(cl).optionals[j];
        if idx < argv.len() {
            heap.env_define(scope, name, argv[idx]);
            idx += 1;
        } else {
            let value = eval(heap, default_form, scope)?;
            heap.env_define(scope, name, value);
        }
    }
    if let Some(rest_sym) = heap.closure(cl).rest {
        let rest_list = heap.list(argv[idx..].to_vec());
        heap.env_define(scope, rest_sym, rest_list);
    }
    Ok(scope)
}

/// Invoke a builtin, enforcing its declared [`Arity`](crate::core::value::Arity) first. The single gate
/// every native call passes through (the evaluator loop *and* `apply`), so a
/// primitive's arity is checked in one place instead of hand-rolled per builtin.
fn call_native(heap: &mut Heap, id: NativeId, argv: &[Value], env: EnvId) -> LispResult {
    let nat = heap.native(id);
    if !nat.arity.accepts(argv.len()) {
        return Err(LispError::arity(arity_message(
            &nat.name,
            nat.arity.min,
            nat.arity.max,
            argv.len(),
        )));
    }
    let func = nat.func;
    func(argv, env, heap)
}

/// Render an arity error — `"{who}: expected {N | N to M | at least N}
/// argument(s), got {got}"`. The one formatter for both builtins (from their
/// declared [`Arity`](crate::core::value::Arity)) and user closures (from their parameter list): a closure
/// with `min..=max` required/optional params passes `Some(max)`; `& rest` (and a
/// variadic builtin) passes `None`.
fn arity_message(who: &str, min: usize, max: Option<usize>, got: usize) -> String {
    let (expected, n) = match max {
        Some(m) if min == m => (min.to_string(), min),
        Some(m) => (format!("{} to {}", min, m), m),
        None => (format!("at least {}", min), min),
    };
    let noun = if n == 1 { "argument" } else { "arguments" };
    format!("{}: expected {} {}, got {}", who, expected, noun, got)
}

fn make_closure(heap: &mut Heap, name: Option<Symbol>, rest: Value, env: EnvId) -> LispResult {
    let parts = heap.list_to_vec(rest)?;
    let param_form = parts
        .first()
        .copied()
        .ok_or_else(|| LispError::runtime("fn: missing parameter list"))?;
    let (params, optionals, rest_param) = parse_params(heap, param_form)?;
    let body = parts[1..].to_vec();
    // A closure defined at the global (parent-less) scope captures the env
    // symbolically (`None`), so it works in any process; otherwise it captures
    // its specific enclosing scope.
    let captured = if heap.is_global(env) { None } else { Some(env) };
    let id = heap.alloc_closure(Closure {
        name,
        params,
        optionals,
        rest: rest_param,
        body,
        env: captured,
    });
    Ok(Value::Fn(id))
}

/// A parsed parameter list: required params, `&optional` params with their
/// default forms, and an optional `&` rest param.
type ParamSpec = (Vec<Symbol>, Vec<(Symbol, Value)>, Option<Symbol>);

fn parse_params(heap: &Heap, form: Value) -> Result<ParamSpec, LispError> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Pair(_) => heap.list_to_vec(form)?,
        Value::Nil => Vec::new(),
        _ => {
            return Err(LispError::type_err(
                "parameter list must be a list (x y) or vector [x y]",
            ))
        }
    };

    let mut required = Vec::new();
    let mut optionals = Vec::new();
    let mut rest = None;
    let mut in_optional = false;
    let mut i = 0;

    while i < items.len() {
        if let Value::Sym(s) = items[i] {
            if value::symbol_is(s, "&optional") {
                if in_optional {
                    return Err(LispError::runtime("&optional may appear only once"));
                }
                in_optional = true;
                i += 1;
                continue;
            }
            if value::symbol_is(s, "&") {
                let r = items.get(i + 1).copied().ok_or_else(|| {
                    LispError::runtime("expected a symbol after & in parameter list")
                })?;
                rest = Some(as_symbol(r)?);
                if i + 2 < items.len() {
                    return Err(LispError::runtime("& rest must be the last parameter"));
                }
                break;
            }
            // A stray `&`-marker (e.g. `&rest`): only now pay for the full name.
            if value::symbol_first_char(s) == Some('&') {
                return Err(LispError::runtime(format!(
                    "unknown parameter marker '{}'; use &optional or & (rest)",
                    value::symbol_name(s)
                )));
            }
        }

        if in_optional {
            optionals.push(parse_optional(heap, items[i])?);
        } else {
            required.push(as_symbol(items[i])?);
        }
        i += 1;
    }

    Ok((required, optionals, rest))
}

fn parse_optional(heap: &Heap, form: Value) -> Result<(Symbol, Value), LispError> {
    match form {
        Value::Sym(s) => Ok((s, Value::Nil)),
        Value::Pair(_) | Value::Vector(_) => {
            let parts = match form {
                Value::Vector(id) => heap.vector(id).to_vec(),
                _ => heap.list_to_vec(form)?,
            };
            let name = as_symbol(
                parts
                    .first()
                    .copied()
                    .ok_or_else(|| LispError::runtime("malformed &optional parameter"))?,
            )?;
            let default = parts.get(1).copied().unwrap_or(Value::Nil);
            Ok((name, default))
        }
        _ => Err(LispError::type_err("malformed &optional parameter")),
    }
}

/// Attach a name to an anonymous closure when it's `def`'d.
fn name_value(heap: &mut Heap, val: Value, name: Symbol) -> Value {
    if let Value::Fn(id) = val {
        if heap.closure(id).name.is_none() {
            let mut c = heap.closure(id).clone();
            c.name = Some(name);
            return Value::Fn(heap.alloc_closure(c));
        }
    }
    val
}

fn as_symbol(v: Value) -> Result<Symbol, LispError> {
    match v {
        Value::Sym(s) => Ok(s),
        _ => Err(LispError::type_err("expected a symbol")),
    }
}

fn as_binding_vec(heap: &Heap, v: Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Pair(_) => heap.list_to_vec(v),
        Value::Vector(id) => Ok(heap.vector(id).to_vec()),
        Value::Nil => Ok(Vec::new()),
        _ => Err(LispError::type_err(
            "let bindings must be a list (a 1 b 2) or vector [a 1 b 2]",
        )),
    }
}

/// Evaluate all-but-last of `items[from..]` for effect; return the tail form.
fn tail_of(
    heap: &mut Heap,
    items: &[Value],
    from: usize,
    env: EnvId,
) -> Result<Option<Value>, LispError> {
    let slice = if from < items.len() {
        &items[from..]
    } else {
        &[][..]
    };
    if slice.is_empty() {
        return Ok(None);
    }
    for &form in &slice[..slice.len() - 1] {
        eval(heap, form, env)?;
    }
    Ok(Some(slice[slice.len() - 1]))
}

fn nth(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}
