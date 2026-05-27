//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in mylisp lives in `std/prelude.lisp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::error::{ErrorKind, LispError, LispResult};
use crate::eval::apply;
use crate::heap::Heap;
use crate::value::{self, EnvId, NativeFn, NativeFnPtr, Value};
use crate::{printer, reader};

/// Install the primitive kernel into `root`.
pub fn register(heap: &mut Heap, root: EnvId) {
    let def = |heap: &mut Heap, name: &str, func: NativeFnPtr| {
        let v = heap.alloc_native(NativeFn { name: name.to_string(), func });
        heap.env_define(root, value::intern(name), v);
    };

    // numeric primitives
    def(heap, "%add", prim_add);
    def(heap, "%sub", prim_sub);
    def(heap, "%mul", prim_mul);
    def(heap, "%div", prim_div);
    def(heap, "%lt", prim_lt);
    def(heap, "%eq", prim_eq);
    def(heap, "mod", modulo);
    def(heap, "rem", remainder);

    // pair / sequence
    def(heap, "cons", cons);
    def(heap, "first", first);
    def(heap, "rest", rest);
    def(heap, "empty?", is_empty);

    // vector
    def(heap, "vector", vector);
    def(heap, "vector-ref", vector_ref);
    def(heap, "vector-length", vector_length);

    // string
    def(heap, "string-length", string_length);

    // type-tag predicates
    def(heap, "nil?", is_nil);
    def(heap, "pair?", is_pair);
    def(heap, "int?", is_int);
    def(heap, "float?", is_float);
    def(heap, "bool?", is_bool);
    def(heap, "string?", is_string);
    def(heap, "symbol?", is_symbol);
    def(heap, "keyword?", is_keyword);
    def(heap, "vector?", is_vector);
    def(heap, "fn?", is_fn);

    // value <-> text and I/O
    def(heap, "str", str_concat);
    def(heap, "pr-str", pr_str);
    def(heap, "print", print);
    def(heap, "println", println);

    // self-hosting
    def(heap, "eval", eval_builtin);
    def(heap, "read-string", read_string);
    def(heap, "load", load);
    def(heap, "require", require);
    def(heap, "apply", apply_builtin);

    // macros
    def(heap, "macroexpand-1", macroexpand_1);
    def(heap, "macroexpand", macroexpand);
    def(heap, "gensym", gensym);

    // errors / control
    def(heap, "throw", throw);
    def(heap, "%try", try_catch);

    // processes (concurrency)
    def(heap, "spawn", spawn);
    def(heap, "send", send);
    def(heap, "receive", receive);
    def(heap, "self", self_pid);
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}

fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!("{}: expected 2 arguments, got {}", who, args.len())));
    }
    Ok((args[0], args[1]))
}

// ---------- numeric ----------

fn as_f64(v: Value) -> Result<f64, LispError> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        _ => Err(LispError::type_err("expected a number")),
    }
}

fn num_bin(
    args: &[Value],
    who: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    float_op: fn(f64, f64) -> f64,
) -> LispResult {
    let (a, b) = two(args, who)?;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => int_op(x, y)
            .map(Value::Int)
            .ok_or_else(|| LispError::runtime(format!("{}: integer overflow", who))),
        _ => Ok(Value::Float(float_op(as_f64(a)?, as_f64(b)?))),
    }
}

fn prim_add(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    num_bin(args, "%add", i64::checked_add, |a, b| a + b)
}
fn prim_sub(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    num_bin(args, "%sub", i64::checked_sub, |a, b| a - b)
}
fn prim_mul(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    num_bin(args, "%mul", i64::checked_mul, |a, b| a * b)
}

fn prim_div(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%div")?;
    let bf = as_f64(b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero"));
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) if x % y == 0 => Ok(Value::Int(x / y)),
        _ => Ok(Value::Float(as_f64(a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    Ok(Value::Bool(as_f64(a)? < as_f64(b)?))
}

fn prim_eq(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%eq")?;
    Ok(Value::Bool(heap.equal(a, b)))
}

fn int_pair(args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    let (a, b) = two(args, who)?;
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Ok((x, y)),
        _ => Err(LispError::type_err(format!("{}: expected integers", who))),
    }
}

fn modulo(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let (a, b) = int_pair(args, "mod")?;
    if b == 0 {
        return Err(LispError::runtime("mod: division by zero"));
    }
    Ok(Value::Int(a.rem_euclid(b)))
}

fn remainder(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let (a, b) = int_pair(args, "rem")?;
    if b == 0 {
        return Err(LispError::runtime("rem: division by zero"));
    }
    Ok(Value::Int(a % b))
}

// ---------- pair / sequence ----------

fn cons(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "cons")?;
    Ok(heap.alloc_pair(a, b))
}

fn first(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::Nil)),
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::type_err("first: not a list")),
    }
}

fn rest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Pair(p) => Ok(heap.cdr(p)),
        Value::Vector(id) => {
            let items: Vec<Value> = heap.vector(id).iter().skip(1).copied().collect();
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::type_err("rest: not a list")),
    }
}

fn is_empty(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let empty = match arg(args, 0) {
        Value::Nil => true,
        Value::Str(id) => heap.string(id).is_empty(),
        Value::Vector(id) => heap.vector(id).is_empty(),
        Value::Pair(_) => false,
        _ => return Err(LispError::type_err("empty?: not a collection")),
    };
    Ok(Value::Bool(empty))
}

// ---------- vector ----------

fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match (arg(args, 0), arg(args, 1)) {
        (Value::Vector(id), Value::Int(n)) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        (Value::Vector(_), Value::Int(_)) => Err(LispError::runtime("vector-ref: index out of range")),
        _ => Err(LispError::type_err("vector-ref: expected a vector and an integer index")),
    }
}

fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Vector(id) => Ok(Value::Int(heap.vector(id).len() as i64)),
        _ => Err(LispError::type_err("vector-length: not a vector")),
    }
}

fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => Ok(Value::Int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::type_err("string-length: not a string")),
    }
}

// ---------- type-tag predicates ----------

fn is_nil(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Nil)))
}
fn is_pair(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Pair(_))))
}
fn is_int(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Int(_))))
}
fn is_float(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Float(_))))
}
fn is_bool(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Bool(_))))
}
fn is_string(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Str(_))))
}
fn is_symbol(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Sym(_))))
}
fn is_keyword(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Keyword(_))))
}
fn is_vector(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Vector(_))))
}
fn is_fn(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Fn(_) | Value::Native(_))))
}

// ---------- value <-> text and I/O ----------

fn str_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut s = String::new();
    for &a in args {
        s.push_str(&printer::display(heap, a));
    }
    Ok(heap.alloc_string(&s))
}

fn pr_str(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = printer::print(heap, arg(args, 0));
    Ok(heap.alloc_string(&s))
}

fn print(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    print!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stdout().flush().ok();
    Ok(Value::Nil)
}

fn println(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Nil)
}

// ---------- self-hosting ----------

fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    crate::eval::eval(heap, arg(args, 0), root)
}

fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Str(id) => {
            let s = heap.string(id).to_string();
            reader::read_one(heap, &s)
        }
        _ => Err(LispError::type_err("read-string: expected a string")),
    }
}

fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::type_err("load: expected a path string")),
    };
    let src = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("load: cannot read {}: {}", path, e)))?;
    let forms = reader::read_all(heap, &src)?;
    let root = heap.env_root(env);
    let mut result = Value::Nil;
    for form in forms {
        result = crate::eval::eval(heap, form, root)?;
    }
    Ok(result)
}

/// Standard-library modules embedded in the binary (like the prelude), loaded
/// on demand by `require` — so they work from any directory, no file paths.
const TEST_LIB: &str = include_str!("../../../std/test.lisp");

/// `(require 'name)` — load an embedded standard-library module into the global
/// environment. Robust to the current directory (the source is baked in).
fn require(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        other => {
            let shown = printer::print(heap, other);
            return Err(LispError::type_err(format!("require: expected a module name, got {}", shown)));
        }
    };
    let src = match name.as_str() {
        "test" => TEST_LIB,
        _ => return Err(LispError::runtime(format!("require: unknown module '{}'", name))),
    };
    let root = heap.env_root(env);
    for form in reader::read_all(heap, src)? {
        crate::eval::eval(heap, form, root)?;
    }
    Ok(Value::Nil)
}

fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity("apply: expected a function and an argument list"));
    }
    let f = args[0];
    let mut argv = args[1..args.len() - 1].to_vec();
    argv.extend(heap.seq_items(args[args.len() - 1])?);
    apply(heap, f, &argv, env)
}

// ---------- macros ----------

fn macroexpand_1(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let (expanded, _) = crate::macros::macroexpand_1(heap, arg(args, 0), env)?;
    Ok(expanded)
}

fn macroexpand(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    crate::macros::macroexpand(heap, arg(args, 0), env)
}

thread_local! {
    static GENSYM_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    let n = GENSYM_COUNTER.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    Ok(value::sym(&format!("{}__{}", prefix, n)))
}

// ---------- errors / control ----------

fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

// ---------- processes ----------

fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if args.is_empty() {
        return Err(LispError::arity("spawn: expected a function and optional arguments"));
    }
    let pid = crate::process::spawn(heap, args[0], &args[1..])?;
    Ok(Value::Int(pid as i64))
}

fn send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::send(heap, arg(args, 0), arg(args, 1))?;
    Ok(Value::Nil)
}

fn receive(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive(heap)
}

fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::self_pid() as i64))
}

fn try_catch(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let handler = arg(args, 1);
    match apply(heap, thunk, &[], env) {
        Ok(value) => Ok(value),
        Err(e) => {
            let caught = match e.payload {
                Some(v) => v,
                None => {
                    let msg = match e.kind {
                        ErrorKind::User => e.message.clone(),
                        _ => e.to_string(),
                    };
                    heap.alloc_string(&msg)
                }
            };
            apply(heap, handler, &[caught], env)
        }
    }
}
