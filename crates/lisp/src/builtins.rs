//! Primitive builtins: the **irreducible kernel** implemented in Rust.
//!
//! Per the core principle (see `CLAUDE.md`), this file is kept as small as
//! possible. Anything that *can* be written in mylisp lives in `std/` instead.
//! What remains here is what the language genuinely cannot bootstrap on its
//! own:
//!
//! - low-level numeric ops on two values (`%add`, `%lt`, ...),
//! - heap constructors/destructors for pairs and vectors,
//! - type-tag predicates (you can't ask "what variant is this?" from mylisp),
//! - I/O and value<->text (`print`, `str`, `read-string`),
//! - the self-hosting hooks (`eval`, `load`, `apply`).
//!
//! The user-facing functions — `+ - * / < > = map filter reduce list ...` — are
//! defined in `std/prelude.lisp` on top of these. Names beginning with `%` are
//! low-level primitives not meant to be called directly.

use std::rc::Rc;

use crate::env::Env;
use crate::error::{LispError, LispResult};
use crate::eval::apply;
use crate::value::{self, NativeFn, NativeFnPtr, Value};
use crate::{printer, reader};

/// Install the primitive kernel into `env` (the root environment).
pub fn register(env: &Rc<Env>) {
    let def = |name: &str, func: NativeFnPtr| {
        env.define(value::intern(name), Value::Native(Rc::new(NativeFn { name: name.to_string(), func })));
    };

    // numeric primitives (always exactly two arguments)
    def("%add", prim_add);
    def("%sub", prim_sub);
    def("%mul", prim_mul);
    def("%div", prim_div);
    def("%lt", prim_lt);
    def("%eq", prim_eq);
    def("mod", modulo);
    def("rem", remainder);

    // pair / sequence primitives
    def("cons", cons);
    def("first", first);
    def("rest", rest);
    def("empty?", is_empty);

    // vector primitives
    def("vector", vector);
    def("vector-ref", vector_ref);
    def("vector-length", vector_length);

    // string primitive
    def("string-length", string_length);

    // type-tag predicates (cannot be expressed in the language itself)
    def("nil?", is_nil);
    def("pair?", is_pair);
    def("int?", is_int);
    def("float?", is_float);
    def("bool?", is_bool);
    def("string?", is_string);
    def("symbol?", is_symbol);
    def("keyword?", is_keyword);
    def("vector?", is_vector);
    def("fn?", is_fn);

    // value <-> text and I/O
    def("str", str_concat);
    def("pr-str", pr_str);
    def("print", print);
    def("println", println);

    // self-hosting hooks
    def("eval", eval_builtin);
    def("read-string", read_string);
    def("load", load);
    def("apply", apply_builtin);

    // macro support
    def("macroexpand-1", macroexpand_1);
    def("macroexpand", macroexpand);
    def("gensym", gensym);
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!("{}: expected 2 arguments, got {}", who, args.len())));
    }
    Ok((args[0].clone(), args[1].clone()))
}

// ---------- numeric primitives ----------

fn as_f64(v: &Value) -> Result<f64, LispError> {
    match v {
        Value::Int(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        _ => Err(LispError::type_err(format!("expected a number, got {}", printer::print(v)))),
    }
}

/// Integer-preserving binary op: int+int stays int (overflow-checked), anything
/// else promotes to float.
fn num_bin(
    args: &[Value],
    who: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let (a, b) = two(args, who)?;
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => int_op(*x, *y)
            .map(Value::Int)
            .ok_or_else(|| LispError::runtime(format!("{}: integer overflow", who))),
        _ => Ok(Value::Float(float_op(as_f64(&a)?, as_f64(&b)?))),
    }
}

fn prim_add(args: &[Value], _: &Rc<Env>) -> LispResult {
    num_bin(args, "%add", i64::checked_add, |a, b| a + b)
}
fn prim_sub(args: &[Value], _: &Rc<Env>) -> LispResult {
    num_bin(args, "%sub", i64::checked_sub, |a, b| a - b)
}
fn prim_mul(args: &[Value], _: &Rc<Env>) -> LispResult {
    num_bin(args, "%mul", i64::checked_mul, |a, b| a * b)
}

fn prim_div(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = two(args, "%div")?;
    let bf = as_f64(&b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero"));
    }
    match (&a, &b) {
        // exact integer division stays an integer
        (Value::Int(x), Value::Int(y)) if x % y == 0 => Ok(Value::Int(x / y)),
        _ => Ok(Value::Float(as_f64(&a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    Ok(Value::Bool(as_f64(&a)? < as_f64(&b)?))
}

fn prim_eq(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = two(args, "%eq")?;
    Ok(Value::Bool(a == b))
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

fn int_pair(args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    let (a, b) = two(args, who)?;
    match (&a, &b) {
        (Value::Int(x), Value::Int(y)) => Ok((*x, *y)),
        _ => Err(LispError::type_err(format!("{}: expected integers", who))),
    }
}

// ---------- pair / sequence primitives ----------

fn cons(args: &[Value], _: &Rc<Env>) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(value::cons(a, b))
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

// ---------- vector primitives ----------

fn vector(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Vector(Rc::new(args.to_vec())))
}

fn vector_ref(args: &[Value], _: &Rc<Env>) -> LispResult {
    match (arg(args, 0), arg(args, 1)) {
        (Value::Vector(items), Value::Int(n)) if n >= 0 && (n as usize) < items.len() => {
            Ok(items[n as usize].clone())
        }
        (Value::Vector(_), Value::Int(_)) => Err(LispError::runtime("vector-ref: index out of range")),
        _ => Err(LispError::type_err("vector-ref: expected a vector and an integer index")),
    }
}

fn vector_length(args: &[Value], _: &Rc<Env>) -> LispResult {
    match arg(args, 0) {
        Value::Vector(items) => Ok(Value::Int(items.len() as i64)),
        other => Err(LispError::type_err(format!("vector-length: not a vector: {}", printer::print(&other)))),
    }
}

fn string_length(args: &[Value], _: &Rc<Env>) -> LispResult {
    match arg(args, 0) {
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        other => Err(LispError::type_err(format!("string-length: not a string: {}", printer::print(&other)))),
    }
}

// ---------- type-tag predicates ----------

fn is_nil(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Nil)))
}
fn is_pair(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Pair(_))))
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
fn is_string(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Str(_))))
}
fn is_symbol(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Sym(_))))
}
fn is_keyword(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Keyword(_))))
}
fn is_vector(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Vector(_))))
}
fn is_fn(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Fn(_) | Value::Native(_))))
}

// ---------- value <-> text and I/O ----------

fn str_concat(args: &[Value], _: &Rc<Env>) -> LispResult {
    let mut s = String::new();
    for a in args {
        s.push_str(&printer::display(a));
    }
    Ok(value::str_val(&s))
}

fn pr_str(args: &[Value], _: &Rc<Env>) -> LispResult {
    Ok(value::str_val(&printer::print(&arg(args, 0))))
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

// ---------- self-hosting hooks ----------

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

fn apply_builtin(args: &[Value], env: &Rc<Env>) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity("apply: expected a function and an argument list"));
    }
    let f = args[0].clone();
    let mut argv = args[1..args.len() - 1].to_vec();
    argv.extend(seq_items(&args[args.len() - 1])?);
    apply(&f, &argv, env)
}

fn seq_items(v: &Value) -> Result<Vec<Value>, LispError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Pair(_) => value::list_to_vec(v),
        Value::Vector(items) => Ok((**items).clone()),
        _ => Err(LispError::type_err(format!("expected a list or vector, got {}", printer::print(v)))),
    }
}

// ---------- macro support ----------

fn macroexpand_1(args: &[Value], env: &Rc<Env>) -> LispResult {
    let (expanded, _) = crate::macros::macroexpand_1(&arg(args, 0), env)?;
    Ok(expanded)
}

fn macroexpand(args: &[Value], env: &Rc<Env>) -> LispResult {
    crate::macros::macroexpand(&arg(args, 0), env)
}

thread_local! {
    static GENSYM_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Produce a fresh, unique symbol — the building block for hygiene-by-convention
/// in macros. Optional argument is a name prefix.
fn gensym(args: &[Value], _: &Rc<Env>) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(s) => s.to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(&other),
    };
    let n = GENSYM_COUNTER.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    Ok(value::sym(&format!("{}__{}", prefix, n)))
}
