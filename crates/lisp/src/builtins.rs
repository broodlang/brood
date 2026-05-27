//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/prelude.blsp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::error::{ErrorKind, LispError, LispResult};
use crate::eval::apply;
use crate::heap::Heap;
use crate::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Value};
use crate::{printer, reader};

/// Install the primitive kernel into `root`.
pub fn register(heap: &mut Heap, root: EnvId) {
    let def = |heap: &mut Heap, name: &str, arity: Arity, func: NativeFnPtr| {
        let v = heap.alloc_native(NativeFn {
            name: name.to_string(),
            arity,
            func,
        });
        heap.env_define(root, value::intern(name), v);
    };

    // numeric primitives
    def(heap, "%add", Arity::exact(2), prim_add);
    def(heap, "%sub", Arity::exact(2), prim_sub);
    def(heap, "%mul", Arity::exact(2), prim_mul);
    def(heap, "%div", Arity::exact(2), prim_div);
    def(heap, "%lt", Arity::exact(2), prim_lt);
    def(heap, "%eq", Arity::exact(2), prim_eq);
    def(heap, "mod", Arity::exact(2), modulo);
    def(heap, "rem", Arity::exact(2), remainder);

    // pair / sequence
    def(heap, "cons", Arity::exact(2), cons);
    def(heap, "first", Arity::exact(1), first);
    def(heap, "rest", Arity::exact(1), rest);
    def(heap, "empty?", Arity::exact(1), is_empty);

    // vector
    def(heap, "vector", Arity::any(), vector);
    def(heap, "vector-ref", Arity::exact(2), vector_ref);
    def(heap, "vector-length", Arity::exact(1), vector_length);

    // string
    def(heap, "string-length", Arity::exact(1), string_length);
    def(heap, "substring", Arity::exact(3), substring);

    // type-tag predicates
    def(heap, "nil?", Arity::exact(1), is_nil);
    def(heap, "pair?", Arity::exact(1), is_pair);
    def(heap, "int?", Arity::exact(1), is_int);
    def(heap, "float?", Arity::exact(1), is_float);
    def(heap, "bool?", Arity::exact(1), is_bool);
    def(heap, "string?", Arity::exact(1), is_string);
    def(heap, "symbol?", Arity::exact(1), is_symbol);
    def(heap, "keyword?", Arity::exact(1), is_keyword);
    def(heap, "vector?", Arity::exact(1), is_vector);
    def(heap, "fn?", Arity::exact(1), is_fn);
    def(heap, "type-of", Arity::exact(1), type_of);

    // value <-> text and I/O
    def(heap, "str", Arity::any(), str_concat);
    def(heap, "pr-str", Arity::exact(1), pr_str);
    def(heap, "print", Arity::any(), print);
    def(heap, "println", Arity::any(), println);
    def(heap, "stdout-tty?", Arity::exact(0), stdout_tty);

    // time
    def(heap, "now", Arity::exact(0), now);

    // memory
    def(heap, "mem-bytes", Arity::exact(0), mem_bytes);
    def(heap, "mem-peak", Arity::exact(0), mem_peak);

    // self-hosting
    def(heap, "eval", Arity::exact(1), eval_builtin);
    def(heap, "read-string", Arity::exact(1), read_string);
    def(heap, "eval-string", Arity::exact(1), eval_string);
    def(heap, "load", Arity::exact(1), load);
    def(heap, "%builtin-module", Arity::exact(1), builtin_module);
    def(heap, "apply", Arity::at_least(2), apply_builtin);

    // symbols
    def(heap, "name", Arity::exact(1), name_of);

    // filesystem — mechanism for the Brood module system + project test runner
    def(heap, "cwd", Arity::exact(0), cwd);
    def(heap, "file-exists?", Arity::exact(1), file_exists);
    def(heap, "dir?", Arity::exact(1), is_dir);
    def(heap, "list-dir", Arity::exact(1), list_dir);

    // macros
    def(heap, "macroexpand-1", Arity::exact(1), macroexpand_1);
    def(heap, "macroexpand", Arity::exact(1), macroexpand);
    def(heap, "gensym", Arity::range(0, 1), gensym);

    // errors / control
    def(heap, "throw", Arity::exact(1), throw);
    def(heap, "%try", Arity::exact(2), try_catch);
    def(heap, "%isolate", Arity::exact(1), isolate);

    // processes (concurrency)
    def(heap, "spawn", Arity::at_least(1), spawn);
    def(heap, "send", Arity::exact(2), send);
    def(heap, "receive", Arity::exact(0), receive);
    def(heap, "self", Arity::exact(0), self_pid);
    def(heap, "spawn-count", Arity::exact(0), spawn_count);
    def(heap, "peak-threads", Arity::exact(0), peak_threads);
    def(heap, "worker-threads", Arity::exact(0), worker_threads);
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}

fn two(args: &[Value], who: &str) -> Result<(Value, Value), LispError> {
    if args.len() != 2 {
        return Err(LispError::arity(format!(
            "{}: expected 2 arguments, got {}",
            who,
            args.len()
        )));
    }
    Ok((args[0], args[1]))
}

// ---------- numeric ----------

/// Require a number, coerced to `f64`; otherwise a self-identifying type error
/// attributed to `who` (the primitive that needed it).
fn expect_number(heap: &Heap, who: &str, v: Value) -> Result<f64, LispError> {
    match v {
        Value::Int(n) => Ok(n as f64),
        Value::Float(f) => Ok(f),
        _ => Err(LispError::wrong_type(heap, who, "number", v)),
    }
}

/// Require an integer; otherwise a self-identifying type error.
fn expect_int(heap: &Heap, who: &str, v: Value) -> Result<i64, LispError> {
    match v {
        Value::Int(n) => Ok(n),
        _ => Err(LispError::wrong_type(heap, who, "int", v)),
    }
}

fn num_bin(
    heap: &Heap,
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
        _ => Ok(Value::Float(float_op(
            expect_number(heap, who, a)?,
            expect_number(heap, who, b)?,
        ))),
    }
}

fn prim_add(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%add", i64::checked_add, |a, b| a + b)
}
fn prim_sub(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%sub", i64::checked_sub, |a, b| a - b)
}
fn prim_mul(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    num_bin(heap, args, "%mul", i64::checked_mul, |a, b| a * b)
}

fn prim_div(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%div")?;
    let bf = expect_number(heap, "%div", b)?;
    if bf == 0.0 {
        return Err(LispError::runtime("division by zero"));
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) if x % y == 0 => Ok(Value::Int(x / y)),
        _ => Ok(Value::Float(expect_number(heap, "%div", a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    Ok(Value::Bool(
        expect_number(heap, "%lt", a)? < expect_number(heap, "%lt", b)?,
    ))
}

fn prim_eq(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%eq")?;
    Ok(Value::Bool(heap.equal(a, b)))
}

fn int_pair(heap: &Heap, args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    let (a, b) = two(args, who)?;
    Ok((expect_int(heap, who, a)?, expect_int(heap, who, b)?))
}

fn modulo(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "mod")?;
    if b == 0 {
        return Err(LispError::runtime("mod: division by zero"));
    }
    Ok(Value::Int(a.rem_euclid(b)))
}

fn remainder(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "rem")?;
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
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.car(p)),
        Value::Vector(id) => Ok(heap.vector(id).first().copied().unwrap_or(Value::Nil)),
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "first", "list or vector", v)),
    }
}

fn rest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Pair(p) => Ok(heap.cdr(p)),
        Value::Vector(id) => {
            let items: Vec<Value> = heap.vector(id).iter().skip(1).copied().collect();
            Ok(heap.list(items))
        }
        Value::Nil => Ok(Value::Nil),
        _ => Err(LispError::wrong_type(heap, "rest", "list or vector", v)),
    }
}

fn is_empty(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let empty = match v {
        Value::Nil => true,
        Value::Str(id) => heap.string(id).is_empty(),
        Value::Vector(id) => heap.vector(id).is_empty(),
        Value::Pair(_) => false,
        _ => return Err(LispError::wrong_type(heap, "empty?", "collection", v)),
    };
    Ok(Value::Bool(empty))
}

// ---------- vector ----------

fn vector(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_vector(args.to_vec()))
}

fn vector_ref(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let n = expect_int(heap, "vector-ref", arg(args, 1))?;
    match v {
        Value::Vector(id) if n >= 0 && (n as usize) < heap.vector(id).len() => {
            Ok(heap.vector(id)[n as usize])
        }
        Value::Vector(_) => Err(LispError::runtime("vector-ref: index out of range")),
        _ => Err(LispError::wrong_type(heap, "vector-ref", "vector", v)),
    }
}

fn vector_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Vector(id) => Ok(Value::Int(heap.vector(id).len() as i64)),
        _ => Err(LispError::wrong_type(heap, "vector-length", "vector", v)),
    }
}

fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::wrong_type(heap, "string-length", "string", v)),
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
    Ok(Value::Bool(matches!(
        arg(args, 0),
        Value::Fn(_) | Value::Native(_)
    )))
}

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The reflective primitive the in-language type checks
/// build on; spellings mirror the `int?`/`string?`/… predicates.
fn type_of(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(value::kw(value::tag(arg(args, 0)).name()))
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

/// `(stdout-tty?)` — true when stdout is an interactive terminal, false when it's
/// captured (a pipe, a file, `cargo test`). The test framework uses this to emit
/// ANSI colour only when a human is watching, so captured output (what an LLM or
/// CI reads) stays clean plain text.
fn stdout_tty(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    use std::io::IsTerminal;
    Ok(Value::Bool(std::io::stdout().is_terminal()))
}

// ---------- time ----------

/// `(now)` — wall-clock milliseconds since the Unix epoch, as an integer.
/// Subtract two readings to measure elapsed time (see `std/test.blsp`).
fn now(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ms))
}

// ---------- memory ----------

/// `(mem-bytes)` — bytes currently allocated across the whole process.
fn mem_bytes(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::alloc::live_bytes() as i64))
}

/// `(mem-peak)` — high-water mark of allocated bytes since the process started.
fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::alloc::peak_bytes() as i64))
}

// ---------- self-hosting ----------

fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}

fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => {
            let s = heap.string(id).to_string();
            reader::read_one(heap, &s)
        }
        _ => Err(LispError::wrong_type(heap, "read-string", "string", v)),
    }
}

fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let path = match v {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "load", "string", v)),
    };
    let src = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("load: cannot read {}: {}", path, e)))?;
    let forms = reader::read_all(heap, &src)?;
    let root = heap.env_root(env);
    let mut result = Value::Nil;
    for form in forms {
        let form = crate::macros::macroexpand_all(heap, form, root)?;
        result = crate::eval::eval(heap, form, root)?;
    }
    Ok(result)
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment (the string analogue of `load`). The module system uses it
/// to evaluate embedded std modules; it's a general self-hosting hook besides.
fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let src = match v {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "eval-string", "string", v)),
    };
    let root = heap.env_root(env);
    let mut result = Value::Nil;
    for form in reader::read_all(heap, &src)? {
        let form = crate::macros::macroexpand_all(heap, form, root)?;
        result = crate::eval::eval(heap, form, root)?;
    }
    Ok(result)
}

/// Standard-library modules baked into the binary (like the prelude), so they load
/// from any directory with no file paths. The require / provide / load-path
/// *policy* is written in Brood (`std/prelude.blsp`, ADR-019); Rust only exposes
/// an embedded module's source here, via `%builtin-module` (ADR-006/008).
const EMBEDDED_MODULES: &[(&str, &str)] = &[
    ("test", include_str!("../../../std/test.blsp")),
    ("project", include_str!("../../../std/project.blsp")),
];

/// `(%builtin-module name)` — the source of a baked-in std module as a string, or
/// nil if there is none. Mechanism only: `require` (Brood) consults this before
/// searching the load-path.
fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let name = match v {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "%builtin-module", "module name", v)),
    };
    match EMBEDDED_MODULES.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::Nil),
    }
}

/// `(name x)` — the spelling of a symbol or keyword as a string (no leading `:`),
/// or the string unchanged. The module system uses it to turn a module name into
/// a filename.
fn name_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(s) | Value::Keyword(s) => Ok(heap.alloc_string(&value::symbol_name(s))),
        Value::Str(_) => Ok(v),
        _ => Err(LispError::wrong_type(
            heap,
            "name",
            "symbol, keyword, or string",
            v,
        )),
    }
}

/// `(substring s start end)` — the characters of `s` in `[start, end)`,
/// char-indexed (consistent with `string-length`). Errors if out of range.
fn substring(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let s = match v {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "substring", "string", v)),
    };
    let start = expect_int(heap, "substring", arg(args, 1))?;
    let end = expect_int(heap, "substring", arg(args, 2))?;
    let len = s.chars().count() as i64;
    if start < 0 || end < start || end > len {
        return Err(LispError::runtime("substring: index out of range"));
    }
    let sub: String = s
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    Ok(heap.alloc_string(&sub))
}

// ---------- filesystem ----------
// Mechanism only: existence / directory reflection so the Brood module system and
// the project test runner can resolve load paths and discover test files. Path
// manipulation and all policy live in Brood (`std/prelude.blsp`, `std/project.blsp`).

/// `(cwd)` — the process's current working directory as a string.
fn cwd(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match std::env::current_dir() {
        Ok(p) => Ok(heap.alloc_string(&p.to_string_lossy())),
        Err(e) => Err(LispError::runtime(format!("cwd: {}", e))),
    }
}

/// `(file-exists? path)` — true if a file or directory exists at `path`.
fn file_exists(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Bool(std::path::Path::new(heap.string(id)).exists())),
        _ => Err(LispError::wrong_type(heap, "file-exists?", "string", v)),
    }
}

/// `(dir? path)` — true if `path` exists and is a directory.
fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Bool(std::path::Path::new(heap.string(id)).is_dir())),
        _ => Err(LispError::wrong_type(heap, "dir?", "string", v)),
    }
}

/// `(list-dir path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let path = match v {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "list-dir", "string", v)),
    };
    let mut names: Vec<String> = match std::fs::read_dir(&path) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(e) => return Err(LispError::runtime(format!("list-dir: {}: {}", path, e))),
    };
    names.sort();
    let mut items = Vec::with_capacity(names.len());
    for n in &names {
        items.push(heap.alloc_string(n));
    }
    Ok(heap.list(items))
}

fn apply_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    if args.len() < 2 {
        return Err(LispError::arity(
            "apply: expected a function and an argument list",
        ));
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

fn gensym(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let prefix = match arg(args, 0) {
        Value::Str(id) => heap.string(id).to_string(),
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Nil => "g".to_string(),
        other => printer::display(heap, other),
    };
    Ok(value::gensym(&prefix))
}

// ---------- errors / control ----------

fn throw(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Err(LispError::thrown(arg(args, 0), heap))
}

// ---------- processes ----------

fn spawn(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if args.is_empty() {
        return Err(LispError::arity(
            "spawn: expected a function and optional arguments",
        ));
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

/// `(spawn-count)` — how many green processes have been spawned since the program
/// started. (Green processes are cheap coroutines, not OS threads — step 4b.)
fn spawn_count(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::spawn_count() as i64))
}

/// `(peak-threads)` — high-water mark of processes running *simultaneously*
/// (bounded by the worker-pool size); how much parallelism was actually reached.
fn peak_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::peak_threads() as i64))
}

/// `(worker-threads)` — size of the scheduler's worker-thread pool that runs the
/// green processes (≈ `nproc`, or the `-j` setting); 0 until the first spawn.
fn worker_threads(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::worker_threads() as i64))
}

/// `(%isolate thunk)` — call `thunk` (no args) with a *private copy* of the
/// runtime's global bindings: any `def`/`set!` it makes is rolled back when it
/// returns, so it cannot affect other code. The test framework wraps each
/// `:isolated` test in this so a test's definitions never leak to another test.
/// Restores the bindings even if the thunk raises (the error then propagates).
///
/// This only isolates *bindings* — the shared code slabs and the symbol interner
/// still grow (memory, not behaviour; there's no GC yet) — and it is sound only
/// with no other process mutating globals concurrently, which the runner ensures
/// by running isolated tests alone.
fn isolate(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let thunk = arg(args, 0);
    let saved = heap.snapshot_globals();
    let result = apply(heap, thunk, &[], env);
    heap.restore_globals(saved);
    result
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
