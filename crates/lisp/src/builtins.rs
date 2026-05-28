//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/prelude.blsp` instead.
//! `%`-prefixed names are low-level primitives not meant to be called directly.
//! The annotated list is in `docs/primitives.md`.

use crate::core::heap::Heap;
use crate::core::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Tag, Value};
use crate::error::{ErrorKind, LispError, LispResult};
use crate::eval::apply;
use crate::syntax::{cst, printer, reader};
use crate::types::{Sig, Ty};

/// Install the primitive kernel into `root`.
pub fn register(heap: &mut Heap, root: EnvId) {
    let def = |heap: &mut Heap, name: &str, arity: Arity, sig: Sig, func: NativeFnPtr| {
        let (params, doc) = primitive_doc(name);
        let v = heap.alloc_native(NativeFn {
            name: name.to_string(),
            arity,
            sig,
            func,
            params,
            doc,
        });
        heap.env_define(root, value::intern(name), v);
    };

    // Lattice shorthands used in the signatures below; see types::Ty for the
    // algebra. NUMBER = int ∪ float, LIST = nil ∪ pair, seq = list ∪ vector
    // (the receivers of first/rest). `callable` = fn ∪ native (a thunk or
    // applicable). `ANY` is the "no useful info" lane — overlaps everything,
    // so the disjointness checker never warns against it.
    let any = Ty::ANY;
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let string = Ty::of(Tag::Str);
    let kw = Ty::of(Tag::Keyword);
    let sym = Ty::of(Tag::Sym);
    let bool_ty = Ty::of(Tag::Bool);
    let nil_ty = Ty::of(Tag::Nil);
    let pair = Ty::of(Tag::Pair);
    let vec_ty = Ty::of(Tag::Vector);
    let map_ty = Ty::of(Tag::Map);
    let pid_ty = Ty::of(Tag::Pid);
    let ref_ty = Ty::of(Tag::Ref);
    let list_ty = Ty::LIST;
    let seq = list_ty.union(vec_ty);
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));

    // numeric primitives — `%add`..`%div` accept and return the wider NUMBER
    // (int + int may overflow into Float; the others always do on a Float arg).
    // `%lt` is comparison → bool; `%eq` accepts anything and returns bool.
    def(heap, "%add", Arity::exact(2), Sig::new(vec![num, num], num), prim_add);
    def(heap, "%sub", Arity::exact(2), Sig::new(vec![num, num], num), prim_sub);
    def(heap, "%mul", Arity::exact(2), Sig::new(vec![num, num], num), prim_mul);
    def(heap, "%div", Arity::exact(2), Sig::new(vec![num, num], num), prim_div);
    def(heap, "%lt", Arity::exact(2), Sig::new(vec![num, num], bool_ty), prim_lt);
    def(heap, "%eq", Arity::exact(2), Sig::new(vec![any, any], bool_ty), prim_eq);
    // `mod` is Brood over `rem` (std/prelude.blsp); only `rem` is primitive.
    def(heap, "rem", Arity::exact(2), Sig::new(vec![int, int], int), remainder);
    // `floor` is the single irreducible Float→Int crossing; quot/ceil/round/pow/
    // sqrt are all Brood over it + rem/`/`/`*`/`<` (std/prelude.blsp).
    def(heap, "floor", Arity::exact(1), Sig::new(vec![num], int), floor);

    // pair / sequence — `empty?` is Brood (type dispatch over string-length /
    // vector-length / map-keys; std/prelude.blsp). `first`/`rest` ARE the pair
    // accessors (car/cdr), so they stay. `rest` always yields a list (a vector's
    // tail is built via `heap.list`), never a vector.
    def(heap, "cons", Arity::exact(2), Sig::new(vec![any, any], pair), cons);
    def(heap, "first", Arity::exact(1), Sig::new(vec![seq], any), first);
    def(heap, "rest", Arity::exact(1), Sig::new(vec![seq], list_ty), rest);

    // vector
    def(heap, "vector", Arity::any(), Sig::variadic(any, vec_ty), vector);
    def(heap, "vector-ref", Arity::exact(2), Sig::new(vec![vec_ty, int], any), vector_ref);
    def(heap, "vector-length", Arity::exact(1), Sig::new(vec![vec_ty], int), vector_length);

    // map — the *minimal* kernel: construct, read, two producers, and one
    // enumerator (`map-pairs` → [k v] vectors). `keys`/`vals`/`contains?`/
    // `reduce-kv` and the `get`/`assoc`/`dissoc` surface (variadic + defaults) are
    // all Brood over these (std/prelude.blsp). Maps are immutable: each op returns
    // a fresh map.
    def(heap, "hash-map", Arity::any(), Sig::variadic(any, map_ty), hash_map);
    def(heap, "map-get", Arity::range(2, 3), Sig::with_rest(vec![map_ty, any], any, any), map_get);
    def(heap, "map-assoc", Arity::exact(3), Sig::new(vec![map_ty, any, any], map_ty), map_assoc);
    def(heap, "map-dissoc", Arity::exact(2), Sig::new(vec![map_ty, any], map_ty), map_dissoc);
    def(heap, "map-pairs", Arity::exact(1), Sig::new(vec![map_ty], list_ty), map_pairs);

    // string
    def(heap, "string-length", Arity::exact(1), Sig::new(vec![string], int), string_length);
    def(heap, "substring", Arity::exact(3), Sig::new(vec![string, int, int], string), substring);
    // Case folding (Unicode tables) and parse-or-nil genuinely need Rust; the rest
    // of the string library (split/join/replace/index-of/trim/…) is Brood over
    // these + `substring`/`str` (std/prelude.blsp).
    def(heap, "upper", Arity::exact(1), Sig::new(vec![string], string), upper);
    def(heap, "lower", Arity::exact(1), Sig::new(vec![string], string), lower);
    // string->number returns int *or* float *or* nil (the parse-failed case).
    def(heap, "string->number", Arity::exact(1), Sig::new(vec![string], num.union(nil_ty)), string_to_number);

    // type reflection — the tag predicates (nil?/int?/string?/…) are Brood
    // (std/prelude.blsp) over this one reflective primitive.
    def(heap, "type-of", Arity::exact(1), Sig::new(vec![any], kw), type_of);

    // value <-> text and I/O
    def(heap, "str", Arity::any(), Sig::variadic(any, string), str_concat);
    def(heap, "pr-str", Arity::exact(1), Sig::new(vec![any], string), pr_str);
    def(heap, "print", Arity::any(), Sig::variadic(any, nil_ty), print);
    def(heap, "eprint", Arity::any(), Sig::variadic(any, nil_ty), eprint);
    // `println` is Brood over `print` (std/prelude.blsp).
    def(heap, "stdout-tty?", Arity::exact(0), Sig::nullary(bool_ty), stdout_tty);

    // time
    def(heap, "now", Arity::exact(0), Sig::nullary(int), now);

    // memory
    def(heap, "mem-bytes", Arity::exact(0), Sig::nullary(int), mem_bytes);
    def(heap, "mem-peak", Arity::exact(0), Sig::nullary(int), mem_peak);

    // self-hosting — eval/load/etc. take and return arbitrary forms / values.
    def(heap, "eval", Arity::exact(1), Sig::new(vec![any], any), eval_builtin);
    def(heap, "read-string", Arity::exact(1), Sig::new(vec![string], any), read_string);
    def(heap, "eval-string", Arity::exact(1), Sig::new(vec![string], any), eval_string);
    // CST parse — mechanism for the in-Brood formatter (std/format.blsp); never
    // fails (malformed input becomes [:error "..."] nodes). Returns nested
    // vectors; see `parse_source` for the shape.
    def(heap, "parse-source", Arity::exact(1), Sig::new(vec![string], vec_ty), parse_source);
    def(heap, "load", Arity::exact(1), Sig::new(vec![string], any), load);
    def(heap, "%builtin-module", Arity::exact(1), Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)), builtin_module);
    def(heap, "%builtin-doc", Arity::exact(1), Sig::new(vec![sym.union(kw).union(string)], string.union(nil_ty)), builtin_doc);
    // `apply`'s last positional arg should be a sequence (it's spliced); the
    // others can be anything. We model it as `(callable, ...any) -> any` — the
    // sequence-at-tail constraint is dynamic-only (a poor fit for fixed-arity
    // sigs).
    def(heap, "apply", Arity::at_least(2), Sig::with_rest(vec![callable], any, any), apply_builtin);

    // symbols
    def(heap, "name", Arity::exact(1), Sig::new(vec![sym.union(kw).union(string)], string), name_of);
    def(heap, "symbol", Arity::exact(1), Sig::new(vec![string.union(sym).union(kw)], sym), to_symbol);
    def(heap, "keyword", Arity::exact(1), Sig::new(vec![string.union(sym).union(kw)], kw), to_keyword);

    // filesystem — mechanism for the Brood module system + project test runner
    def(heap, "cwd", Arity::exact(0), Sig::nullary(string), cwd);
    def(heap, "file-exists?", Arity::exact(1), Sig::new(vec![string], bool_ty), file_exists);
    def(heap, "dir?", Arity::exact(1), Sig::new(vec![string], bool_ty), is_dir);
    def(heap, "list-dir", Arity::exact(1), Sig::new(vec![string], list_ty), list_dir);
    def(heap, "make-dir", Arity::exact(1), Sig::new(vec![string], nil_ty), make_dir);
    def(heap, "spit", Arity::exact(2), Sig::new(vec![string, string], nil_ty), spit);
    def(heap, "slurp", Arity::exact(1), Sig::new(vec![string], string), slurp);

    // system / environment
    def(heap, "getenv", Arity::exact(1), Sig::new(vec![string], string.union(nil_ty)), getenv);
    def(heap, "run-process", Arity::exact(2), Sig::new(vec![string, seq], int), run_process);

    // macros
    def(heap, "macroexpand-1", Arity::exact(1), Sig::new(vec![any], any), macroexpand_1);
    def(heap, "macroexpand", Arity::exact(1), Sig::new(vec![any], any), macroexpand);
    def(heap, "gensym", Arity::range(0, 1), Sig::new(vec![string], sym), gensym);

    // advisory type checker (the Ty lattice's first consumer; see docs/types.md)
    def(heap, "check", Arity::exact(1), Sig::new(vec![any], list_ty), check_builtin);
    def(heap, "check-file", Arity::exact(1), Sig::new(vec![string], list_ty), check_file_builtin);

    // source positions (editor tooling; see docs/tooling.md)
    def(heap, "form-pos", Arity::exact(1), Sig::new(vec![any], vec_ty.union(nil_ty)), form_pos);
    def(heap, "current-file", Arity::exact(0), Sig::nullary(string.union(nil_ty)), current_file);
    def(heap, "source-location", Arity::exact(1), Sig::new(vec![sym], vec_ty.union(nil_ty)), source_location);

    // introspection (editor tooling; see docs/lsp.md) — derive what we can from
    // the bound value (arglist, doc); enumerate the global table for completion.
    def(heap, "doc", Arity::exact(1), Sig::new(vec![any], string.union(nil_ty)), doc);
    def(heap, "arglist", Arity::exact(1), Sig::new(vec![any], list_ty), arglist);
    def(heap, "global-names", Arity::exact(0), Sig::nullary(list_ty), global_names);
    def(heap, "bound?", Arity::exact(1), Sig::new(vec![sym], bool_ty), bound_p);

    // errors / control
    def(heap, "throw", Arity::exact(1), Sig::new(vec![any], Ty::NEVER), throw);
    def(heap, "%try", Arity::exact(2), Sig::new(vec![callable, callable], any), try_catch);
    def(heap, "%isolate", Arity::exact(1), Sig::new(vec![callable], any), isolate);

    // dynamic variables (the `defdyn`/`binding` surface is Brood — see prelude)
    def(heap, "%declare-dynamic", Arity::exact(1), Sig::new(vec![sym], nil_ty), declare_dynamic);
    // `%binding`'s first arg is the *list/vector of names*, second is the
    // *list/vector of values*, third is the thunk — the macro `binding` emits
    // these as `(quote (*a* *b* …))` + `[v1 v2 …]` + `(fn () …)`.
    def(heap, "%binding", Arity::exact(3), Sig::new(vec![seq, seq, callable], any), binding);
    def(heap, "dynamic?", Arity::exact(1), Sig::new(vec![any], bool_ty), dynamic_p);

    // processes (concurrency)
    def(heap, "%spawn", Arity::exact(1), Sig::new(vec![callable], pid_ty), spawn);
    // `send`'s target is a pid OR a `{:name :node}` address map.
    def(heap, "send", Arity::exact(2), Sig::new(vec![pid_ty.union(map_ty), any], nil_ty), send);
    // Arg shape: (matcher: callable, timeout: int|nil, on-timeout: callable|nil).
    // The `receive` macro in `std/prelude.blsp` expands to exactly this; the
    // `callable|nil` on the third position is for the no-`after`-clause case
    // (the macro passes `nil`).
    def(heap, "%receive", Arity::exact(3), Sig::new(vec![callable, int.union(nil_ty), callable.union(nil_ty)], any), receive_match);
    def(heap, "self", Arity::exact(0), Sig::nullary(pid_ty), self_pid);
    def(heap, "ref", Arity::exact(0), Sig::nullary(ref_ty), make_ref);
    // `monitor` also accepts a name map (forwarded to the remote node).
    def(heap, "monitor", Arity::exact(1), Sig::new(vec![pid_ty.union(map_ty)], ref_ty), monitor);
    def(heap, "demonitor", Arity::exact(1), Sig::new(vec![ref_ty], nil_ty), demonitor);
    def(heap, "spawn-count", Arity::exact(0), Sig::nullary(int), spawn_count);
    def(heap, "peak-threads", Arity::exact(0), Sig::nullary(int), peak_threads);
    def(heap, "worker-threads", Arity::exact(0), Sig::nullary(int), worker_threads);

    // distributed nodes (connect two runtimes over TCP — crate::dist)
    def(heap, "node-start", Arity::exact(3), Sig::new(vec![sym, string, string], sym), node_start);
    def(heap, "connect", Arity::exact(1), Sig::new(vec![string], sym), connect);
    def(heap, "register", Arity::exact(2), Sig::new(vec![sym, pid_ty], pid_ty), register_name);
    def(heap, "whereis", Arity::exact(1), Sig::new(vec![sym], pid_ty.union(nil_ty)), whereis_name);
    // `node-name` is the keyword `:nonode` until `node-start` sets it to a symbol.
    def(heap, "node-name", Arity::exact(0), Sig::nullary(sym.union(kw)), node_name);
    def(heap, "nodes", Arity::exact(0), Sig::nullary(list_ty), nodes);
    def(heap, "monitor-node", Arity::exact(1), Sig::new(vec![sym], ref_ty), monitor_node);
}

/// Docstrings + parameter names for the public primitives, so `(doc 'name)`,
/// `(arglist 'name)`, and LSP hover treat a Rust builtin like a Brood `defn`
/// (which can't apply here — primitives have no source body). One row per
/// user-facing primitive; mirrors the "Purpose" column of `docs/primitives.md`.
/// `&` in the params marks a rest (variadic) tail. Internal `%`-prefixed
/// primitives are intentionally absent (they aren't meant to be called directly).
#[rustfmt::skip]
static PRIMITIVE_DOCS: &[(&str, &[&str], &str)] = &[
    ("rem", &["a", "b"], "Integer remainder of a / b (truncated, taking the sign of the dividend)."),
    ("floor", &["x"], "Round x toward negative infinity to an integer."),
    ("cons", &["x", "xs"], "A new pair with head x and tail xs."),
    ("first", &["coll"], "The head of a list or vector, or nil if empty."),
    ("rest", &["coll"], "All but the head of a list or vector."),
    ("vector", &["&", "items"], "A vector of the given items."),
    ("vector-ref", &["v", "i"], "The element at index i of vector v."),
    ("vector-length", &["v"], "The number of elements in vector v."),
    ("hash-map", &["&", "kvs"], "A map from alternating key/value arguments (last wins on duplicate keys)."),
    ("map-get", &["m", "k", "default"], "The value at key k in map m, or default (else nil)."),
    ("map-assoc", &["m", "k", "v"], "A fresh map like m with key k set to v."),
    ("map-dissoc", &["m", "k"], "A fresh map like m with key k removed."),
    ("map-pairs", &["m"], "The entries of m as a list of [k v] vectors, in insertion order."),
    ("string-length", &["s"], "The number of characters in string s."),
    ("substring", &["s", "start", "end"], "The characters of s in the range [start, end), char-indexed."),
    ("upper", &["s"], "s upper-cased (Unicode-aware)."),
    ("lower", &["s"], "s lower-cased (Unicode-aware)."),
    ("string->number", &["s"], "Parse s strictly as an int, else a float, else nil (unlike read-string)."),
    ("type-of", &["x"], "The runtime type of x as a keyword (:int, :string, :pair, ...)."),
    ("check", &["form"], "Advisory type-check a quoted form: a list of warning strings, or nil. Never raises."),
    ("check-file", &["path"], "Advisory type-check every top-level form in the file at path: a list of `path:line:col: warning: …` strings, or nil. Does not evaluate the file."),
    ("str", &["&", "xs"], "Concatenate the display forms of the arguments into one string."),
    ("pr-str", &["x"], "The readable (re-readable) text form of x."),
    ("print", &["&", "xs"], "Write the display forms of the arguments to stdout; returns nil."),
    ("eprint", &["&", "xs"], "Write the display forms of the arguments to stderr; returns nil."),
    ("stdout-tty?", &[], "True when stdout is an interactive terminal (false when piped or captured)."),
    ("now", &[], "Wall-clock milliseconds since the Unix epoch."),
    ("mem-bytes", &[], "Bytes currently allocated process-wide."),
    ("mem-peak", &[], "High-water mark of allocated bytes since process start."),
    ("eval", &["form"], "Evaluate a form in the global environment."),
    ("read-string", &["s"], "Parse and return the first form in string s."),
    ("parse-source", &["s"], "Parse s into a lossless CST tree as nested vectors (mechanism for std/format.blsp)."),
    ("eval-string", &["s"], "Read and evaluate every form in string s (the string analogue of load)."),
    ("load", &["path"], "Read and evaluate every form in the file at path."),
    ("apply", &["f", "&", "args"], "Call f with the leading args plus the final list argument spliced in as trailing args."),
    ("name", &["x"], "The spelling of a symbol or keyword as a string (no leading colon)."),
    ("symbol", &["x"], "Coerce a string, symbol, or keyword to the matching symbol (interning if needed)."),
    ("keyword", &["x"], "Coerce a string, symbol, or keyword to the matching keyword (interning if needed)."),
    ("cwd", &[], "The current working directory."),
    ("file-exists?", &["path"], "Whether path exists."),
    ("dir?", &["path"], "Whether path is a directory."),
    ("list-dir", &["path"], "The entry names directly under directory path, sorted."),
    ("make-dir", &["path"], "Create a directory and any missing parents (like mkdir -p)."),
    ("spit", &["path", "s"], "Write string s to the file at path."),
    ("slurp", &["path"], "Read the whole file at path into a string (does not evaluate it)."),
    ("getenv", &["name"], "The value of environment variable name, or nil if unset."),
    ("run-process", &["prog", "args"], "Run external program prog with an args list, inheriting stdio; returns its exit code."),
    ("macroexpand-1", &["form"], "Expand form by a single macro step."),
    ("macroexpand", &["form"], "Fully expand the macros in form."),
    ("gensym", &["prefix"], "A fresh, unique symbol, with an optional name prefix."),
    ("form-pos", &["form"], "A form's [line col] source position, or nil."),
    ("current-file", &[], "The path of the file currently being loaded, or nil."),
    ("source-location", &["name"], "Where global name was defined, as [file line col], or nil. Quote it: (source-location 'foo)."),
    ("doc", &["f"], "The docstring of a function, macro, or primitive, or nil."),
    ("arglist", &["f"], "The parameter list of a function, macro, or primitive, or nil."),
    ("global-names", &[], "Every globally bound symbol, sorted by spelling."),
    ("bound?", &["sym"], "Whether sym is bound in scope. Quote it: (bound? 'foo)."),
    ("dynamic?", &["x"], "Whether x is a symbol declared dynamic with defdyn. Quote it: (dynamic? '*foo*)."),
    ("throw", &["x"], "Raise x as an error - a non-local exit caught by try/catch."),
    ("%spawn", &["thunk"], "Run thunk (a 0-arg fn) in a new green process; returns its pid. Use the `spawn` macro."),
    ("send", &["target", "msg"], "Copy msg into target's mailbox; target is a pid or {:name :node} address. Routes locally or over a node link. Returns nil."),
    ("self", &[], "This process's own pid (carries this node's identity)."),
    ("ref", &[], "A fresh, globally-unique reference token (tags a request to its reply)."),
    ("monitor", &["pid"], "Watch pid; returns a monitor ref. Delivers [:down ref pid reason] when pid dies."),
    ("demonitor", &["mref"], "Drop the monitor identified by mref (best-effort)."),
    ("spawn-count", &[], "How many green processes have been spawned since program start."),
    ("peak-threads", &[], "High-water mark of OS threads running processes concurrently."),
    ("worker-threads", &[], "The size of the scheduler's worker-thread pool (about nproc)."),
    ("node-start", &["name", "addr", "cookie"], "Name this runtime and listen for peers on addr (\"host:port\"); cookie authenticates links. Returns the node name."),
    ("connect", &["spec"], "Link to a peer node named in spec (\"name@host:port\"); cookie-authenticated. Returns the peer's node name."),
    ("register", &["name", "pid"], "Bind a local name so peers can address this process via {:name name :node this-node}. Returns the pid."),
    ("whereis", &["name"], "The local pid registered under `name`, or nil. Strictly local — does not query other nodes."),
    ("node-name", &[], "This runtime's node name (:nonode until node-start)."),
    ("nodes", &[], "A list of currently connected peer node names."),
    ("monitor-node", &["name"], "Get [:nodedown name] when the link to node `name` goes down (heartbeat timeout or close)."),
];

/// The `(params, doc)` for a primitive `name`, or `(&[], "")` if undocumented.
fn primitive_doc(name: &str) -> (&'static [&'static str], &'static str) {
    PRIMITIVE_DOCS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|&(_, p, d)| (p, d))
        .unwrap_or((&[], ""))
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

/// Require a symbol; otherwise a self-identifying type error.
fn expect_symbol(heap: &Heap, who: &str, v: Value) -> Result<value::Symbol, LispError> {
    match v {
        Value::Sym(s) => Ok(s),
        _ => Err(LispError::wrong_type(heap, who, "symbol", v)),
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

/// `(map-pairs m)` — the entries as a list of `[k v]` vectors, in insertion
/// order, in one O(n) pass. The *single* map enumerator: `keys`/`vals`/
/// `contains?`/`reduce-kv` are all Brood over it (std/prelude.blsp).
fn map_pairs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_map(heap, "map-pairs", arg(args, 0))?;
    let entries = heap.map(id).to_vec(); // copy out, releasing the borrow before we alloc
    let pairs: Vec<Value> = entries
        .into_iter()
        .map(|(k, v)| heap.alloc_vector(vec![k, v]))
        .collect();
    Ok(heap.list(pairs))
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

fn eprint(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let parts: Vec<String> = args.iter().map(|&a| printer::display(heap, a)).collect();
    eprint!("{}", parts.join(" "));
    use std::io::Write;
    std::io::stderr().flush().ok();
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

/// `(parse-source s)` — parse s into a lossless CST tree as nested vectors, the
/// mechanism behind `std/format.blsp`. Never raises: malformed input becomes
/// `[:error "raw"]` nodes (parsing resumes after them). See `syntax::cst`.
///
/// Shape (each node is a vector `[kind …]`):
/// - Leaves carry the original source text:
///   `[:symbol "foo"]`, `[:keyword ":foo"]`, `[:int "42"]`, `[:float "1.5"]`,
///   `[:bool "true"]`, `[:nil "nil"]`, `[:str "\"hi\""]` (raw — quotes/escapes
///   included), `[:whitespace "  \n"]`, `[:comment ";; hi\n"]`, `[:error "raw"]`.
/// - Reader macros wrap a single child form:
///   `[:quote child]`, `[:quasi child]`, `[:unquote child]`, `[:splice child]`.
/// - Containers carry a child vector:
///   `[:root [child …]]`, `[:list [child …]]`, `[:vector [child …]]`,
///   `[:map [child …]]`.
///
/// Roundtrip property: concatenating every leaf's text in tree order reproduces
/// the input — this is what makes the CST a faithful basis for formatting.
fn parse_source(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "parse-source", arg(args, 0))?;
    let root = cst::parse(&s);
    Ok(cst_to_value(heap, &root, &s))
}

fn cst_to_value(heap: &mut Heap, node: &cst::Node, src: &str) -> Value {
    use cst::NodeKind::*;
    let tag = |k: &'static str| Value::Keyword(value::intern(k));
    match node.kind {
        // Leaves: [kind raw-text].
        Symbol | Keyword | Int | Float | Str | Bool | Nil | Whitespace | Comment | Error => {
            let k = match node.kind {
                Symbol => "symbol",
                Keyword => "keyword",
                Int => "int",
                Float => "float",
                Str => "str",
                Bool => "bool",
                Nil => "nil",
                Whitespace => "whitespace",
                Comment => "comment",
                Error => "error",
                _ => unreachable!(),
            };
            let text = heap.alloc_string(node.text(src));
            heap.alloc_vector(vec![tag(k), text])
        }
        // Reader-macro wrappers: [kind child]. The single structural child is
        // the wrapped form; any leading whitespace child is dropped (the wrapper
        // owns its position via its parent's children list).
        Quote | Quasi | Unquote | Splice => {
            let k = match node.kind {
                Quote => "quote",
                Quasi => "quasi",
                Unquote => "unquote",
                Splice => "splice",
                _ => unreachable!(),
            };
            // A reader-macro node's children are the wrapped form's parse
            // result(s) — usually a single form. Walk and pick the first
            // non-trivia child; nest the rest as following siblings would be a
            // parse bug, but in case of empty (EOF after ~/`/'/), emit nil.
            let child = node
                .forms()
                .next()
                .map(|c| cst_to_value(heap, c, src))
                .unwrap_or(Value::Nil);
            heap.alloc_vector(vec![tag(k), child])
        }
        // Containers: [kind [child …]]. Children include trivia (whitespace +
        // comments) so the formatter can preserve blank-line + comment intent.
        Root | List | Vector | Map => {
            let k = match node.kind {
                Root => "root",
                List => "list",
                Vector => "vector",
                Map => "map",
                _ => unreachable!(),
            };
            let kids: Vec<Value> = node
                .children
                .iter()
                .map(|c| cst_to_value(heap, c, src))
                .collect();
            let kids_vec = heap.alloc_vector(kids);
            heap.alloc_vector(vec![tag(k), kids_vec])
        }
    }
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
        // Record def sites before expansion (ADR-031): `defn`/`defmacro` are still
        // recognisable here, and their spans aren't yet lost to macroexpansion.
        heap.note_definition(form, pos);
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
    ("format", include_str!("../../../std/format.blsp")),
];

/// Baked-in reference *documents* (markdown), the counterpart to
/// [`EMBEDDED_MODULES`] for non-module text. `(%builtin-doc 'brood-for-claude)`
/// returns the language guide that `nest new` scaffolds into each new project,
/// so a freshly-scaffolded project is self-contained without depending on a
/// Brood install path.
const EMBEDDED_DOCS: &[(&str, &str)] = &[(
    "brood-for-claude",
    include_str!("../../../docs/brood-for-claude.md"),
)];

/// The lookup body shared by `%builtin-module` and `%builtin-doc`: coerce the
/// (symbol | keyword | string) name argument, find it in `table`, return the
/// baked-in source as a fresh string (or `nil` if absent). `who`/`label` are
/// used only in the type-error message.
fn lookup_embedded(
    args: &[Value],
    heap: &mut Heap,
    table: &[(&str, &str)],
    who: &'static str,
    label: &'static str,
) -> LispResult {
    let v = arg(args, 0);
    let name = match v {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Str(id) => heap.string(id).to_string(),
        _ => return Err(LispError::wrong_type(heap, who, label, v)),
    };
    match table.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok(heap.alloc_string(src)),
        None => Ok(Value::Nil),
    }
}

/// `(%builtin-module name)` — the source of a baked-in std module as a string,
/// or nil if there is none. Mechanism only: `require` (Brood) consults this
/// before searching the load-path.
fn builtin_module(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_MODULES, "%builtin-module", "module name")
}

/// `(%builtin-doc name)` — the source of a baked-in reference document as a
/// string, or nil if there is none. Used by `nest new` to scaffold the language
/// guide into each new project.
fn builtin_doc(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    lookup_embedded(args, heap, EMBEDDED_DOCS, "%builtin-doc", "doc name")
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

/// `(symbol x)` — the symbol whose spelling is `x`. Accepts a string (intern as
/// a fresh-or-existing symbol), a symbol (identity), or a keyword (same spelling,
/// retagged as a symbol). The lenient inverse of `name`; pairs with `keyword`.
fn to_symbol(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Sym(_) => Ok(v),
        Value::Keyword(s) => Ok(Value::Sym(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Sym(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "symbol",
            "string, symbol, or keyword",
            v,
        )),
    }
}

/// `(keyword x)` — the keyword whose spelling is `x`. Accepts a string (intern),
/// a keyword (identity), or a symbol (same spelling, retagged as a keyword).
/// Mirrors `symbol`; the two share an interner so a keyword and a symbol with the
/// same spelling carry equal `Symbol` ids (the tag is the only distinction).
fn to_keyword(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    match v {
        Value::Keyword(_) => Ok(v),
        Value::Sym(s) => Ok(Value::Keyword(s)),
        Value::Str(id) => {
            let name = heap.string(id).to_string();
            Ok(Value::Keyword(value::intern(&name)))
        }
        _ => Err(LispError::wrong_type(
            heap,
            "keyword",
            "string, symbol, or keyword",
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

/// `(check-file path)` — run the advisory type checker over every top-level
/// form in the file at `path` and return a list of pre-formatted warning
/// strings (each `"path:line:col: warning: message"`), or `nil` if clean.
///
/// Reads but does **not** evaluate the file — same `check_file` walk the
/// `brood --check` CLI uses, with the file-globals accumulator threaded
/// across top-level forms. The whole-file-at-once shape is what lets `(defn
/// foo …)` at line 1 silence the unbound check on `(foo …)` at line 100. Used
/// by `(check-project)` in `std/project.blsp` for the `nest test` / `nest run`
/// pre-flight.
fn check_file_builtin(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "check-file", arg(args, 0))?;
    let src = std::fs::read_to_string(&path)
        .map_err(|e| LispError::runtime(format!("check-file: cannot read {}: {}", path, e)))?;
    let forms = reader::read_all_positioned(heap, &src).map_err(|e| e.or_file(path.clone()))?;
    let just_forms: Vec<Value> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = crate::types::check::check_file(heap, &just_forms);
    let mut out = Vec::with_capacity(warnings.len());
    for (pos, msg) in &warnings {
        let s = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        out.push(heap.alloc_string(&s));
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
/// `(source-location 'name)` — where `name`'s global definition was loaded from,
/// as `[file line col]`, or `nil` if it has no recorded site (a builtin, a
/// prelude global, or an unknown/local name). The site is captured at load time
/// before macroexpansion, so `defn`/`defmacro` definitions are located
/// accurately. The image-query foundation for cross-file goto-definition (ADR-031
/// / docs/lsp.md). Takes a symbol, so quote it: `(source-location 'foo)`.
fn source_location(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let name = match arg(args, 0) {
        Value::Sym(s) => s,
        other => return Err(LispError::wrong_type(heap, "source-location", "symbol", other)),
    };
    match heap.def_site(name) {
        Some(loc) => {
            let file = heap.alloc_string(&loc.file);
            Ok(heap.alloc_vector(vec![
                file,
                Value::Int(loc.pos.line as i64),
                Value::Int(loc.pos.col as i64),
            ]))
        }
        None => Ok(Value::Nil),
    }
}

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
        // A primitive's docstring lives on the `NativeFn` (the `PRIMITIVE_DOCS`
        // table), since it has no Brood body to carry a leading string.
        Value::Native(id) => {
            let d = heap.native(id).doc;
            (!d.is_empty()).then(|| d.to_string())
        }
        _ => None,
    };
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::Nil),
    }
}

/// `(arglist f)` — the parameter list of a function, macro, or primitive as a
/// list, mirroring the source surface: required names, then `&optional` names,
/// then `& rest`. `nil` for a non-function (or a primitive without recorded
/// params). Feeds signature help / hover.
fn arglist(args: &[Value], _env: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Fn(id) | Value::Macro(id) => id,
        // A primitive carries its param names as a flat `&'static` list (incl. any
        // `&`/`&optional` markers, already in order) — hand them back as symbols.
        Value::Native(id) => {
            let params = heap.native(id).params;
            if params.is_empty() {
                return Ok(Value::Nil);
            }
            let items: Vec<Value> = params.iter().map(|p| value::sym(p)).collect();
            return Ok(heap.list(items));
        }
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
    let pid = crate::process::spawn(heap, arg(args, 0))?;
    Ok(crate::process::pid_value(pid))
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
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            // Local pid: in-process registration, returns a fresh mref.
            Ok(crate::process::monitor(id))
        }
        Value::Pid { node, id } => {
            // Remote pid: same shape — mint a mref, register *here* (so
            // demonitor can find it later, and net-split can fire
            // `:noconnection`), and ship a `Frame::Monitor` to the peer
            // which routes through the same `process::add_monitor` on the
            // far side.
            let mref = crate::process::next_ref();
            let watcher = crate::process::self_pid();
            crate::dist::monitor_remote(node, id, watcher, mref);
            Ok(Value::Ref(mref))
        }
        _ => Err(LispError::type_err("monitor: first argument must be a pid")),
    }
}

/// `(demonitor mref)` — drop the monitor created by `(monitor …)`. Tries the
/// local table first; if the mref isn't there it must have been on a remote
/// peer, so a `Frame::Demonitor` is fanned out to every connected peer that
/// holds a pending remote monitor with this watcher + mref.
fn demonitor(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    match arg(args, 0) {
        Value::Ref(n) => {
            // Local first (in-process MONITORS table).
            crate::process::demonitor(n);
            // Then ask any peer holding this mref to drop their watcher.
            // We scan PENDING_REMOTE for matching entries and `Demonitor` each
            // unique peer once. The same `process::drop_monitor` predicate the
            // local demonitor used is reused on the far side via the frame
            // handler.
            crate::process::demonitor_remote_fanout(n);
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
    Ok(crate::process::pid_value(crate::process::self_pid()))
}

/// `(ref)` — a fresh, globally-unique reference token. Shares the runtime's ref
/// counter with `(monitor …)` so every ref is distinct.
fn make_ref(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Ref(crate::process::next_ref()))
}

// ----- distributed nodes -----------------------------------------------------

/// Coerce a node/name argument (a keyword or symbol) to its interned `Symbol`.
fn expect_node_name(who: &str, v: Value) -> Result<value::Symbol, LispError> {
    match v {
        Value::Keyword(s) | Value::Sym(s) => Ok(s),
        _ => Err(LispError::type_err(format!(
            "{who}: expected a keyword or symbol name"
        ))),
    }
}

/// `(node-start name "host:port" cookie)` — name this runtime and listen for peer
/// nodes. Returns the node name.
fn node_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let name = expect_node_name("node-start", arg(args, 0))?;
    let addr = expect_string(heap, "node-start", arg(args, 1))?;
    let cookie = expect_string(heap, "node-start", arg(args, 2))?;
    crate::dist::node_start(name, &addr, cookie)
        .map_err(|e| LispError::runtime(format!("node-start: {e}")))?;
    Ok(Value::Keyword(name))
}

/// `(connect "name@host:port")` — link to a peer node (cookie-authenticated).
/// Returns the peer's node name on success.
fn connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let spec = expect_string(heap, "connect", arg(args, 0))?;
    let peer = crate::dist::connect(&spec).map_err(|e| LispError::runtime(format!("connect: {e}")))?;
    Ok(Value::Keyword(peer))
}

/// `(register name pid)` — bind a local name so peers can address this process by
/// `{:name name :node this-node}` before they hold its pid. Returns the pid.
fn register_name(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let name = expect_node_name("register", arg(args, 0))?;
    match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => {
            crate::dist::register(name, id);
            Ok(Value::Pid { node, id })
        }
        Value::Pid { .. } => Err(LispError::type_err(
            "register: can only register a local pid",
        )),
        _ => Err(LispError::type_err("register: second argument must be a pid")),
    }
}

/// `(node-name)` — this runtime's node name (`:nonode` until `node-start`).
fn node_name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Keyword(crate::dist::local_node()))
}

/// `(whereis name)` — the **local** pid registered under `name`, or `nil`.
/// Lets idempotent bootstrap shapes test for "is this server already running
/// here?" before re-`spawn`ing — see `remote-spawn` in `std/prelude.blsp`.
/// A remote-side registration isn't visible here; this is a strictly local
/// lookup over the `NAMES` table.
fn whereis_name(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let name = expect_node_name("whereis", arg(args, 0))?;
    match crate::dist::whereis(name) {
        Some(id) => Ok(Value::Pid {
            node: crate::dist::local_node(),
            id,
        }),
        None => Ok(Value::Nil),
    }
}

/// `(monitor-node name)` — the calling process is sent `[:nodedown name]` when a
/// link to `name` goes down (heartbeat timeout or clean close). Returns the name.
fn monitor_node(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    let name = expect_node_name("monitor-node", arg(args, 0))?;
    crate::dist::monitor_node(name, crate::process::self_pid());
    Ok(Value::Keyword(name))
}

/// `(nodes)` — a list of currently connected peer node names.
fn nodes(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let names: Vec<Value> = crate::dist::connected_nodes()
        .into_iter()
        .map(Value::Keyword)
        .collect();
    Ok(heap.list(names))
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

// ----- dynamic variables -----------------------------------------------------
//
// The kernel for `defdyn`/`binding`; the surface macros are in the prelude. A
// dynamic variable's *value* resolves through the per-process binding stack in
// the `Heap` (see `Heap::env_get`), so reads need no primitive here — only the
// declaration, the scoped rebind, and the predicate.

/// `(%declare-dynamic 'name)` — mark a symbol as a dynamic variable, so
/// `binding` will accept it (and `dynamic?` reports it). `defdyn` expands to
/// this plus a plain `def` of the default value.
fn declare_dynamic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%declare-dynamic", arg(args, 0))?;
    value::mark_dynamic(sym);
    Ok(Value::Sym(sym))
}

/// `(dynamic? x)` — true when `x` is a symbol declared dynamic with `defdyn`.
/// A non-symbol is simply not dynamic (no error), so it composes in predicates.
fn dynamic_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::Bool(matches!(arg(args, 0), Value::Sym(s) if value::is_dynamic(s))))
}

/// `(%binding syms vals thunk)` — run `thunk` (no args) with each dynamic var in
/// `syms` bound to the matching value in `vals` for the dynamic extent of the
/// call, restoring the previous bindings on return *or* error. `syms` (a quoted
/// list) and `vals` (a vector) are equal-length sequences built by the `binding`
/// macro — both emitted as unshadowable literals, so a local rebinding of `list`
/// can't break the form. Every name must be declared dynamic (else it's almost
/// certainly a typo — a plain global won't track the rebind). The bindings live
/// in this process's heap, so they don't reach a `spawn`ed child.
fn binding(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let syms = heap.seq_items(arg(args, 0))?;
    let vals = heap.seq_items(arg(args, 1))?;
    let thunk = arg(args, 2);
    // Validate every name up front, before pushing anything — so a bad `binding`
    // leaves the dynamic stack untouched rather than half-pushed.
    let mut names = Vec::with_capacity(syms.len());
    for s in &syms {
        let sym = expect_symbol(heap, "binding", *s)?;
        if !value::is_dynamic(sym) {
            return Err(LispError::runtime(format!(
                "binding: {} is not a dynamic variable (declare it with defdyn)",
                value::symbol_name(sym)
            )));
        }
        names.push(sym);
    }
    for (i, &sym) in names.iter().enumerate() {
        heap.push_dynamic(sym, arg(&vals, i));
    }
    let result = apply(heap, thunk, &[], env);
    for _ in 0..names.len() {
        heap.pop_dynamic();
    }
    result
}
