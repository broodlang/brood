//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/prelude.blsp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::core::heap::Heap;
use crate::core::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Value};
use crate::error::{ErrorKind, LispError, LispResult};
use crate::eval::apply;
use crate::syntax::{printer, reader};

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
    // `mod` is Brood over `rem` (std/prelude.blsp); only `rem` is primitive.
    def(heap, "rem", Arity::exact(2), remainder);
    // `floor` is the single irreducible Float→Int crossing; quot/ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    def(heap, "floor", Arity::exact(1), floor);

    // pair / sequence — `empty?` is Brood (type dispatch over string-length /
    // vector-length / map-keys; std/prelude.blsp). `first`/`rest` ARE the pair
    // accessors (car/cdr), so they stay.
    def(heap, "cons", Arity::exact(2), cons);
    def(heap, "first", Arity::exact(1), first);
    def(heap, "rest", Arity::exact(1), rest);

    // vector
    def(heap, "vector", Arity::any(), vector);
    def(heap, "vector-ref", Arity::exact(2), vector_ref);
    def(heap, "vector-length", Arity::exact(1), vector_length);

    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`map-keys`). `vals`/`contains?` and the `get`/`assoc`/`dissoc`
    // surface (variadic + defaults) are all Brood over these (std/prelude.blsp).
    // Maps are immutable: each op returns a fresh map.
    def(heap, "hash-map", Arity::any(), hash_map);
    def(heap, "map-get", Arity::range(2, 3), map_get);
    def(heap, "map-assoc", Arity::exact(3), map_assoc);
    def(heap, "map-dissoc", Arity::exact(2), map_dissoc);
    def(heap, "map-keys", Arity::exact(1), map_keys);

    // string
    def(heap, "string-length", Arity::exact(1), string_length);
    def(heap, "substring", Arity::exact(3), substring);
    // Case folding (Unicode tables) and parse-or-nil genuinely need Rust; the rest
    // of the string library (split/join/replace/index-of/trim/…) is Brood over
    // these + `substring`/`str` (std/prelude.blsp).
    def(heap, "upper", Arity::exact(1), upper);
    def(heap, "lower", Arity::exact(1), lower);
    def(heap, "string->number", Arity::exact(1), string_to_number);

    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    def(heap, "type-of", Arity::exact(1), type_of);

    // value <-> text and I/O
    def(heap, "str", Arity::any(), str_concat);
    def(heap, "pr-str", Arity::exact(1), pr_str);
    def(heap, "print", Arity::any(), print);
    // `println` is Brood over `print` (std/prelude.blsp).
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
    def(heap, "make-dir", Arity::exact(1), make_dir);
    def(heap, "spit", Arity::exact(2), spit);
    def(heap, "slurp", Arity::exact(1), slurp);

    // system / environment
    def(heap, "getenv", Arity::exact(1), getenv);
    def(heap, "run-process", Arity::exact(2), run_process);

    // macros
    def(heap, "macroexpand-1", Arity::exact(1), macroexpand_1);
    def(heap, "macroexpand", Arity::exact(1), macroexpand);
    def(heap, "gensym", Arity::range(0, 1), gensym);

    // advisory type checker (the Ty lattice's first consumer; see docs/types.md)
    def(heap, "check", Arity::exact(1), check_builtin);

    // source positions (editor tooling; see docs/tooling.md)
    def(heap, "form-pos", Arity::exact(1), form_pos);
    def(heap, "current-file", Arity::exact(0), current_file);

    // introspection (editor tooling; see docs/lsp.md) — derive what we can from
    // the bound value (arglist, doc); enumerate the global table for completion.
    def(heap, "doc", Arity::exact(1), doc);
    def(heap, "arglist", Arity::exact(1), arglist);
    def(heap, "global-names", Arity::exact(0), global_names);
    def(heap, "bound?", Arity::exact(1), bound_p);

    // errors / control
    def(heap, "throw", Arity::exact(1), throw);
    def(heap, "%try", Arity::exact(2), try_catch);
    def(heap, "%isolate", Arity::exact(1), isolate);

    // processes (concurrency)
    def(heap, "spawn", Arity::at_least(1), spawn);
    def(heap, "send", Arity::exact(2), send);
    def(heap, "%receive", Arity::exact(3), receive_match);
    def(heap, "self", Arity::exact(0), self_pid);
    def(heap, "ref", Arity::exact(0), make_ref);
    def(heap, "monitor", Arity::exact(1), monitor);
    def(heap, "demonitor", Arity::exact(1), demonitor);
    def(heap, "spawn-count", Arity::exact(0), spawn_count);
    def(heap, "peak-threads", Arity::exact(0), peak_threads);
    def(heap, "worker-threads", Arity::exact(0), worker_threads);
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).copied().unwrap_or(Value::Nil)
}

/// Destructure exactly two args. The declared `Arity` is the *primary* arity
/// check (enforced once in `eval::call_native` before any builtin runs); this
/// re-check is defense-in-depth for a direct Rust call that bypasses the gate
/// (e.g. a unit test) — it keeps such a call a clean error instead of a panic.
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

/// Require a string, returned **owned** so the `heap` borrow is released before
/// the builtin reads or allocates further (most callers go on to touch
/// `&mut heap`). The string analogue of [`expect_int`]/[`expect_number`].
fn expect_string(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    match v {
        Value::Str(id) => Ok(heap.string(id).to_string()),
        _ => Err(LispError::wrong_type(heap, who, "string", v)),
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
        // Exact integer quotient when it divides evenly; otherwise a float.
        // `checked_*` guards the one overflowing case (`i64::MIN / -1`), which
        // then falls through to the float path instead of panicking.
        (Value::Int(x), Value::Int(y)) => match (x.checked_rem(y), x.checked_div(y)) {
            (Some(0), Some(q)) => Ok(Value::Int(q)),
            _ => Ok(Value::Float(x as f64 / y as f64)),
        },
        _ => Ok(Value::Float(expect_number(heap, "%div", a)? / bf)),
    }
}

fn prim_lt(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%lt")?;
    // Compare two ints directly; coercing to f64 first loses precision past 2^53
    // (e.g. `(< 9007199254740992 9007199254740993)` would wrongly be false).
    let lt = match (a, b) {
        (Value::Int(x), Value::Int(y)) => x < y,
        _ => expect_number(heap, "%lt", a)? < expect_number(heap, "%lt", b)?,
    };
    Ok(Value::Bool(lt))
}

fn prim_eq(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = two(args, "%eq")?;
    Ok(Value::Bool(heap.equal(a, b)))
}

fn int_pair(heap: &Heap, args: &[Value], who: &str) -> Result<(i64, i64), LispError> {
    let (a, b) = two(args, who)?;
    Ok((expect_int(heap, who, a)?, expect_int(heap, who, b)?))
}

fn remainder(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let (a, b) = int_pair(heap, args, "rem")?;
    match a.checked_rem(b) {
        Some(r) => Ok(Value::Int(r)),
        None if b == 0 => Err(LispError::runtime("rem: division by zero")),
        None => Err(LispError::runtime("rem: integer overflow")),
    }
}

/// Floor toward negative infinity, returning an `Int` — the one Float→Int
/// crossing the language can't bootstrap (no other primitive produces an `Int`
/// from a `Float`). An `Int` passes through; a `Float` is floored and cast to
/// `i64` (the cast saturates for out-of-range magnitudes). `ceil`/`round`/`quot`/
/// `pow`/`sqrt` are all Brood over this + `rem`/`/`/`*`/`<` (std/prelude.blsp).
fn floor(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) => Ok(Value::Int(n)),
        v => Ok(Value::Int(expect_number(heap, "floor", v)?.floor() as i64)),
    }
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

// ---------- map ----------

/// Require a map; otherwise a self-identifying type error attributed to `who`.
fn expect_map(heap: &Heap, who: &str, v: Value) -> Result<value::MapId, LispError> {
    match v {
        Value::Map(id) => Ok(id),
        _ => Err(LispError::wrong_type(heap, who, "map", v)),
    }
}

/// `(hash-map k v k v …)` — build a map from alternating key/value args (the
/// programmatic form of the `{ }` literal). Errors on an odd count; last-wins on
/// duplicate keys.
fn hash_map(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    if !args.len().is_multiple_of(2) {
        return Err(LispError::arity(
            "hash-map: expected an even number of arguments (key/value pairs)",
        ));
    }
    let pairs: Vec<(Value, Value)> = args.chunks_exact(2).map(|kv| (kv[0], kv[1])).collect();
    Ok(heap.map_from_pairs(pairs))
}

/// `(map-get m k [default])` — the value `k` maps to, or `default` (nil if
/// omitted) when absent.
fn map_get(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-get", arg(args, 0))?;
    Ok(heap
        .map_get(id, arg(args, 1))
        .unwrap_or_else(|| arg(args, 2)))
}

/// `(map-assoc m k v)` — a fresh map with `k` bound to `v`.
fn map_assoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-assoc", arg(args, 0))?;
    Ok(heap.map_assoc(id, arg(args, 1), arg(args, 2)))
}

/// `(map-dissoc m k)` — a fresh map with `k` removed.
fn map_dissoc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-dissoc", arg(args, 0))?;
    Ok(heap.map_dissoc(id, arg(args, 1)))
}

/// `(map-keys m)` — the keys as a list, in insertion order.
fn map_keys(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-keys", arg(args, 0))?;
    let keys: Vec<Value> = heap.map(id).iter().map(|(k, _)| *k).collect();
    Ok(heap.list(keys))
}

fn string_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Str(id) => Ok(Value::Int(heap.string(id).chars().count() as i64)),
        _ => Err(LispError::wrong_type(heap, "string-length", "string", v)),
    }
}

// ---------- type reflection ----------

/// `(type-of x)` — the runtime type tag of `x` as a keyword: `:int` `:float`
/// `:string` `:symbol` `:keyword` `:bool` `:nil` `:pair` `:vector` `:fn`
/// `:macro` `:native`. The single irreducible reflective primitive: the tag
/// predicates (`int?`/`string?`/…) are Brood wrappers over it (`std/prelude.blsp`),
/// and the in-language type checks build on it too.
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
    Ok(Value::Int(crate::core::alloc::live_bytes() as i64))
}

/// `(mem-peak)` — high-water mark of allocated bytes since the process started.
fn mem_peak(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::core::alloc::peak_bytes() as i64))
}

// ---------- self-hosting ----------

fn eval_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    crate::eval::eval(heap, form, root)
}

fn read_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "read-string", arg(args, 0))?;
    reader::read_one(heap, &s)
}

fn load(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "load", arg(args, 0))?;
    let src = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("load: cannot read {}: {}", path, e)))?;
    // Read positioned so errors point at a line; tag every error with the file
    // (`FILE:LINE:COL:`, see docs/tooling.md).
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let root = heap.env_root(env);
    // Expose the file to Brood (`(current-file)`) for the duration of the load,
    // so the test macros can record each test's source location; restore the
    // previous file afterward since loads nest.
    let prev = heap.set_current_file(Some(path.clone()));
    let mut result = Ok(Value::Nil);
    for (form, pos) in forms {
        result = crate::eval::macros::macroexpand_all(heap, form, root)
            .and_then(|f| crate::eval::eval(heap, f, root))
            .map_err(|e| e.or_pos(pos).or_file(path.clone()));
        if result.is_err() {
            break;
        }
    }
    heap.set_current_file(prev);
    result
}

/// `(eval-string "src")` — read and evaluate every form in a string against the
/// global environment (the string analogue of `load`). The module system uses it
/// to evaluate embedded std modules; it's a general self-hosting hook besides.
fn eval_string(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "eval-string", arg(args, 0))?;
    let root = heap.env_root(env);
    let mut result = Value::Nil;
    for form in reader::read_all(heap, &src)? {
        let form = crate::eval::macros::macroexpand_all(heap, form, root)?;
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
    ("docs", include_str!("../../../std/docs.blsp")),
    ("hatch", include_str!("../../../std/hatch.blsp")),
];

/// `(%builtin-module name)` — the source of a baked-in std module as a string, or
/// nil if there is none. Mechanism only: `require` (Brood) consults this before
/// searching the load-path.
fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let name = match v {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "%builtin-module",
                "module name",
                v,
            ))
        }
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
    let s = expect_string(heap, "substring", arg(args, 0))?;
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

/// `(upper s)` — `s` with every character upper-cased. Case folding is
/// Unicode-aware (e.g. `ß` → `SS`), so it leans on the standard library's tables
/// rather than being expressible in Brood.
fn upper(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "upper", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_uppercase()))
}

/// `(lower s)` — `s` with every character lower-cased (Unicode-aware, like `upper`).
fn lower(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "lower", arg(args, 0))?;
    Ok(heap.alloc_string(&s.to_lowercase()))
}

/// `(string->number s)` — parse `s` as an integer if it is one, else as a float,
/// else `nil`. The inverse of `number->string`. A robust parse-or-nil can't be
/// expressed over `read-string` (which would read `"3abc"` as `3` and stop), so
/// the strict parse is a primitive. Surrounding whitespace is not accepted —
/// `trim` first if the input may carry any.
fn string_to_number(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "string->number", arg(args, 0))?;
    if let Ok(i) = s.parse::<i64>() {
        Ok(Value::Int(i))
    } else if let Ok(f) = s.parse::<f64>() {
        Ok(Value::Float(f))
    } else {
        Ok(Value::Nil)
    }
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
    let path = expect_string(heap, "file-exists?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).exists()))
}

/// `(dir? path)` — true if `path` exists and is a directory.
fn is_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "dir?", arg(args, 0))?;
    Ok(Value::Bool(std::path::Path::new(&path).is_dir()))
}

/// `(list-dir path)` — the entry names (not full paths) directly under a
/// directory, sorted for determinism. Errors if `path` isn't a readable directory.
fn list_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "list-dir", arg(args, 0))?;
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

/// `(make-dir path)` — create `path` and any missing parents (like `mkdir -p`).
/// Returns nil. Used by the project scaffolder (`nest new`).
fn make_dir(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "make-dir", arg(args, 0))?;
    std::fs::create_dir_all(&path)
        .map_err(|e| LispError::runtime(format!("make-dir: {}: {}", path, e)))?;
    Ok(Value::Nil)
}

/// `(spit path content)` — write `content` (a string) to `path`, replacing any
/// existing file. Returns nil. The write-side counterpart to `load`.
fn spit(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let path = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string path", pv)),
    };
    let cv = arg(args, 1);
    let content = match cv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, "spit", "string content", cv)),
    };
    std::fs::write(&path, content)
        .map_err(|e| LispError::runtime(format!("spit: {}: {}", path, e)))?;
    Ok(Value::Nil)
}

/// `(slurp path)` — read the whole file at `path` and return it as a string. The
/// read-side counterpart to `spit`; unlike `load` it does not evaluate, so the
/// doc tooling can inspect a module's source (e.g. its leading docstring form).
fn slurp(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "slurp", arg(args, 0))?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("slurp: {}: {}", path, e)))?;
    Ok(heap.alloc_string(&content))
}

/// `(getenv name)` — the value of environment variable `name` as a string, or nil
/// if it is unset. Lets Brood locate things like the user config directory.
fn getenv(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_string(heap, "getenv", arg(args, 0))?;
    match std::env::var(&name) {
        Ok(val) => Ok(heap.alloc_string(&val)),
        Err(_) => Ok(Value::Nil),
    }
}

/// `(run-process prog args)` — run external program `prog` with `args` (a list or
/// vector of strings), inheriting stdio, and return its exit code as an integer
/// (-1 if killed by a signal). The Emacs `call-process` analogue: the general
/// subprocess mechanism (used by the project scaffolder's `git init`).
fn run_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pv = arg(args, 0);
    let prog = match pv {
        Value::Str(id) => heap.string(id).to_string(),
        _ => {
            return Err(LispError::wrong_type(
                heap,
                "run-process",
                "string program",
                pv,
            ))
        }
    };
    let mut argv = Vec::new();
    for a in heap.seq_items(arg(args, 1))? {
        match a {
            Value::Str(id) => argv.push(heap.string(id).to_string()),
            _ => {
                return Err(LispError::type_err(
                    "run-process: arguments must be strings",
                ))
            }
        }
    }
    match std::process::Command::new(&prog).args(&argv).status() {
        Ok(status) => Ok(Value::Int(status.code().unwrap_or(-1) as i64)),
        Err(e) => Err(LispError::runtime(format!("run-process: {}: {}", prog, e))),
    }
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
    let (expanded, _) = crate::eval::macros::macroexpand_1(heap, arg(args, 0), env)?;
    Ok(expanded)
}

fn macroexpand(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    crate::eval::macros::macroexpand(heap, arg(args, 0), env)
}

/// `(check 'form)` — run the advisory type checker over `form` (macro-expanded
/// first, like the real compile pass) and return a list of warning strings, or
/// `nil` when nothing is provably wrong. Advisory only: it never raises.
fn check_builtin(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let root = heap.env_root(env);
    let form = crate::eval::macros::macroexpand_all(heap, arg(args, 0), root)?;
    let warnings = crate::types::check::check_form(heap, form);
    let mut out = Vec::with_capacity(warnings.len());
    for w in &warnings {
        out.push(heap.alloc_string(w));
    }
    Ok(heap.list(out))
}

// ---------- source positions (editor tooling; see docs/tooling.md) ----------

/// `(form-pos form)` — the `[line col]` (1-based) where `form` was read, or
/// `nil`. Recorded by the reader for list forms; used by the test macros to
/// capture a test's source line *before* the form expands.
fn form_pos(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.form_pos(arg(args, 0)) {
        Some(p) => Ok(heap.alloc_vector(vec![Value::Int(p.line as i64), Value::Int(p.col as i64)])),
        None => Ok(Value::Nil),
    }
}

/// `(current-file)` — the path of the file currently being `load`ed, or `nil`
/// (e.g. at the REPL). Maintained by `load`.
fn current_file(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    match heap.current_file().map(str::to_string) {
        Some(f) => Ok(heap.alloc_string(&f)),
        None => Ok(Value::Nil),
    }
}

// ---------- introspection (editor tooling; see docs/lsp.md) ----------

/// `(doc f)` — the docstring of a function or macro value, or `nil`. A docstring
/// is the leading string literal in a `fn`/`defn` body (stored on the closure
/// when more body follows it). Powers hover / `describe-function`.
fn doc(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let text = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => heap.closure(id).doc.clone(),
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::Nil),
    }
}

/// `(arglist f)` — the parameter list of a function or macro as a list, mirroring
/// the source surface: required names, then `&optional` names, then `& rest`.
/// `nil` for a non-function. Feeds signature help / hover.
fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => id,
        _ => return Ok(Value::Nil),
    };
    // Copy the parts out before re-borrowing the heap mutably to build the list.
    let (params, optionals, rest) = {
        let cl = heap.closure(id);
        (
            cl.params.clone(),
            cl.optionals.iter().map(|&(s, _)| s).collect::<Vec<_>>(),
            cl.rest,
        )
    };
    let mut items: Vec<Value> = params.into_iter().map(Value::Sym).collect();
    if !optionals.is_empty() {
        items.push(value::sym("&optional"));
        items.extend(optionals.into_iter().map(Value::Sym));
    }
    if let Some(r) = rest {
        items.push(value::sym("&"));
        items.push(Value::Sym(r));
    }
    Ok(heap.list(items))
}

/// `(global-names)` — a list of every symbol bound in the global table
/// (prelude + user `def`s), sorted by spelling so the order is deterministic
/// (for completion / workspace-symbol tooling and reproducible doc generation).
fn global_names(_args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let mut syms = heap.global_symbols();
    // `symbol_name` locks the interner and allocates, so resolve each spelling
    // once (cached) rather than twice per comparison.
    syms.sort_by_cached_key(|&s| value::symbol_name(s));
    let syms: Vec<Value> = syms.into_iter().map(Value::Sym).collect();
    Ok(heap.list(syms))
}

/// `(bound? 'name)` — whether `name` is bound in the current scope (which
/// reaches the global table). Takes a symbol, so quote it: `(bound? 'foo)`.
fn bound_p(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Sym(s) => Ok(Value::Bool(heap.env_get(env, s).is_some())),
        other => Err(LispError::wrong_type(heap, "bound?", "symbol", other)),
    }
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

/// `(monitor pid)` — watch `pid`; returns a monitor `ref`. The caller receives
/// `[:down <ref> <pid> <reason>]` when `pid` dies (immediately, reason `:noproc`,
/// if it is already dead).
fn monitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Int(n) if n >= 0 => Ok(crate::process::monitor(n as u64)),
        _ => Err(LispError::type_err(
            "monitor: first argument must be a pid (integer)",
        )),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`.
fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            crate::process::demonitor(n);
            Ok(Value::Nil)
        }
        _ => Err(LispError::type_err(
            "demonitor: argument must be a monitor ref",
        )),
    }
}

/// `(%receive matcher timeout on-timeout)` — the selective-receive primitive the
/// `receive` macro (`std/prelude.blsp`) expands to. See `crate::process::receive_match`.
fn receive_match(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::process::receive_match(heap, arg(args, 0), arg(args, 1), arg(args, 2))
}

fn self_pid(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Int(crate::process::self_pid() as i64))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
fn make_ref(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Ref(crate::process::next_ref()))
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
/// runtime's global bindings: any `def` it makes is rolled back when it
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
