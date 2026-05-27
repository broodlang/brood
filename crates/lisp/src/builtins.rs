//! Builtins: functions implemented in Rust and installed into the global
//! environment by [`register`]. They receive already-evaluated arguments.
//!
//! Anything that does not need to be a builtin should live in the prelude
//! (`std/prelude.lisp`) instead — the less that is baked into Rust, the more of
//! the language is editable at runtime.

use std::rc::Rc;

use crate::env::Env;
use crate::error::{LispError, LispResult};
use crate::eval::{apply, truthy};
use crate::value::{self, NativeFn, NativeFnPtr, Value};
use crate::{printer, reader};

/// Install all builtins into `env` (expected to be the root environment).
pub fn register(env: &Rc<Env>) {
    let def = |name: &str, func: NativeFnPtr| {
        env.define(value::intern(name), Value::Native(Rc::new(NativeFn { name: name.to_string(), func })));
    };

    // arithmetic
    def("+", add);
    def("-", sub);
    def("*", mul);
    def("/", div);
    def("mod", modulo);
    def("rem", remainder);

    // comparison & logic
    def("=", eq);
    def("not=", neq);
    def("<", lt);
    def("<=", le);
    def(">", gt);
    def(">=", ge);
    def("not", not);

    // lists / sequences
    def("cons", cons);
    def("first", first);
    def("rest", rest);
    def("car", first);
    def("cdr", rest);
    def("list", list);
    def("vector", vector);
    def("append", append);
    def("reverse", reverse);
    def("nth", nth);
    def("count", count);
    def("length", count);
    def("empty?", is_empty);

    // higher order
    def("map", map);
    def("filter", filter);
    def("reduce", reduce);
    def("apply", apply_builtin);

    // predicates
    def("nil?", is_nil);
    def("pair?", is_pair);
    def("list?", is_list);
    def("symbol?", is_symbol);
    def("keyword?", is_keyword);
    def("string?", is_string);
    def("number?", is_number);
    def("int?", is_int);
    def("float?", is_float);
    def("bool?", is_bool);
    def("fn?", is_fn);
    def("vector?", is_vector);

    // strings / io
    def("str", str_concat);
    def("print", print);
    def("println", println);
    def("pr-str", pr_str);

    // metaprogramming / self-hosting
    def("eval", eval_builtin);
    def("read-string", read_string);
    def("load", load);
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

// ---------- numbers ----------

fn as_f64(v: &Value) -> Result<f64, LispError> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(LispError::type_err(format!("expected a number, got {}", printer::print(v)))),
    }
}

fn all_ints(args: &[Value]) -> bool {
    args.iter().all(|a| matches!(a, Value::Int(_)))
}

fn add(args: &[Value], _: &Rc<Env>) -> LispResult {
    if all_ints(args) {
        let mut acc: i64 = 0;
        for a in args {
            if let Value::Int(n) = a {
                acc = acc.checked_add(*n).ok_or_else(|| LispError::runtime("integer overflow"))?;
            }
        }
        Ok(Value::Int(acc))
    } else {
        let mut acc = 0.0;
        for a in args {
            acc += as_f64(a)?;
        }
        Ok(Value::Float(acc))
    }
}

fn sub(args: &[Value], _: &Rc<Env>) -> LispResult {
    if args.is_empty() {
        return Err(LispError::arity("-: expected at least 1 argument"));
    }
    if all_ints(args) {
        let mut iter = args.iter();
        let mut acc = match iter.next().unwrap() {
            Value::Int(n) => *n,
            _ => unreachable!(),
        };
        if args.len() == 1 {
            return Ok(Value::Int(acc.checked_neg().ok_or_else(|| LispError::runtime("integer overflow"))?));
        }
        for a in iter {
            if let Value::Int(n) = a {
                acc = acc.checked_sub(*n).ok_or_else(|| LispError::runtime("integer overflow"))?;
            }
        }
        Ok(Value::Int(acc))
    } else {
        let mut iter = args.iter();
        let mut acc = as_f64(iter.next().unwrap())?;
        if args.len() == 1 {
            return Ok(Value::Float(-acc));
        }
        for a in iter {
            acc -= as_f64(a)?;
        }
        Ok(Value::Float(acc))
    }
}

fn mul(args: &[Value], _: &Rc<Env>) -> LispResult {
    if all_ints(args) {
        let mut acc: i64 = 1;
        for a in args {
            if let Value::Int(n) = a {
                acc = acc.checked_mul(*n).ok_or_else(|| LispError::runtime("integer overflow"))?;
            }
        }
        Ok(Value::Int(acc))
    } else {
        let mut acc = 1.0;
        for a in args {
            acc *= as_f64(a)?;
        }
        Ok(Value::Float(acc))
    }
}

fn div(args: &[Value], _: &Rc<Env>) -> LispResult {
    if args.is_empty() {
        return Err(LispError::arity("/: expected at least 1 argument"));
    }
    let mut iter = args.iter();
    let mut acc = as_f64(iter.next().unwrap())?;
    if args.len() == 1 {
        if acc == 0.0 {
            return Err(LispError::runtime("division by zero"));
        }
        acc = 1.0 / acc;
    } else {
        for a in iter {
            let d = as_f64(a)?;
            if d == 0.0 {
                return Err(LispError::runtime("division by zero"));
            }
            acc /= d;
        }
    }
    // Integer inputs that divide evenly stay integers; otherwise the result is a float.
    if all_ints(args) && acc.fract() == 0.0 {
        Ok(Value::Int(acc as i64))
    } else {
        Ok(Value::Float(acc))
    }
}

fn int_pair(args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!("{}: expected 2 arguments", who)));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok((*a, *b)),
        _ => Err(LispError::type_err(format!("{}: expected integers", who))),
    }
}

fn modulo(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = int_pair(args, "mod")?;
    if b == 0 {
        return Err(LispError::runtime("mod: division by zero"));
    }
    Ok(Value::Int(a.rem_euclid(b)))
}

fn remainder(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = int_pair(args, "rem")?;
    if b == 0 {
        return Err(LispError::runtime("rem: division by zero"));
    }
    Ok(Value::Int(a % b))
}

// ---------- comparison & logic ----------

fn eq(args: &[Value], _: &Rc<Env>) -> LispResult {
    for w in args.windows(2) {
        if w[0] != w[1] {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn neq(args: &[Value], env: &Rc<Env>) -> LispResult {
    match eq(args, env)? {
        Value::Bool(b) => Ok(Value::Bool(!b)),
        _ => unreachable!(),
    }
}

fn cmp_chain(args: &[Value], who: &str, pred: fn(f64, f64) -> bool) -> LispResult {
    for w in args.windows(2) {
        let a = as_f64(&w[0]).map_err(|_| LispError::type_err(format!("{}: expected numbers", who)))?;
        let b = as_f64(&w[1]).map_err(|_| LispError::type_err(format!("{}: expected numbers", who)))?;
        if !pred(a, b) {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn lt(args: &[Value], _: &Rc<Env>) -> LispResult {
    cmp_chain(args, "<", |a, b| a < b)
}
fn le(args: &[Value], _: &Rc<Env>) -> LispResult {
    cmp_chain(args, "<=", |a, b| a <= b)
}
fn gt(args: &[Value], _: &Rc<Env>) -> LispResult {
    cmp_chain(args, ">", |a, b| a > b)
}
fn ge(args: &[Value], _: &Rc<Env>) -> LispResult {
    cmp_chain(args, ">=", |a, b| a >= b)
}

fn not(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(!truthy(&arg(args, 0))))
}

// ---------- lists / sequences ----------

fn seq_items(v: &Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Pair(_) => value::list_to_vec(v),
        Value::Vector(items) => Ok((**items).clone()),
        _ => Err(LispError::type_err(format!("expected a list or vector, got {}", printer::print(v)))),
    }
}

fn cons(args: &[Value], _: &Rc<Env>) -> LispResult {
    if args.len() != 2 {
        return Err(LispError::arity("cons: expected 2 arguments"));
    }
    Ok(value::cons(args[0].clone(), args[1].clone()))
}

fn first(args: &[Value], _: &Rc<Env>) -> LispResult {
    match arg(args, 0) {
        Value::Pair(p) => Ok(p.0.clone()),
        Value::Vector(v) => Ok(v.first().cloned().unwrap_or(Value::Nil)),
        Value::Nil => Ok(Value::Nil),
        other => Err(LispError::type_err(format!("first: not a list: {}", printer::print(&other)))),
    }
}

fn rest(args: &[Value], _: &Rc<Env>) -> LispResult {
    match arg(args, 0) {
        Value::Pair(p) => Ok(p.1.clone()),
        Value::Vector(v) => Ok(value::list(v.iter().skip(1).cloned().collect())),
        Value::Nil => Ok(Value::Nil),
        other => Err(LispError::type_err(format!("rest: not a list: {}", printer::print(&other)))),
    }
}

fn list(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(value::list(args.to_vec()))
}

fn vector(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Vector(Rc::new(args.to_vec())))
}

fn append(args: &[Value], _: &Rc<Env>) -> LispResult {
    let mut out = Vec::new();
    for a in args {
        out.extend(seq_items(a)?);
    }
    Ok(value::list(out))
}

fn reverse(args: &[Value], _: &Rc<Env>) -> LispResult {
    let mut items = seq_items(&arg(args, 0))?;
    items.reverse();
    Ok(value::list(items))
}

fn nth(args: &[Value], _: &Rc<Env>) -> LispResult {
    let coll = arg(args, 0);
    let idx = match arg(args, 1) {
        Value::Int(n) => n,
        _ => return Err(LispError::type_err("nth: index must be an integer")),
    };
    let items = seq_items(&coll)?;
    if idx < 0 || idx as usize >= items.len() {
        Ok(arg(args, 2)) // optional default, nil if absent
    } else {
        Ok(items[idx as usize].clone())
    }
}

fn count(args: &[Value], _: &Rc<Env>) -> LispResult {
    let v = arg(args, 0);
    let n = match &v {
        Value::Nil => 0,
        Value::Str(s) => s.chars().count(),
        Value::Vector(items) => items.len(),
        Value::Pair(_) => value::list_to_vec(&v)?.len(),
        _ => return Err(LispError::type_err(format!("count: cannot count {}", printer::print(&v)))),
    };
    Ok(Value::Int(n as i64))
}

fn is_empty(args: &[Value], _: &Rc<Env>) -> LispResult {
    let v = arg(args, 0);
    let empty = match &v {
        Value::Nil => true,
        Value::Str(s) => s.is_empty(),
        Value::Vector(items) => items.is_empty(),
        Value::Pair(_) => false,
        _ => return Err(LispError::type_err(format!("empty?: not a collection: {}", printer::print(&v)))),
    };
    Ok(Value::Bool(empty))
}

// ---------- higher order ----------

fn map(args: &[Value], env: &Rc<Env>) -> LispResult {
    let f = arg(args, 0);
    let items = seq_items(&arg(args, 1))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(apply(&f, &[item], env)?);
    }
    Ok(value::list(out))
}

fn filter(args: &[Value], env: &Rc<Env>) -> LispResult {
    let f = arg(args, 0);
    let items = seq_items(&arg(args, 1))?;
    let mut out = Vec::new();
    for item in items {
        if truthy(&apply(&f, &[item.clone()], env)?) {
            out.push(item);
        }
    }
    Ok(value::list(out))
}

fn reduce(args: &[Value], env: &Rc<Env>) -> LispResult {
    let f = arg(args, 0);
    match args.len() {
        3 => {
            let mut acc = args[1].clone();
            for item in seq_items(&args[2])? {
                acc = apply(&f, &[acc, item], env)?;
            }
            Ok(acc)
        }
        2 => {
            let items = seq_items(&args[1])?;
            if items.is_empty() {
                return apply(&f, &[], env);
            }
            let mut acc = items[0].clone();
            for item in &items[1..] {
                acc = apply(&f, &[acc, item.clone()], env)?;
            }
            Ok(acc)
        }
        _ => Err(LispError::arity("reduce: expected (reduce f coll) or (reduce f init coll)")),
    }
}

fn apply_builtin(args: &[Value], env: &Rc<Env>) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity("apply: expected a function and an argument list"));
    }
    let f = args[0].clone();
    let mut argv = args[1..args.len() - 1].to_vec();
    argv.extend(seq_items(&args[args.len() - 1])?);
    apply(&f, &argv, env)
}

// ---------- predicates ----------

fn is_nil(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Nil)))
}
fn is_pair(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Pair(_))))
}
fn is_list(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Nil | Value::Pair(_))))
}
fn is_symbol(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Sym(_))))
}
fn is_keyword(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Keyword(_))))
}
fn is_string(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Str(_))))
}
fn is_number(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Int(_) | Value::Float(_))))
}
fn is_int(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Int(_))))
}
fn is_float(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Float(_))))
}
fn is_bool(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Bool(_))))
}
fn is_fn(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Fn(_) | Value::Native(_))))
}
fn is_vector(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Vector(_))))
}

// ---------- strings / io ----------

fn str_concat(args: &[Value], _: &Rc<Env>) -> LispResult {
    let mut s = String::new();
    for a in args {
        s.push_str(&printer::display(a));
    }
    Ok(value::str_val(&s))
}

fn print(args: &[Value], _: &Rc<Env>) -> LispResult {
    let parts: Vec<String> = args.iter().map(printer::display).collect();
    print!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stdout().flush().ok();
    Ok(Value::Nil)
}

fn println(args: &[Value], _: &Rc<Env>) -> LispResult {
    let parts: Vec<String> = args.iter().map(printer::display).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Nil)
}

fn pr_str(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(value::str_val(&printer::print(&arg(args, 0))))
}

// ---------- metaprogramming / self-hosting ----------

fn eval_builtin(args: &[Value], env: &Rc<Env>) -> LispResult {
    crate::eval::eval(arg(args, 0), Env::root(env))
}

fn read_string(args: &[Value], _: &Rc<Env>) -> LispResult {
    match arg(args, 0) {
        Value::Str(s) => reader::read_one(&s),
        other => Err(LispError::type_err(format!("read-string: expected a string, got {}", printer::print(&other)))),
    }
}

fn load(args: &[Value], env: &Rc<Env>) -> LispResult {
    let path = match arg(args, 0) {
        Value::Str(s) => s.to_string(),
        other => return Err(LispError::type_err(format!("load: expected a path string, got {}", printer::print(&other)))),
    };
    let src = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("load: cannot read {}: {}", path, e)))?;
    let forms = reader::read_all(&src)?;
    let root = Env::root(env);
    let mut result = Value::Nil;
    for form in forms {
        result = crate::eval::eval(form, root.clone())?;
    }
    Ok(result)
}
