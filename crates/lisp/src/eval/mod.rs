//! The evaluator: a tree-walker with proper tail calls. Now heap-threaded — it
//! takes `&mut Heap` and addresses values by handle / `EnvId`.
//!
//! `macros` (quasiquote + the macroexpand compile pass) lives alongside it here:
//! the two are mutually recursive — the compile pass lowers the `fn`/`let`
//! pattern surfaces the evaluator runs, and the evaluator falls back to it.

pub mod macros;

use std::sync::LazyLock;

use smallvec::SmallVec;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::value::{self, Closure, ClosureId, EnvId, NativeId, Symbol, Value};
use crate::error::{LispError, LispResult};

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
    "fn",
    "lambda",
    "quasiquote",
    "defmacro",
    "let",
    "let*",
    "letrec",
];

// Keyed by interned symbol id — use the fast integer hasher, since `special_name`
// hits this on every combination (the default SipHash-on-a-`u32` is overhead).
static SPECIAL_IDS: LazyLock<SymbolMap<&'static str>> = LazyLock::new(|| {
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

    // GC-block guard: increments `GC_BLOCK` for the lifetime of this `eval`
    // frame. The safepoint below collects only when this is the **outermost**
    // contributor (`gc_block_depth() == 1`) — no other eval / macroexpand frame
    // is on the stack, so the eval's own loop-body locals (`head`/`rest`/
    // `callee`/`argv`/`scope`) are dead at `continue 'tail` and only `expr`/`env`
    // persist. `Drop` runs on every return path (including `?` and panic).
    let _gc_block = crate::process::GcBlockGuard::enter();

    'tail: loop {
        match expr {
            Value::Sym(s) => {
                // Tag the symbol's recorded source position on an unbound
                // error — so e.g. a `(let (a 1) zzz)` whose `zzz` is unbound
                // points at the *symbol's* line rather than the enclosing
                // top-level form's start. The `or_form_pos` lookup runs only
                // on the error path.
                let expr_sym = expr;
                return heap
                    .env_get(env, s)
                    .ok_or_else(|| {
                        LispError::unbound(format!("unbound symbol: {}", value::symbol_name(s)))
                    })
                    .map_err(|e| e.or_form_pos(heap, expr_sym));
            }
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(eval(heap, item, env)?);
                }
                return Ok(heap.alloc_vector(out));
            }
            Value::Map(id) => {
                // A map literal evaluates each key and value, then canonicalises
                // (last-wins on equal keys). Like a vector literal, but in pairs.
                let entries = heap.map(id).to_vec();
                let mut pairs = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    let k = eval(heap, k, env)?;
                    let v = eval(heap, v, env)?;
                    pairs.push((k, v));
                }
                return Ok(heap.map_from_pairs(pairs));
            }
            Value::Pair(_) => {} // combination, handled below
            _ => return Ok(expr),
        }

        // GC safepoint. Outermost eval only — inner evals (arg evaluation, body
        // forms, etc.) sit at `GC_BLOCK >= 2` and short-circuit. At the
        // outermost loop top:
        //   • `expr` / `env` are passed as roots,
        //   • the dynamic stack and the explicit root stack are scanned by
        //     `collect` itself,
        //   • no other Rust frame holds an unrooted LOCAL transient (the
        //     `GC_BLOCK == 1` invariant — see `docs/memory-model.md`).
        // Cost on inner-eval iterations: one TLS read + compare (fail-fast).
        if crate::process::gc_block_depth() == 1 && heap.gc_due() {
            heap.collect(&[expr], &[env]);
        }
        // Reduction-counted preemption: bound the work a process does before it
        // yields its worker (fairness — a CPU-bound process can't monopolise a
        // core). Counted per *combination* (a function call / special form), which
        // is where loops actually occur — leaf evals (symbols, literals) return
        // above without a tick. A cheap thread-local decrement; a no-op for the
        // root thread. See `process::tick`.
        crate::process::tick();

        let (head, rest) = match expr {
            Value::Pair(p) => heap.pair(p),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let Value::Sym(s) = head {
            match special_name(s) {
                "quote" => {
                    // `(quote x)` returns x literally — but only x; reject
                    // `(quote a b)` rather than silently dropping the tail.
                    let (form, r) = uncons(heap, rest);
                    if !matches!(r, Value::Nil) {
                        return Err(LispError::arity(
                            "quote: expected exactly one argument",
                        )
                        .or_form_pos(heap, expr));
                    }
                    return Ok(form);
                }
                "if" => {
                    // (if test then else?) — read the operands straight off the
                    // cons spine; a missing branch defaults to nil (as nth did),
                    // so no intermediate Vec is allocated per conditional.
                    let (test_form, r) = uncons(heap, rest);
                    let (then_form, r) = uncons(heap, r);
                    let (else_form, _) = uncons(heap, r);
                    let test = eval(heap, test_form, env)
                        .map_err(|e| e.or_form_pos(heap, test_form))?;
                    expr = if truthy(test) { then_form } else { else_form };
                    continue 'tail;
                }
                "do" => match tail_of_cons(heap, rest, env)? {
                    Some(last) => {
                        expr = last;
                        continue 'tail;
                    }
                    None => return Ok(Value::Nil),
                },
                "def" => {
                    let args = heap.list_to_vec(rest)?;
                    let name = as_symbol(
                        args.first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("def: missing name"))?,
                    )?;
                    let val = if args.len() > 1 {
                        eval(heap, args[1], env).map_err(|e| e.or_form_pos(heap, args[1]))?
                    } else {
                        Value::Nil
                    };
                    let val = name_value(heap, val, name);
                    let root = heap.env_root(env);
                    // Arity-change diagnostic: if `def` is *rebinding* a callable
                    // to one of a different arity, callers expecting the old shape
                    // will hit a runtime arity error on the next call. Surface it
                    // at reload time so the mismatch isn't a silent surprise (the
                    // hot-reload + late-binding contract — docs/shared-code.md).
                    // Fires only when an old binding exists, so the prelude/std
                    // first-time build is silent.
                    if let Some(old) = heap.env_get(root, name) {
                        if let (Some(old_a), Some(new_a)) =
                            (value_arity(heap, old), value_arity(heap, val))
                        {
                            if old_a.min != new_a.min || old_a.max != new_a.max {
                                eprintln!(
                                    "[reload] arity changed for {}: {} -> {}",
                                    value::symbol_name(name),
                                    arity_to_string(old_a),
                                    arity_to_string(new_a),
                                );
                            }
                        }
                    }
                    heap.env_define(root, name, val);
                    return Ok(Value::Sym(name));
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
                    // Inner unquote evals tag their own positions; this
                    // fallback uses the quasiquote combination only when an
                    // error somehow escaped without one (e.g. a missing-arg
                    // from `quasiquote` itself).
                    return crate::eval::macros::quasiquote(heap, template, env)
                        .map_err(|e| e.or_form_pos(heap, expr));
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
                    let (binds_form, body) = uncons(heap, rest);
                    if !matches!(rest, Value::Pair(_)) {
                        return Err(LispError::runtime("let: missing bindings"));
                    }
                    let binds = as_binding_vec(heap, binds_form)?;
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
                        let rhs = binds[i + 1];
                        let val = eval(heap, rhs, scope)
                            .map_err(|e| e.or_form_pos(heap, rhs))?;
                        heap.env_define(scope, bind_name, val);
                        i += 2;
                    }
                    match tail_of_cons(heap, body, scope)? {
                        Some(last) => {
                            expr = last;
                            env = scope;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                "letrec" => {
                    // Mutual local recursion: every binding's name is visible in
                    // every binding's RHS (and to itself). The frame is a
                    // last-write-wins association vector (`env_define` pushes,
                    // lookup scans from the end), so we make all names visible by
                    // pre-defining them to nil, then evaluate each RHS in the
                    // scope and push the real value — closures built during the
                    // bind phase capture the scope and resolve names lazily at
                    // call time, so mutual recursion just works. Brood stays
                    // immutable: the user can't observe the nil pre-binding
                    // (touching a name before its RHS has been evaluated reads
                    // the placeholder, but for the recursion-of-functions case
                    // letrec is meant for, the bodies don't fire until call
                    // time, by which point the real value is the latest entry).
                    let (binds_form, body) = uncons(heap, rest);
                    if !matches!(rest, Value::Pair(_)) {
                        return Err(LispError::runtime("letrec: missing bindings"));
                    }
                    let binds = as_binding_vec(heap, binds_form)?;
                    if binds.len() % 2 != 0 {
                        return Err(LispError::runtime(
                            "letrec: bindings must be name/value pairs",
                        ));
                    }
                    // Plain-symbol targets only — letrec exists for mutual
                    // recursion of *named* values; pattern binding would muddy
                    // what "the names visible in the RHSs" means.
                    if binds
                        .iter()
                        .step_by(2)
                        .any(|&b| !matches!(b, Value::Sym(_)))
                    {
                        return Err(LispError::runtime(
                            "letrec: binding targets must be plain symbols",
                        ));
                    }
                    let scope = heap.new_env(Some(env));
                    let mut i = 0;
                    while i < binds.len() {
                        let bind_name = as_symbol(binds[i])?;
                        heap.env_define(scope, bind_name, Value::Nil);
                        i += 2;
                    }
                    let mut i = 0;
                    while i < binds.len() {
                        let bind_name = as_symbol(binds[i])?;
                        let rhs = binds[i + 1];
                        let val = eval(heap, rhs, scope)
                            .map_err(|e| e.or_form_pos(heap, rhs))?;
                        heap.env_define(scope, bind_name, val);
                        i += 2;
                    }
                    match tail_of_cons(heap, body, scope)? {
                        Some(last) => {
                            expr = last;
                            env = scope;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                _ => {}
            }
        }

        // --- macro expansion + function application ---
        // Resolve the head once: a symbol head is looked up in the environment
        // (a single walk, shared by the macro check and the callee), any other
        // head form is evaluated. A macro expands and re-enters the loop; anything
        // else becomes the callee. (Previously the head was resolved twice — once
        // for the macro test, once via `eval(head)` — doubling global-table hits
        // on the hottest path.)
        // Source position of *this* combination — used as the fallback when an
        // error bubbles up from a sub-eval / primitive / closure-arity check
        // that didn't tag itself with a more-inner position. `or_form_pos`
        // checks `pos.is_none()` first, so the innermost annotation wins; this
        // is only a cost on the error path (`form_pos` lookup is skipped
        // entirely when the error is already tagged).
        let call_form = expr;

        let callee = match head {
            Value::Sym(s) => {
                // An unbound-symbol error from a *tail-position* call (the
                // last form of a `do`/`let`/`letrec` body, set as `expr` via
                // `continue 'tail`) exits this eval frame directly — no outer
                // `or_form_pos` will see it. Attach `call_form`'s position
                // here so the diagnostic points at the failing call's line,
                // not the enclosing top-level form's start.
                let v = heap
                    .env_get(env, s)
                    .ok_or_else(|| {
                        LispError::unbound(format!("unbound symbol: {}", value::symbol_name(s)))
                    })
                    .map_err(|e| e.or_form_pos(heap, call_form))?;
                if let Value::Macro(mid) = v {
                    let arg_forms = heap.list_to_vec(rest)?;
                    expr = apply_closure(heap, mid, &arg_forms)
                        .map_err(|e| e.or_form_pos(heap, call_form))?;
                    continue 'tail;
                }
                v
            }
            _ => eval(heap, head, env).map_err(|e| e.or_form_pos(heap, head))?,
        };

        // Evaluate the argument forms straight off the `rest` cons spine into
        // `argv`, without first collecting them into an intermediate Vec. Inline
        // storage (no heap alloc) for the common small-arity call.
        let mut argv: SmallVec<[Value; 8]> = SmallVec::new();
        let mut cur = rest;
        loop {
            match cur {
                Value::Nil => break,
                Value::Pair(p) => {
                    let (form, next) = heap.pair(p);
                    argv.push(eval(heap, form, env).map_err(|e| e.or_form_pos(heap, form))?);
                    cur = next;
                }
                _ => return Err(LispError::type_err("improper argument list in call")),
            }
        }

        // Inline `apply` unfolding: when the callee is the `apply` builtin,
        // splice the trailing sequence into `argv` and dispatch the real
        // callee here — instead of going through `call_native(apply)` →
        // `apply_builtin` → `eval::apply` (which would add ~4 Rust frames per
        // chained `(apply f …)` and defeat TCO for `apply`-driven recursion).
        // Looped so nested `(apply apply (list f xs))` is also flattened.
        // A mis-arity (`(apply f)`) is left for `call_native` to flag with the
        // canonical "expected at least 2 arguments" message.
        let mut cur_callee = callee;
        let mut cur_argv = argv;
        while let Value::Native(id) = cur_callee {
            if heap.native(id).name != "apply" || cur_argv.len() < 2 {
                break;
            }
            let last = cur_argv.pop().expect("argv non-empty (checked above)");
            let real = cur_argv.remove(0);
            cur_argv.extend(
                heap.seq_items(last)
                    .map_err(|e| e.or_form_pos(heap, call_form))?,
            );
            cur_callee = real;
        }

        match cur_callee {
            Value::Native(id) => {
                return call_native(heap, id, &cur_argv, env)
                    .map_err(|e| e.or_form_pos(heap, call_form));
            }
            Value::Fn(id) => {
                let scope = bind_params(heap, id, &cur_argv)
                    .map_err(|e| e.or_form_pos(heap, call_form))?;
                let body_len = heap.closure(id).body.len();
                if body_len == 0 {
                    return Ok(Value::Nil);
                }
                for i in 0..body_len - 1 {
                    let form = heap.closure(id).body[i];
                    eval(heap, form, scope).map_err(|e| e.or_form_pos(heap, form))?;
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
        // Same as the closure body branch in `eval`: tag the body form's
        // position on any error so the diagnostic points at the failing line.
        result = eval(heap, form, scope).map_err(|e| e.or_form_pos(heap, form))?;
    }
    Ok(result)
}

fn bind_params(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> Result<EnvId, LispError> {
    // Snapshot the closure's metadata once. Every re-read of `heap.closure(cl)`
    // is a region-dispatch + slab index, and the body below would otherwise
    // re-read it ~4-6 times per call plus once per parameter in the binding
    // loop. Closures are immutable once allocated, so this snapshot stays
    // consistent. `params` and `optionals` copy into inline `SmallVec`s, so
    // frames of ≤4 params/optionals (the common case) pay no heap alloc here.
    let mut params: SmallVec<[Symbol; 4]> = SmallVec::new();
    let mut optionals: SmallVec<[(Symbol, Value); 4]> = SmallVec::new();
    let (cl_env_opt, cl_name, rest_sym) = {
        let cl_data = heap.closure(cl);
        params.extend_from_slice(&cl_data.params);
        optionals.extend_from_slice(&cl_data.optionals);
        (cl_data.env, cl_data.name, cl_data.rest)
    };
    // A global-capturing closure (env == None) resolves to this process's global.
    let cl_env = cl_env_opt.unwrap_or_else(|| heap.global());
    let required = params.len();
    let n_opt = optionals.len();
    let has_rest = rest_sym.is_some();
    let max = if has_rest {
        usize::MAX
    } else {
        required + n_opt
    };

    if argv.len() < required || argv.len() > max {
        let who = cl_name
            .map(value::symbol_name)
            .unwrap_or_else(|| "fn".to_string());
        let max = if has_rest {
            None
        } else {
            Some(required + n_opt)
        };
        return Err(LispError::arity(arity_message(
            &who,
            required,
            max,
            argv.len(),
        )));
    }

    let scope = heap.new_env(Some(cl_env));
    for (i, &arg) in argv.iter().enumerate().take(required) {
        heap.env_define(scope, params[i], arg);
    }
    let mut idx = required;
    for j in 0..n_opt {
        let (name, default_form) = optionals[j];
        if idx < argv.len() {
            heap.env_define(scope, name, argv[idx]);
            idx += 1;
        } else {
            // Tag the default-form's source position on any error from its
            // evaluation, so a diagnostic from inside an `&optional` default
            // points at the default's line (not at the enclosing top-level
            // form's start).
            let value = eval(heap, default_form, scope)
                .map_err(|e| e.or_form_pos(heap, default_form))?;
            heap.env_define(scope, name, value);
        }
    }
    if let Some(rs) = rest_sym {
        let rest_list = heap.list_from_slice(&argv[idx..]);
        heap.env_define(scope, rs, rest_list);
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
    let mut body = parts[1..].to_vec();
    // A leading string literal is a docstring *only when more body follows* —
    // a function whose body is a lone string returns that string (CL/Elisp
    // rule). Strip it out so it isn't evaluated for effect each call.
    let doc = match body.first() {
        Some(&Value::Str(id)) if body.len() > 1 => {
            let d = heap.string(id).to_string();
            body.remove(0);
            Some(d)
        }
        _ => None,
    };
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
        doc,
        env: captured,
    });
    Ok(Value::Fn(id))
}

/// A parsed parameter list: required params, `&optional` params with their
/// default forms, and an optional `&` rest param.
type ParamSpec = (Vec<Symbol>, Vec<(Symbol, Value)>, Option<Symbol>);

fn parse_params(heap: &Heap, form: Value) -> Result<ParamSpec, LispError> {
    let items = heap
        .seq_items(form)
        .map_err(|_| LispError::type_err("parameter list must be a list (x y) or vector [x y]"))?;

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
            let parts = heap.seq_items(form)?;
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
    heap.seq_items(v).map_err(|_| {
        LispError::type_err("let bindings must be a list (a 1 b 2) or vector [a 1 b 2]")
    })
}

/// Split a list cell into `(head, tail)`; `(nil, nil)` if it isn't a pair. For
/// reading a fixed number of operands off a form's argument spine (`if`, `let`'s
/// bindings/body split) without materializing a `Vec`.
fn uncons(heap: &Heap, v: Value) -> (Value, Value) {
    match v {
        Value::Pair(p) => heap.pair(p),
        _ => (Value::Nil, Value::Nil),
    }
}

/// Evaluate all-but-last of the forms in the cons-list `body` for effect; return
/// the last form (or `None` if empty). Walks the spine directly, so a `do`/`let`
/// body costs no intermediate `Vec`. Errors on an improper list. Each non-tail
/// form's source position is attached to any error it raises (innermost wins),
/// so a `do`-body failure points at the failing form's line, not the enclosing
/// `do`'s.
fn tail_of_cons(heap: &mut Heap, body: Value, env: EnvId) -> Result<Option<Value>, LispError> {
    let mut cur = body;
    loop {
        match cur {
            Value::Nil => return Ok(None),
            Value::Pair(p) => {
                let (form, next) = heap.pair(p);
                if matches!(next, Value::Nil) {
                    return Ok(Some(form));
                }
                eval(heap, form, env).map_err(|e| e.or_form_pos(heap, form))?;
                cur = next;
            }
            _ => return Err(LispError::type_err("improper body list")),
        }
    }
}

/// Arity of a callable value (closure, macro, or native primitive), or `None`
/// for non-callables. Used by the `def` arity-change diagnostic to compare an
/// old binding's shape against a new one's.
fn value_arity(heap: &Heap, v: Value) -> Option<value::Arity> {
    match v {
        Value::Fn(id) | Value::Macro(id) => {
            let c = heap.closure(id);
            let min = c.params.len();
            let max = if c.rest.is_some() {
                None
            } else {
                Some(min + c.optionals.len())
            };
            Some(value::Arity { min, max })
        }
        Value::Native(id) => Some(heap.native(id).arity),
        _ => None,
    }
}

/// Render an `Arity` as a compact "N", "N-M", or "N+" string for the
/// arity-change diagnostic.
fn arity_to_string(a: value::Arity) -> String {
    match a.max {
        Some(max) if max == a.min => format!("{}", a.min),
        Some(max) => format!("{}-{}", a.min, max),
        None => format!("{}+", a.min),
    }
}
