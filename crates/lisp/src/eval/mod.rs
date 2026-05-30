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

/// The evaluator's special forms, as a closed enum. The hot path (every
/// combination) dispatches on the head symbol's interned id (a `u32`) to one of
/// these — or `None` for an ordinary call — then `match`es the *enum* (a jump on
/// a small discriminant). Previously this returned a `&'static str` and the
/// caller matched on the string; the enum drops those per-form string compares
/// and gives the compiler a dense jump table. (It still avoids `symbol_name`'s
/// global-interner lock and `String` allocation, as before.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SpecialForm {
    Quote,
    If,
    Do,
    Def,
    Fn, // `fn` and `lambda` — surface synonyms, identical semantics
    Quasiquote,
    Defmacro,
    Let, // `let` and `let*` — surface synonyms here (sequential binding)
    Letrec,
}

/// Spelling → form. `fn`/`lambda` and `let`/`let*` collapse to one variant each.
/// This is deliberately the evaluator-*core* subset; `builtins.rs::SPECIAL_FORMS`
/// is the broader, LSP-facing list (it also names the macro keywords).
const SPECIAL_SPELLINGS: &[(&str, SpecialForm)] = &[
    ("quote", SpecialForm::Quote),
    ("if", SpecialForm::If),
    ("do", SpecialForm::Do),
    ("def", SpecialForm::Def),
    ("fn", SpecialForm::Fn),
    ("lambda", SpecialForm::Fn),
    ("quasiquote", SpecialForm::Quasiquote),
    ("defmacro", SpecialForm::Defmacro),
    ("let", SpecialForm::Let),
    ("let*", SpecialForm::Let),
    ("letrec", SpecialForm::Letrec),
];

// Keyed by interned symbol id — use the fast integer hasher, since `special_form`
// hits this on every combination (the default SipHash-on-a-`u32` is overhead).
static SPECIAL_IDS: LazyLock<SymbolMap<SpecialForm>> = LazyLock::new(|| {
    SPECIAL_SPELLINGS
        .iter()
        .map(|&(n, f)| (value::intern(n), f))
        .collect()
});

#[inline]
fn special_form(s: Symbol) -> Option<SpecialForm> {
    SPECIAL_IDS.get(&s).copied()
}

pub fn eval(heap: &mut Heap, expr: Value, env: EnvId) -> LispResult {
    let mut expr = expr;
    let mut env = env;
    #[cfg(debug_assertions)]
    if crate::process::in_green_process() && heap.env_is_poisoned(env) {
        eprintln!(
            "[entry] eval entered with POISONED env={:#x} gc_block={}",
            env.0,
            crate::process::gc_block_depth()
        );
    }

    // GC-block guard: increments `GC_BLOCK` for the lifetime of this `eval` frame.
    // Since ADR-061 the GC safepoint no longer gates on this depth (it collects at
    // any eval depth — every frame roots its transients on the operand stack); the
    // guard now feeds only the stack-overflow byte guard below, which keys its base
    // off the outermost eval (`gc_block_depth() <= 1`). `Drop` runs on every return
    // path (including `?` and panic).
    let _gc_block = crate::process::GcBlockGuard::enter();

    // Stack-budget guard (ADR-043; runaway non-tail recursion). Every nested
    // `eval` frame is real Rust stack; an unbounded non-tail recursion —
    // `(defn boom (n) (+ 1 (boom (+ n 1))))` — would overflow the coroutine
    // stack as a SIGSEGV the host can't `catch_unwind`, aborting the whole REPL
    // / `nest mcp` server. We probe the current stack pointer (address of a
    // local in *this* frame) and compare bytes-used since the outermost eval
    // against the budget; crossing it fails *here* with a clean, catchable
    // error. Tail calls re-enter the `'tail:` loop below without a new frame, so
    // they consume no extra stack and never trip this — it only ever bites
    // runaway *non-tail* recursion. One TLS read + compare on entry. (Bytes, not
    // frame count — see `process::stack_overflow_check` for why.)
    let stack_probe = 0u8;
    if let Some(used) = crate::process::stack_overflow_check(&stack_probe as *const u8 as usize) {
        return Err(LispError::runtime(format!(
            "recursion too deep: used {used} bytes of stack, over the \
             {}-byte budget (runaway non-tail recursion?)",
            crate::process::stack_budget()
        ))
        .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
        .with_hint(
            "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), \
             or raise the budget with BROOD_STACK_BUDGET",
        )
        .or_form_pos(heap, expr));
    }

    'tail: loop {
        match expr {
            Value::Sym(s) => {
                #[cfg(debug_assertions)]
                heap.debug_walk_env_chain(env, s);
                let expr_sym = expr;
                return heap
                    .env_get(env, s)
                    .ok_or_else(|| unbound_error(s))
                    .map_err(|e| e.or_form_pos(heap, expr_sym));
            }
            Value::Vector(id) => {
                // A vector literal evaluates each element. Those evals can collect
                // at ANY depth (ADR-061), so keep both the unevaluated elements and
                // the accumulated results on the operand stack across them.
                let items = heap.vector(id).to_vec();
                let n = items.len();
                let vb = heap.roots_len();
                let eb = heap.env_roots_len();
                heap.push_env_root(env);
                for &it in &items {
                    heap.push_root(it); // vb .. : source elements
                }
                let out_base = heap.roots_len(); // accumulated results
                for i in 0..n {
                    let env_now = heap.env_root_at(eb);
                    let item = heap.root_at(vb + i);
                    match eval(heap, item, env_now) {
                        Ok(v) => heap.push_root(v),
                        Err(e) => {
                            heap.truncate_roots(vb);
                            heap.truncate_env_roots(eb);
                            return Err(e);
                        }
                    }
                }
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    out.push(heap.root_at(out_base + i));
                }
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Ok(heap.alloc_vector(out));
            }
            Value::Map(id) => {
                // A map literal evaluates each key and value, then canonicalises
                // (last-wins on equal keys). Like a vector literal, but in pairs —
                // and likewise operand-stack-rooted so a deep collection during an
                // element eval can't dangle the source forms or accumulated pairs.
                let entries = heap.map_entries(id);
                let n = entries.len();
                let vb = heap.roots_len();
                let eb = heap.env_roots_len();
                heap.push_env_root(env);
                for &(k, v) in &entries {
                    heap.push_root(k); // vb + 2i     : source key
                    heap.push_root(v); // vb + 2i + 1 : source value
                }
                let res_base = heap.roots_len(); // accumulated (k, v) results, flattened
                for i in 0..n {
                    let env_now = heap.env_root_at(eb);
                    let kf = heap.root_at(vb + 2 * i);
                    let kv = match eval(heap, kf, env_now) {
                        Ok(x) => x,
                        Err(e) => {
                            heap.truncate_roots(vb);
                            heap.truncate_env_roots(eb);
                            return Err(e);
                        }
                    };
                    heap.push_root(kv);
                    let env_now = heap.env_root_at(eb);
                    let vf = heap.root_at(vb + 2 * i + 1);
                    let vv = match eval(heap, vf, env_now) {
                        Ok(x) => x,
                        Err(e) => {
                            heap.truncate_roots(vb);
                            heap.truncate_env_roots(eb);
                            return Err(e);
                        }
                    };
                    heap.push_root(vv);
                }
                let mut pairs = Vec::with_capacity(n);
                for i in 0..n {
                    pairs.push((heap.root_at(res_base + 2 * i), heap.root_at(res_base + 2 * i + 1)));
                }
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Ok(heap.map_from_pairs(pairs));
            }
            Value::Pair(_) => {} // combination, handled below
            _ => return Ok(expr),
        }

        // GC safepoint — fires at ANY eval depth (ADR-061). The loop-top is the
        // one point in a frame where its only live LOCAL transients are `expr` /
        // `env` (the per-iteration `argv`/`scope`/… were already torn down or not
        // yet built); they're passed as roots below. Every *ancestor* frame's live
        // transients sit on the heap operand stack (`roots`/`env_roots`), which
        // `collect` relocates in place, and the dynamic stack is scanned too — so
        // no Rust frame holds an unrooted LOCAL handle a moving collection would
        // strand. The one exception is the macro-expansion compile pass (its
        // forms aren't operand-stack rooted), which opts out via `MACRO_BLOCK`.
        // (The soft-memory-limit check below is deliberately *not* gated.) Cost
        // when not collecting: one TLS read + a `gc_due` compare.
        if !crate::process::macro_block_active() && heap.gc_due() {
            // Copying collection: `expr`/`env` MOVE to fresh slabs, so write the
            // relocated handles back into the loop's live registers (the dynamic
            // stack and explicit root stack are relocated in place by `collect`).
            let mut roots = [expr];
            let mut envs = [env];
            heap.collect(&mut roots, &mut envs);
            expr = roots[0];
            env = envs[0];
        }
        // Memory safety backstop (ADR-043): if total allocation has crossed the
        // soft ceiling, fail *here* with a clean, catchable error rather than
        // running on to the hard allocator limit (which aborts the whole
        // process). Off by default; the test runners set it so adversarial /
        // hostile code can't exhaust host RAM. Process-wide, not per-process
        // (we only account bytes process-wide today).
        //
        // Unlike the GC safepoint above this is **not** gated on `GC_BLOCK == 1`:
        // raising the error merely returns `Err` and unwinds (every `GcBlockGuard`
        // drops on the way out) — it never frees or moves a LOCAL value, so the
        // "no unrooted transient at depth > 1" invariant that constrains `collect`
        // doesn't apply. Checking at every depth is what makes the limit actually
        // catch a runaway loop sitting in *argument* position (`(f (build …))`),
        // which runs at `GC_BLOCK >= 2` and would otherwise sail past this
        // safepoint straight into the hard-limit abort. Cheap when disabled: one
        // relaxed load of a zero, early `None`.
        if let Some(used) = crate::core::alloc::soft_limit_hit() {
            return Err(LispError::runtime(format!(
                "memory limit exceeded: {used} bytes allocated process-wide \
                 exceeds the {}-byte soft limit (raise or unset BROOD_MEM_LIMIT)",
                crate::core::alloc::soft_limit()
            ))
            .with_code(crate::error::error_codes::MEMORY_LIMIT)
            .or_form_pos(heap, expr));
        }
        // Reduction-counted preemption: bound the work a process does before it
        // yields its worker (fairness — a CPU-bound process can't monopolise a
        // core). Counted per *combination* (a function call / special form), which
        // is where loops actually occur — leaf evals (symbols, literals) return
        // above without a tick. A cheap thread-local decrement; a no-op for the
        // root thread. See `process::tick`.
        crate::process::tick();
        // Eval deadline (the `nest mcp` watchdog): abort a runaway that's exceeded
        // its time budget so it can't wedge the server. Inline — propagates as an
        // ordinary error through the dispatcher's existing handling. Cheap when no
        // deadline is armed (one thread-local `Cell` get).
        if crate::process::deadline_exceeded() {
            return Err(LispError::runtime(
                "evaluation exceeded its time limit (MCP tool watchdog)",
            )
            .or_form_pos(heap, expr));
        }

        let (head, rest) = match expr {
            Value::Pair(p) => heap.pair(p),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let Value::Sym(s) = head {
            match special_form(s) {
                Some(SpecialForm::Quote) => {
                    // `(quote x)` returns x literally — but only x; reject
                    // `(quote a b)` rather than silently dropping the tail.
                    let (form, r) = uncons(heap, rest);
                    if !matches!(r, Value::Nil) {
                        return Err(LispError::arity("quote: expected exactly one argument")
                            .or_form_pos(heap, expr));
                    }
                    return Ok(form);
                }
                Some(SpecialForm::If) => {
                    // (if test then else?) — read the operands straight off the
                    // cons spine; a missing branch defaults to nil (as nth did),
                    // so no intermediate Vec is allocated per conditional.
                    let (test_form, r) = uncons(heap, rest);
                    let (then_form, r) = uncons(heap, r);
                    let (else_form, _) = uncons(heap, r);
                    // Evaluating the test can collect at ANY depth (ADR-061), so
                    // keep the unchosen branches + env on the operand stack and
                    // re-read the relocated handles before the tail hand-off.
                    let vb = heap.roots_len();
                    let eb = heap.env_roots_len();
                    heap.push_env_root(env);
                    heap.push_root(then_form); // vb+0
                    heap.push_root(else_form); // vb+1
                    let test = match eval(heap, test_form, env)
                        .map_err(|e| e.or_form_pos(heap, test_form))
                    {
                        Ok(t) => t,
                        Err(e) => {
                            heap.truncate_roots(vb);
                            heap.truncate_env_roots(eb);
                            return Err(e);
                        }
                    };
                    let then_form = heap.root_at(vb);
                    let else_form = heap.root_at(vb + 1);
                    env = heap.env_root_at(eb);
                    heap.truncate_roots(vb);
                    heap.truncate_env_roots(eb);
                    expr = if truthy(test) { then_form } else { else_form };
                    continue 'tail;
                }
                Some(SpecialForm::Do) => match tail_of_cons(heap, rest, env)? {
                    Some((last, env_r)) => {
                        expr = last;
                        env = env_r;
                        continue 'tail;
                    }
                    None => return Ok(Value::Nil),
                },
                Some(SpecialForm::Def) => {
                    let args = heap.list_to_vec(rest)?;
                    let name = as_symbol(
                        args.first()
                            .copied()
                            .ok_or_else(|| LispError::runtime("def: missing name"))?,
                    )?;
                    let val = if args.len() > 1 {
                        // The value eval can collect at any depth (ADR-061); root
                        // `env` across it (the result is fresh post-collection, and
                        // `name` is an interned symbol — neither needs re-reading).
                        let eb = heap.env_roots_len();
                        heap.push_env_root(env);
                        let out =
                            eval(heap, args[1], env).map_err(|e| e.or_form_pos(heap, args[1]));
                        env = heap.env_root_at(eb);
                        heap.truncate_env_roots(eb);
                        out?
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
                Some(SpecialForm::Fn) => {
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
                Some(SpecialForm::Quasiquote) => {
                    let args = heap.list_to_vec(rest)?;
                    let template = args.into_iter().next().unwrap_or(Value::Nil);
                    // Inner unquote evals tag their own positions; this
                    // fallback uses the quasiquote combination only when an
                    // error somehow escaped without one (e.g. a missing-arg
                    // from `quasiquote` itself).
                    return crate::eval::macros::quasiquote(heap, template, env)
                        .map_err(|e| e.or_form_pos(heap, expr));
                }
                Some(SpecialForm::Defmacro) => {
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
                    // Staleness diagnostic (hot reload): redefining a macro does
                    // *not* re-expand callers already compiled with the old
                    // expansion — they keep the old code until re-evaluated. Warn
                    // when *rebinding* an existing macro so the mismatch isn't a
                    // silent surprise; silent on first definition (the prelude/std
                    // build, and a file's first load). Mirrors the `def`
                    // arity-change diagnostic above (docs/live-editing.md Stage 7).
                    if matches!(heap.env_get(root, name), Some(Value::Macro(_))) {
                        eprintln!(
                            "[reload] macro {} redefined; callers expanded before now keep the old expansion — re-eval them",
                            value::symbol_name(name)
                        );
                    }
                    heap.env_define(root, name, macro_val);
                    return Ok(Value::Sym(name));
                }
                Some(SpecialForm::Let) => {
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
                    let (scope, body) = bind_sequential(heap, &binds, scope, body)?;
                    match tail_of_cons(heap, body, scope)? {
                        Some((last, env_r)) => {
                            expr = last;
                            env = env_r;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                Some(SpecialForm::Letrec) => {
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
                    // Pre-define every name to nil so all are visible in every RHS
                    // (no eval here → no GC). The RHS evals then run via the shared
                    // operand-stack-rooted helper.
                    let mut i = 0;
                    while i < binds.len() {
                        let bind_name = as_symbol(binds[i])?;
                        heap.env_define(scope, bind_name, Value::Nil);
                        i += 2;
                    }
                    let (scope, body) = bind_sequential(heap, &binds, scope, body)?;
                    match tail_of_cons(heap, body, scope)? {
                        Some((last, env_r)) => {
                            expr = last;
                            env = env_r;
                            continue 'tail;
                        }
                        None => return Ok(Value::Nil),
                    }
                }
                None => {}
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
        let mut call_form = expr;
        let mut spine = rest;

        let callee = match head {
            Value::Sym(s) => {
                #[cfg(debug_assertions)]
                heap.debug_walk_env_chain(env, s);
                // An unbound-symbol error from a *tail-position* call (the
                // last form of a `do`/`let`/`letrec` body, set as `expr` via
                // `continue 'tail`) exits this eval frame directly — no outer
                // `or_form_pos` will see it. Attach `call_form`'s position
                // here so the diagnostic points at the failing call's line,
                // not the enclosing top-level form's start. (No GC can run
                // between here and the lookup — `env_get` doesn't eval.)
                let v = heap
                    .env_get(env, s)
                    .ok_or_else(|| unbound_error(s))
                    .map_err(|e| e.or_form_pos(heap, call_form))?;
                if let Value::Macro(mid) = v {
                    // Macro expansion can collect at any depth (ADR-061); root
                    // `env` across it so the `continue 'tail` re-reads the
                    // relocated handle. `arg_forms` is consumed by `bind_params`
                    // inside `apply_closure`, which roots it itself.
                    let arg_forms = heap.list_to_vec(spine)?;
                    let eb = heap.env_roots_len();
                    heap.push_env_root(env);
                    let out =
                        apply_closure(heap, mid, &arg_forms).map_err(|e| e.or_form_pos(heap, call_form));
                    env = heap.env_root_at(eb);
                    heap.truncate_env_roots(eb);
                    expr = out?;
                    continue 'tail;
                }
                v
            }
            _ => {
                // A computed head (`((f) …)`) is evaluated — and that eval can
                // collect at any depth, so root `call_form` + `env` across it,
                // then re-read the relocated handles and re-derive the spine
                // from the moved `call_form`.
                let vb = heap.roots_len();
                let eb = heap.env_roots_len();
                heap.push_root(call_form);
                heap.push_env_root(env);
                let out = eval(heap, head, env).map_err(|e| e.or_form_pos(heap, head));
                call_form = heap.root_at(vb);
                env = heap.env_root_at(eb);
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                let callee = out?;
                spine = match call_form {
                    Value::Pair(p) => heap.pair(p).1,
                    _ => Value::Nil,
                };
                callee
            }
        };

        // Evaluate the argument forms off the cons spine into an owned `argv`,
        // keeping the callee / accumulated args / spine cursor on the operand
        // stack (env on the env stack) so a collection at ANY eval depth
        // relocates them in place (ADR-061). Returns the relocated callee /
        // call_form / env alongside `argv`.
        let (argv, callee, call_form, env_r) = eval_arguments(heap, callee, call_form, spine, env)?;
        env = env_r;

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
                // `bind_params` selects the arm matching this call's arity, binds
                // it, and hands back that arm's body (snapshotted into an inline
                // `SmallVec` so the loop below doesn't re-dispatch the slab).
                let (scope, body) =
                    bind_params(heap, id, &cur_argv).map_err(|e| e.or_form_pos(heap, call_form))?;
                if body.is_empty() {
                    return Ok(Value::Nil);
                }
                let (last, init) = body.split_last().expect("checked non-empty");
                if init.is_empty() {
                    // Single-form body (the common case): hand `last` straight to
                    // the loop — the safepoint roots `expr`/`env` for us, so no
                    // operand-stack push is needed.
                    expr = *last;
                    env = scope;
                    continue 'tail;
                }
                // Multi-form body: each non-last form is evaluated for effect, and
                // an eval can collect at any depth (ADR-061) — so root `scope`,
                // `last`, and the remaining body forms across those evals, then
                // re-read the relocated handles for the tail hand-off.
                let vb = heap.roots_len();
                let eb = heap.env_roots_len();
                heap.push_env_root(scope);
                heap.push_root(*last); // vb+0
                for &form in init {
                    heap.push_root(form); // vb+1 ..
                }
                for i in 0..init.len() {
                    let scope_now = heap.env_root_at(eb);
                    let form = heap.root_at(vb + 1 + i);
                    if let Err(e) = eval(heap, form, scope_now).map_err(|e| e.or_form_pos(heap, form)) {
                        heap.truncate_roots(vb);
                        heap.truncate_env_roots(eb);
                        return Err(e);
                    }
                }
                expr = heap.root_at(vb);
                env = heap.env_root_at(eb);
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
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

/// Evaluate a combination's argument forms off the `spine` cons list into an
/// owned `argv`, while keeping every LOCAL transient the call still needs on the
/// heap **operand stack** (ADR-061): `call_form`, `callee`, the spine cursor, and
/// each accumulated arg live on `heap.roots`; `env` lives on `heap.env_roots`.
/// The copying collector relocates those in place, so a GC safepoint at ANY eval
/// depth (not just the outermost) is safe here. Returns the owned `argv` plus the
/// post-collection (relocated) `callee`, `call_form`, and `env`. The operand-stack
/// region is always torn down before returning, including on the error path.
#[inline]
fn eval_arguments(
    heap: &mut Heap,
    callee: Value,
    call_form: Value,
    spine: Value,
    env: EnvId,
) -> Result<(SmallVec<[Value; 8]>, Value, Value, EnvId), LispError> {
    let vbase = heap.roots_len();
    let ebase = heap.env_roots_len();
    heap.push_root(call_form); // vbase+0
    heap.push_root(callee); // vbase+1
    heap.push_root(spine); // vbase+2 — the cons-spine cursor, advanced in place
    heap.push_env_root(env); // ebase
                             // Evaluated args accumulate at vbase+3 ..
    loop {
        let cur = heap.root_at(vbase + 2);
        let form = match cur {
            Value::Nil => break,
            Value::Pair(p) => heap.pair(p).0,
            _ => {
                heap.truncate_roots(vbase);
                heap.truncate_env_roots(ebase);
                return Err(LispError::type_err("improper argument list in call"));
            }
        };
        let env_now = heap.env_root_at(ebase);
        match eval(heap, form, env_now).map_err(|e| e.or_form_pos(heap, form)) {
            Ok(v) => heap.push_root(v),
            Err(e) => {
                heap.truncate_roots(vbase);
                heap.truncate_env_roots(ebase);
                return Err(e);
            }
        }
        // Advance the cursor from the (possibly relocated) slot, not the stale
        // `cur`/`next` read before the eval.
        let next = match heap.root_at(vbase + 2) {
            Value::Pair(p) => heap.pair(p).1,
            _ => Value::Nil,
        };
        heap.set_root(vbase + 2, next);
    }
    let argc = heap.roots_len() - (vbase + 3);
    let mut argv: SmallVec<[Value; 8]> = SmallVec::with_capacity(argc);
    for i in 0..argc {
        argv.push(heap.root_at(vbase + 3 + i));
    }
    let callee = heap.root_at(vbase + 1);
    let call_form = heap.root_at(vbase);
    let env = heap.env_root_at(ebase);
    heap.truncate_roots(vbase);
    heap.truncate_env_roots(ebase);
    Ok((argv, callee, call_form, env))
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
    // `bind_params` selects the arm for `argv`'s arity and returns its body.
    let (scope, body) = bind_params(heap, cl, argv)?;
    if body.is_empty() {
        return Ok(Value::Nil);
    }
    // Each body-form eval can collect at ANY depth (ADR-061), so keep `scope` and
    // the remaining body forms on the operand stack across them (the intermediate
    // `result`s are dead the moment they're overwritten, so they need no slot).
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_env_root(scope);
    for &form in &body {
        heap.push_root(form);
    }
    let n = body.len();
    let mut result = Value::Nil;
    for i in 0..n {
        let scope_now = heap.env_root_at(eb);
        let form = heap.root_at(vb + i);
        // Same as the closure body branch in `eval`: tag the body form's
        // position on any error so the diagnostic points at the failing line.
        match eval(heap, form, scope_now).map_err(|e| e.or_form_pos(heap, form)) {
            Ok(v) => result = v,
            Err(e) => {
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Err(e);
            }
        }
    }
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    Ok(result)
}

/// Select the closure's arm for this call's arity, bind its parameters into a
/// fresh scope, and return `(scope, that arm's body)`. Dispatching by argument
/// count here — and binding each fixed arm's params *directly* — is what makes a
/// multi-arity function's common small-arity call cheap: no rest-list, no
/// `match*`, just one env frame (see [`Closure::select_arm`] / `ClosureArm`).
fn bind_params(
    heap: &mut Heap,
    cl: ClosureId,
    argv: &[Value],
) -> Result<(EnvId, SmallVec<[Value; 4]>), LispError> {
    // Snapshot the selected arm's metadata once. Every re-read of `heap.closure`
    // is a region-dispatch + slab index, and the binding loop below would
    // otherwise re-read it once per parameter. Closures are immutable once
    // allocated, so this snapshot stays consistent. `params`/`optionals`/`body`
    // copy into inline `SmallVec`s, so the common (≤4) case pays no heap alloc.
    let mut params: SmallVec<[Symbol; 4]> = SmallVec::new();
    let mut optionals: SmallVec<[(Symbol, Value); 4]> = SmallVec::new();
    let mut body: SmallVec<[Value; 4]> = SmallVec::new();
    let (cl_env_opt, rest_sym) = {
        let cl_data = heap.closure(cl);
        let arm = match cl_data.select_arm(argv.len()) {
            Some(a) => a,
            None => return Err(arity_error_for(cl_data, argv.len())),
        };
        params.extend_from_slice(&arm.params);
        optionals.extend_from_slice(&arm.optionals);
        body.extend_from_slice(&arm.body);
        (cl_data.env, arm.rest)
    };
    // A global-capturing closure (env == None) resolves to this process's global.
    let cl_env = cl_env_opt.unwrap_or_else(|| heap.global());
    let required = params.len();
    let n_opt = optionals.len();

    let scope = heap.new_env(Some(cl_env));
    for (i, &arg) in argv.iter().enumerate().take(required) {
        heap.env_define(scope, params[i], arg);
    }
    // Fast path: no `&optional` params. Binding the rest list (if any) does no
    // eval, so no GC can run — `argv` / `body` stay valid, no rooting needed.
    if n_opt == 0 {
        if let Some(rs) = rest_sym {
            let rest_list = heap.list_from_slice(&argv[required..]);
            heap.env_define(scope, rs, rest_list);
        }
        return Ok((scope, body));
    }
    // `&optional` defaults present: each default's eval can collect at ANY depth
    // (ADR-061). The caller's `argv` slice, `scope`, every default form, and the
    // body forms are LOCAL transients a deep collection would relocate — root them
    // all on the operand stack and read back relocated handles. (Only paid by
    // functions that actually declare `&optional` params.)
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_env_root(scope); // eb
    let argv_base = heap.roots_len();
    for &a in argv {
        heap.push_root(a); // argv_base ..
    }
    let argn = argv.len();
    let opt_base = heap.roots_len();
    for &(_, d) in &optionals {
        heap.push_root(d); // opt_base .. : default forms
    }
    let body_base = heap.roots_len();
    for &f in &body {
        heap.push_root(f); // body_base .. : body forms
    }
    let mut idx = required;
    for j in 0..n_opt {
        let name = optionals[j].0;
        let scope_now = heap.env_root_at(eb);
        if idx < argn {
            let arg = heap.root_at(argv_base + idx);
            heap.env_define(scope_now, name, arg);
            idx += 1;
        } else {
            // Tag the default-form's source position on any error from its
            // evaluation, so a diagnostic from inside an `&optional` default
            // points at the default's line (not at the enclosing top-level
            // form's start).
            let default_form = heap.root_at(opt_base + j);
            let value = match eval(heap, default_form, scope_now)
                .map_err(|e| e.or_form_pos(heap, default_form))
            {
                Ok(v) => v,
                Err(e) => {
                    heap.truncate_roots(vb);
                    heap.truncate_env_roots(eb);
                    return Err(e);
                }
            };
            let scope_now = heap.env_root_at(eb);
            heap.env_define(scope_now, name, value);
        }
    }
    if let Some(rs) = rest_sym {
        let mut rest_items: SmallVec<[Value; 8]> = SmallVec::new();
        for i in idx..argn {
            rest_items.push(heap.root_at(argv_base + i));
        }
        let rest_list = heap.list_from_slice(&rest_items);
        let scope_now = heap.env_root_at(eb);
        heap.env_define(scope_now, rs, rest_list);
    }
    let mut body_r: SmallVec<[Value; 4]> = SmallVec::with_capacity(body.len());
    for i in 0..body.len() {
        body_r.push(heap.root_at(body_base + i));
    }
    let scope_r = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    Ok((scope_r, body_r))
}

/// Build the arity error for a call whose argument count no arm accepts. For a
/// single-arity closure this is the familiar "expected N (to M | at least N)";
/// for a multi-arity one it lists every accepted arity.
fn arity_error_for(cl: &Closure, got: usize) -> LispError {
    let who = cl
        .name
        .map(value::symbol_name)
        .unwrap_or_else(|| "fn".to_string());
    if cl.arms.len() == 1 {
        let arm = &cl.arms[0];
        return LispError::arity(arity_message(&who, arm.min_arity(), arm.max_arity(), got));
    }
    let mut accepted: Vec<String> = cl
        .arms
        .iter()
        .map(|a| match a.max_arity() {
            None => format!("{}+", a.min_arity()),
            Some(m) if m == a.min_arity() => m.to_string(),
            Some(m) => format!("{}-{}", a.min_arity(), m),
        })
        .collect();
    accepted.sort();
    accepted.dedup();
    LispError::arity(format!(
        "{}: no clause accepts {} argument(s) (arities: {})",
        who,
        got,
        accepted.join(", ")
    ))
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
/// Construct an "unbound symbol: …" error, attaching the scheduler-race hint
/// when we're currently executing in a *green* (spawned) process. The hint
/// covers the under-load failure mode `docs/claude-demo-findings.md` flagged
/// — fan-out of ~20+ workers racing prelude lookups so internal names like
/// `acc`/`fold`/`%eq` spuriously look unbound. False positives are
/// tolerable: the hint conditions on "if this fired under fan-out, try
/// `-j 1`," not on every unbound being a race. (`docs/error-codes.md`.)
fn unbound_error(sym: Symbol) -> LispError {
    let e = LispError::unbound(format!("unbound symbol: {}", value::symbol_name(sym)));
    if crate::process::in_green_process() {
        e.with_hint(
            "this fired inside a spawned process — if it happens only under \
             fan-out load, the scheduler may be racing prelude lookups; \
             try -j 1 (or `nest test -j 1`) to bound concurrency",
        )
    } else {
        e
    }
}

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
    // A closure defined at the global (parent-less) scope captures the env
    // symbolically (`None`), so it works in any process; otherwise it captures
    // its specific enclosing scope.
    let captured = if heap.is_global(env) { None } else { Some(env) };

    // Multi-arity? An optional leading docstring, then every remaining form a
    // `(param-list body…)` *arity* clause (pattern clauses were lowered to
    // `match*` by the compile pass, so they never reach here). Each clause
    // becomes a `ClosureArm`, dispatched by argument count at call time.
    let (lead_doc, clause_forms): (Option<Value>, &[Value]) = match parts.first() {
        Some(&Value::Str(_)) if parts.len() > 1 => (Some(parts[0]), &parts[1..]),
        _ => (None, &parts[..]),
    };
    if !clause_forms.is_empty()
        && clause_forms
            .iter()
            .all(|&f| crate::eval::macros::is_arity_clause(heap, f))
    {
        let doc = lead_doc.and_then(|d| match d {
            Value::Str(id) => Some(heap.string(id).to_string()),
            _ => None,
        });
        let mut arms = Vec::with_capacity(clause_forms.len());
        for &clause in clause_forms {
            let cparts = heap.list_to_vec(clause)?;
            let (params, optionals, rest_param) = parse_params(heap, cparts[0])?;
            arms.push(value::ClosureArm {
                params,
                optionals,
                rest: rest_param,
                body: cparts[1..].to_vec(),
            });
        }
        let id = heap.alloc_closure(Closure {
            name,
            arms,
            doc,
            env: captured,
        });
        return Ok(Value::Fn(id));
    }

    // Single-arity: `parts[0]` is the param list, `parts[1..]` the body.
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
    let id = heap.alloc_closure(Closure::single(
        name, params, optionals, rest_param, body, doc, captured,
    ));
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
/// Evaluate `let`/`letrec` binding RHSs in `scope`, defining each `name`. Each
/// RHS eval can collect at ANY depth (ADR-061), so `scope`, the binding forms,
/// and the trailing `body` are kept on the operand stack across the evals;
/// returns the relocated `(scope, body)`. `binds` is name/value-interleaved with
/// all even slots already validated as symbols.
fn bind_sequential(
    heap: &mut Heap,
    binds: &[Value],
    scope: EnvId,
    body: Value,
) -> Result<(EnvId, Value), LispError> {
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_env_root(scope);
    heap.push_root(body); // vb+0
    for &b in binds {
        heap.push_root(b); // vb+1 .. (names + rhs, interleaved)
    }
    let n = binds.len();
    let mut i = 0;
    while i < n {
        let bind_name = as_symbol(heap.root_at(vb + 1 + i))?;
        let rhs = heap.root_at(vb + 1 + i + 1);
        let scope_now = heap.env_root_at(eb);
        let val = match eval(heap, rhs, scope_now).map_err(|e| e.or_form_pos(heap, rhs)) {
            Ok(v) => v,
            Err(e) => {
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Err(e);
            }
        };
        let scope_now = heap.env_root_at(eb);
        heap.env_define(scope_now, bind_name, val);
        i += 2;
    }
    let body_r = heap.root_at(vb);
    let scope_r = heap.env_root_at(eb);
    heap.truncate_roots(vb);
    heap.truncate_env_roots(eb);
    Ok((scope_r, body_r))
}

fn tail_of_cons(
    heap: &mut Heap,
    body: Value,
    env: EnvId,
) -> Result<Option<(Value, EnvId)>, LispError> {
    // Fast peek: empty body, or a single form (no eval → no GC → no rooting).
    match body {
        Value::Nil => return Ok(None),
        Value::Pair(p) => {
            let (form, next) = heap.pair(p);
            if matches!(next, Value::Nil) {
                return Ok(Some((form, env)));
            }
        }
        _ => return Err(LispError::type_err("improper body list")),
    }
    // Multi-form body: each non-last form is evaluated for effect, and an eval can
    // collect at ANY depth (ADR-061). Keep `env` on the env operand stack and the
    // spine cursor on the value operand stack so a deep collection relocates them
    // in place; return the relocated `env` alongside the tail form for the
    // caller's `continue 'tail`.
    let vb = heap.roots_len();
    let eb = heap.env_roots_len();
    heap.push_env_root(env);
    heap.push_root(body); // vb+0 = spine cursor
    loop {
        let cur = heap.root_at(vb);
        match cur {
            Value::Nil => {
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Ok(None);
            }
            Value::Pair(p) => {
                let (form, next) = heap.pair(p);
                if matches!(next, Value::Nil) {
                    let env_r = heap.env_root_at(eb);
                    heap.truncate_roots(vb);
                    heap.truncate_env_roots(eb);
                    return Ok(Some((form, env_r)));
                }
                let env_now = heap.env_root_at(eb);
                if let Err(e) = eval(heap, form, env_now).map_err(|e| e.or_form_pos(heap, form)) {
                    heap.truncate_roots(vb);
                    heap.truncate_env_roots(eb);
                    return Err(e);
                }
                let next = match heap.root_at(vb) {
                    Value::Pair(p2) => heap.pair(p2).1,
                    _ => Value::Nil,
                };
                heap.set_root(vb, next);
            }
            _ => {
                heap.truncate_roots(vb);
                heap.truncate_env_roots(eb);
                return Err(LispError::type_err("improper body list"));
            }
        }
    }
}

/// Arity of a callable value (closure, macro, or native primitive), or `None`
/// for non-callables. Used by the `def` arity-change diagnostic to compare an
/// old binding's shape against a new one's.
fn value_arity(heap: &Heap, v: Value) -> Option<value::Arity> {
    match v {
        Value::Fn(id) | Value::Macro(id) => {
            // Across all arms: the smallest min, and the largest max (unbounded if
            // any arm has `&` rest). A single-arity closure has one arm.
            let c = heap.closure(id);
            let min = c.arms.iter().map(|a| a.min_arity()).min().unwrap_or(0);
            let max = c
                .arms
                .iter()
                .try_fold(0usize, |acc, a| a.max_arity().map(|m| acc.max(m)));
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
