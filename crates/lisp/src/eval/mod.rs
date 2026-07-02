//! The evaluator: a tree-walker with proper tail calls. Now heap-threaded — it
//! takes `&mut Heap` and addresses values by handle / `EnvId`.
//!
//! `macros` (quasiquote + the macroexpand compile pass) lives alongside it here:
//! the two are mutually recursive — the compile pass lowers the `fn`/`let`
//! pattern surfaces the evaluator runs, and the evaluator falls back to it.

pub mod compile; // the compiling-VM execution engine (ADR-076) — gated by BROOD_VM
pub mod macros;

use std::sync::LazyLock;

use smallvec::SmallVec;

use crate::core::heap::{Heap, Root, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Closure, ClosureId, EnvId, NativeId, Symbol, Value, ValueRef};
use crate::error::{LispError, LispResult};

/// Truthiness: only `nil` and `false` are falsy.
pub fn truthy(v: Value) -> bool {
    !matches!(v.unpack(), ValueRef::Nil | ValueRef::Bool(false))
}

/// Evaluate `form` and attach its source position to any error.
/// Collapses the recurring `eval(heap, form, env).map_err(|e| e.or_form_pos(heap, form))`.
#[inline]
fn eval_at(heap: &mut Heap, form: Value, env: EnvId) -> LispResult {
    eval(heap, form, env).map_err(|e| e.or_form_pos(heap, form))
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
    Fn,
    Quasiquote,
    Let, // sequential binding
    Letrec,
}

/// Spelling → form. This is deliberately the evaluator-*core* subset;
/// `builtins.rs::SPECIAL_FORMS` is the broader, LSP-facing list (it also names
/// the macro keywords).
const SPECIAL_SPELLINGS: &[(&str, SpecialForm)] = &[
    (kw::QUOTE, SpecialForm::Quote),
    (kw::IF, SpecialForm::If),
    (kw::DO, SpecialForm::Do),
    (kw::DEF, SpecialForm::Def),
    (kw::FN, SpecialForm::Fn),
    // `lambda` is an exact synonym for `fn` (the macroexpand pass canonicalises it, so it
    // reaches here only on a raw/un-expanded eval path — e.g. a quasiquote-built or
    // `(eval '(lambda …))` form).
    (kw::LAMBDA, SpecialForm::Fn),
    (kw::QUASIQUOTE, SpecialForm::Quasiquote),
    (kw::LET, SpecialForm::Let),
    (kw::LETREC, SpecialForm::Letrec),
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

/// Is `s` an evaluator-core special form (`if`/`let`/`fn`/…)? Exposed so
/// [`Heap::alloc_closure`](crate::core::heap::Heap::alloc_closure) can exclude a
/// special-form head when precomputing a thin-wrapper [`Passthrough`](crate::core::value::Passthrough)
/// — a special form isn't a callable value, so it can't be redirected to.
#[inline]
pub(crate) fn is_special_form(s: Symbol) -> bool {
    SPECIAL_IDS.contains_key(&s)
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
        return Err(stack_depth_error(used).or_form_pos(heap, expr));
    }

    #[cfg(debug_assertions)]
    {
        static EVAL_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *EVAL_TRACE.get_or_init(|| {
            std::env::var("BROOD_EVAL_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
        }) {
            eprintln!("[eval-trace] {}", crate::syntax::printer::print(heap, expr));
        }
    }

    'tail: loop {
        match expr.unpack() {
            ValueRef::Sym(s) => {
                #[cfg(debug_assertions)]
                heap.debug_walk_env_chain(env, s);
                let expr_sym = expr;
                return heap
                    .env_get(env, s)
                    .ok_or_else(|| unbound_error(heap, s))
                    .map_err(|e| e.or_form_pos(heap, expr_sym));
            }
            ValueRef::Vector(id) => {
                // A vector literal evaluates each element. Those evals can collect
                // at ANY depth (ADR-061), so keep both the unevaluated elements and
                // the accumulated results on the operand stack across them.
                let items = heap.vector(id).to_vec();
                let n = items.len();
                return heap.root_scope(|heap| {
                    let env_r = heap.root_env(env);
                    let src: SmallVec<[Root; 8]> = items.iter().map(|&it| heap.root(it)).collect();
                    let mut out_r: SmallVec<[Root; 8]> = SmallVec::with_capacity(n);
                    for &ir in &src {
                        let env_now = heap.read_root_env(env_r);
                        let item = heap.read_root(ir);
                        let v = eval(heap, item, env_now)?;
                        out_r.push(heap.root(v));
                    }
                    let out: Vec<Value> = out_r.iter().map(|&r| heap.read_root(r)).collect();
                    Ok(heap.alloc_vector(out))
                });
            }
            ValueRef::Map(id) => {
                // A map literal evaluates each key and value, then canonicalises
                // (last-wins on equal keys). Like a vector literal, but in pairs —
                // and likewise operand-stack-rooted so a deep collection during an
                // element eval can't dangle the source forms or accumulated pairs.
                let entries = heap.map_entries(id);
                let n = entries.len();
                return heap.root_scope(|heap| {
                    let env_r = heap.root_env(env);
                    let src: SmallVec<[(Root, Root); 8]> = entries
                        .iter()
                        .map(|&(k, v)| (heap.root(k), heap.root(v)))
                        .collect();
                    let mut res: SmallVec<[(Root, Root); 8]> = SmallVec::with_capacity(n);
                    for &(kr, vr) in &src {
                        let env_now = heap.read_root_env(env_r);
                        let kf = heap.read_root(kr);
                        let kv_val = eval(heap, kf, env_now)?;
                        let kv = heap.root(kv_val);
                        let env_now = heap.read_root_env(env_r);
                        let vf = heap.read_root(vr);
                        let vv = eval(heap, vf, env_now)?;
                        res.push((kv, heap.root(vv)));
                    }
                    let pairs: Vec<(Value, Value)> = res
                        .iter()
                        .map(|&(k, v)| (heap.read_root(k), heap.read_root(v)))
                        .collect();
                    Ok(heap.map_from_pairs(pairs))
                });
            }
            ValueRef::Pair(_) => {} // combination, handled below
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
        // RUNTIME-region safepoint — auto-compact the shared code region once
        // hot-reload churn crosses the threshold (the shared-code analog of the
        // LOCAL collect above). Same `macro_block_active` gate and same `expr`/
        // `env` rewrite contract: `maybe_runtime_collect` only rewrites RUNTIME
        // handles (never moves LOCAL data), and the rooted set it touches —
        // globals + `roots`/`env_roots`/`dynamics` + both LOCAL gens — is exactly
        // the ADR-061 invariant the LOCAL safepoint already relies on (so the VM's
        // frame slots on `roots` are covered), plus the live `expr`/`env` passed
        // here. Runs only when this heap uniquely owns the runtime (single-process
        // / quiescent); a shared runtime backs off. Cost when not due: a `boxcar`
        // length read + a compare.
        if !crate::process::macro_block_active() && heap.rt_gc_due() {
            let mut roots = [expr];
            let mut envs = [env];
            heap.maybe_runtime_collect(&mut roots, &mut envs);
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
            return Err(memory_limit_error(used).or_form_pos(heap, expr));
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
            return Err(deadline_error().or_form_pos(heap, expr));
        }

        let (head, rest) = match expr.unpack() {
            ValueRef::Pair(p) => heap.pair(p),
            _ => unreachable!(),
        };

        // --- special forms ---
        if let ValueRef::Sym(s) = head.unpack() {
            match special_form(s) {
                Some(SpecialForm::Quote) => {
                    // `(quote x)` returns x literally — but only x; reject
                    // `(quote a b)` rather than silently dropping the tail.
                    let (form, r) = uncons(heap, rest);
                    if !matches!(r.unpack(), ValueRef::Nil) {
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
                    let (test, then_form, else_form, new_env) = heap.root_scope(|heap| {
                        let env_r = heap.root_env(env);
                        let then_r = heap.root(then_form);
                        let else_r = heap.root(else_form);
                        let test = eval_at(heap, test_form, env)?;
                        Ok((
                            test,
                            heap.read_root(then_r),
                            heap.read_root(else_r),
                            heap.read_root_env(env_r),
                        ))
                    })?;
                    env = new_env;
                    expr = if truthy(test) { then_form } else { else_form };
                    continue 'tail;
                }
                Some(SpecialForm::Do) => match tail_of_cons(heap, rest, env)? {
                    Some((last, env_r)) => {
                        expr = last;
                        env = env_r;
                        continue 'tail;
                    }
                    None => return Ok(Value::nil()),
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
                        let (v, new_env) = heap.root_scope(|heap| {
                            let env_r = heap.root_env(env);
                            // Run the RHS on the VM when enabled (it's already
                            // macroexpanded+resolved if this `def` came through the
                            // normal pipeline; `run` falls back to the tree-walker
                            // for anything it can't compile). Without this a
                            // top-level `(def x <expr>)` evaluates `<expr>` entirely
                            // on the tree-walker — `def` is a special form, so the
                            // whole form (RHS included) was deferring. Mirrors
                            // `Interp::eval_str`'s per-form dispatch.
                            let v = if compile::vm_enabled() {
                                compile::run(heap, args[1], env)
                                    .map_err(|e| e.or_form_pos(heap, args[1]))?
                            } else {
                                eval_at(heap, args[1], env)?
                            };
                            Ok((v, heap.read_root_env(env_r)))
                        })?;
                        env = new_env;
                        v
                    } else {
                        Value::nil()
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
                        // Macro-redefinition diagnostic (hot reload): redefining a
                        // macro does *not* re-expand callers already compiled with
                        // the old expansion — they keep the old code until re-eval'd.
                        // `defmacro` is a macro lowering to `(def name (%make-macro …))`,
                        // so this is the def-side home of what used to live in the
                        // `defmacro` special form.
                        if matches!(old.unpack(), ValueRef::Macro(_))
                            && matches!(val.unpack(), ValueRef::Macro(_))
                        {
                            eprintln!(
                                "[reload] macro {} redefined; callers expanded before now keep the old expansion — re-eval them",
                                value::symbol_name(name)
                            );
                        }
                    }
                    heap.env_define(root, name, val);
                    return Ok(Value::symbol(name));
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
                    let template = args.into_iter().next().unwrap_or(Value::nil());
                    // Expand the template into *builder code* (a pure structural
                    // transform — no eval re-entry, so no GC-rooting hazard), then
                    // hand it back to the loop to evaluate. The unquoted sub-forms
                    // become `list`/`append` operands the evaluator roots. Tail
                    // position: the builder code's value is this form's value, and
                    // the loop-top safepoint roots the new `expr`.
                    expr = crate::eval::macros::expand_quasiquote(heap, template)
                        .map_err(|e| e.or_form_pos(heap, expr))?;
                    continue 'tail;
                }
                Some(SpecialForm::Let) => {
                    let (binds_form, body) = uncons(heap, rest);
                    if !matches!(rest.unpack(), ValueRef::Pair(_)) {
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
                        .any(|&b| !matches!(b.unpack(), ValueRef::Sym(_)))
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
                        None => return Ok(Value::nil()),
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
                    if !matches!(rest.unpack(), ValueRef::Pair(_)) {
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
                        .any(|&b| !matches!(b.unpack(), ValueRef::Sym(_)))
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
                        heap.env_define(scope, bind_name, Value::nil());
                        i += 2;
                    }
                    let (scope, body) = bind_sequential(heap, &binds, scope, body)?;
                    match tail_of_cons(heap, body, scope)? {
                        Some((last, env_r)) => {
                            expr = last;
                            env = env_r;
                            continue 'tail;
                        }
                        None => return Ok(Value::nil()),
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

        let callee = match head.unpack() {
            ValueRef::Sym(s) => {
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
                    .ok_or_else(|| unbound_error(heap, s))
                    .map_err(|e| e.or_form_pos(heap, call_form))?;
                if let ValueRef::Macro(mid) = v.unpack() {
                    // Macro expansion can collect at any depth (ADR-061); root
                    // `env` across it so the `continue 'tail` re-reads the
                    // relocated handle. `arg_forms` is consumed by `bind_params`
                    // inside `apply_closure`, which roots it itself.
                    let arg_forms = heap.list_to_vec(spine)?;
                    let (expanded, new_env) = heap.root_scope(|heap| {
                        let env_r = heap.root_env(env);
                        let out = apply_closure(heap, mid, &arg_forms)
                            .map_err(|e| e.or_form_pos(heap, call_form))?;
                        Ok((out, heap.read_root_env(env_r)))
                    })?;
                    env = new_env;
                    expr = expanded;
                    continue 'tail;
                }
                v
            }
            _ => {
                // A computed head (`((f) …)`) is evaluated — and that eval can
                // collect at any depth, so root `call_form` + `env` across it,
                // then re-read the relocated handles and re-derive the spine
                // from the moved `call_form`.
                let (callee, new_call_form, new_env) = heap.root_scope(|heap| {
                    let call_form_r = heap.root(call_form);
                    let env_r = heap.root_env(env);
                    let callee = eval_at(heap, head, env)?;
                    Ok((
                        callee,
                        heap.read_root(call_form_r),
                        heap.read_root_env(env_r),
                    ))
                })?;
                call_form = new_call_form;
                env = new_env;
                spine = match call_form.unpack() {
                    ValueRef::Pair(p) => heap.pair(p).1,
                    _ => Value::nil(),
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
        // Dispatch loop: re-entered when `apply` is unfolded or a thin-wrapper
        // closure (`passthrough_arm`) redirects to its inner call — both rewrite
        // `cur_callee`/`cur_argv` and `continue 'dispatch` rather than recursing.
        'dispatch: loop {
            while let ValueRef::Native(id) = cur_callee.unpack() {
                if heap.native(id).name != "apply" || cur_argv.len() < 2 {
                    break;
                }
                let last = cur_argv.pop().expect("argv non-empty (checked above)");
                let mut real = cur_argv.remove(0);
                // A lazy seq-view as the spliced arg list realises first —
                // `seq_items` can't run its transducer. The realise re-enters `eval`
                // (a GC safepoint that relocates LOCAL handles), so root `real` and
                // the remaining leading args across it and re-read after — otherwise
                // `(apply <local-closure> … <seq-view>)` derefs a stale handle →
                // use-after-GC (ADR-114 re-read discipline; mirrors `realize_seqviews`).
                let last = if matches!(last.unpack(), ValueRef::SeqView(_)) {
                    heap.root_scope(|heap| -> LispResult {
                        let real_r = heap.root(real);
                        let arg_roots: Vec<_> = cur_argv.iter().map(|&v| heap.root(v)).collect();
                        let realized = crate::builtins::realize_seqview(heap, env, last)?;
                        real = heap.read_root(real_r);
                        for (slot, &r) in cur_argv.iter_mut().zip(arg_roots.iter()) {
                            *slot = heap.read_root(r);
                        }
                        Ok(realized)
                    })
                    .map_err(|e| e.or_form_pos(heap, call_form))?
                } else {
                    last
                };
                cur_argv.extend(
                    heap.seq_items(last)
                        .map_err(|e| e.or_form_pos(heap, call_form))?,
                );
                cur_callee = real;
            }

            match cur_callee.unpack() {
                ValueRef::Native(id) => {
                    return call_native(heap, id, &cur_argv, env)
                        .map_err(|e| e.or_form_pos(heap, call_form));
                }
                ValueRef::Fn(id) => {
                    // Thin-wrapper elision: redirect a pure pass-through arm
                    // (`(%add a b)` for `+`, etc.) straight to its inner call on the
                    // already-evaluated args, skipping the redundant frame + bind +
                    // body walk. `eval`ing `head` here is a symbol lookup (no GC, so
                    // `cur_argv` stays valid). Only redirect when it resolves to a
                    // function; anything else falls through to the normal path.
                    if let Some((head, map)) = passthrough_arm(heap, id, cur_argv.len()) {
                        let cl_env = heap.closure(id).env.unwrap_or_else(|| heap.global());
                        // Tree-walker inner-head resolution: a full `eval` (a symbol
                        // lookup — no GC, so `cur_argv` stays valid). The shared
                        // `passthrough_redirect_ok` then gates the redirect (callable
                        // inner only), counts the reduction, and honours the deadline.
                        let inner =
                            eval(heap, head, cl_env).map_err(|e| e.or_form_pos(heap, call_form))?;
                        // A redirect back to the *same* closure is direct self-recursion
                        // (`(defn hog () (hog))`), not a thin wrapper — fall through to
                        // the normal call path (which re-enters the `'tail:` loop, whose
                        // reduction check can preempt) rather than spinning the redirect.
                        let self_cycle =
                            matches!(inner.unpack(), ValueRef::Fn(iid) if iid.0 == id.0);
                        if !self_cycle
                            && passthrough_redirect_ok(inner)
                                .map_err(|e| e.or_form_pos(heap, call_form))?
                        {
                            let mut next_argv: SmallVec<[Value; 8]> =
                                SmallVec::with_capacity(map.len());
                            for &i in &map {
                                next_argv.push(cur_argv[i]);
                            }
                            cur_callee = inner;
                            cur_argv = next_argv;
                            continue 'dispatch;
                        }
                    }
                    // `bind_params` selects the arm matching this call's arity, binds
                    // it, and hands back that arm's body (snapshotted into an inline
                    // `SmallVec` so the loop below doesn't re-dispatch the slab).
                    let (scope, body) = bind_params(heap, id, &cur_argv)
                        .map_err(|e| e.or_form_pos(heap, call_form))?;
                    if body.is_empty() {
                        return Ok(Value::nil());
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
                    let (new_last, new_scope) = heap.root_scope(|heap| {
                        let scope_r = heap.root_env(scope);
                        let last_r = heap.root(*last);
                        let init_r: SmallVec<[Root; 8]> =
                            init.iter().map(|&f| heap.root(f)).collect();
                        for &fr in &init_r {
                            let scope_now = heap.read_root_env(scope_r);
                            let form = heap.read_root(fr);
                            eval_at(heap, form, scope_now)?;
                        }
                        Ok((heap.read_root(last_r), heap.read_root_env(scope_r)))
                    })?;
                    expr = new_last;
                    env = new_scope;
                    continue 'tail;
                }
                other => {
                    let shown = crate::syntax::printer::print(heap, other);
                    let mut err =
                        LispError::type_err(format!("cannot call non-function: {}", shown));
                    // A literal (string / number / keyword / bool) in head position almost
                    // always means C-style call syntax: `f(x)` reads in Brood as two forms
                    // — `f` then `(x)` — so the inner `(x)` tries to call the *value* of
                    // `x`. Nudge toward the Lisp form instead of a bare type error.
                    if matches!(
                        other.unpack(),
                        ValueRef::Str(_)
                            | ValueRef::Int(_)
                            | ValueRef::Float(_)
                            | ValueRef::Bool(_)
                            | ValueRef::Keyword(_)
                    ) {
                        err = err.with_hint(
                            "a value can't be called — in Brood the function goes inside the \
                         parens: write (f x), not f(x). (`name(args)` reads as two separate \
                         forms, so the `(args)` tries to call a value.)",
                        );
                    }
                    return Err(err.or_form_pos(heap, call_form));
                }
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
    // Only genuinely LOCAL operands take an operand-stack slot; when running
    // promoted/RUNTIME code `call_form`/`callee`/`spine` are immovable and stay
    // inline (the region check — ADR-061 perf follow-up). Evaluated args are
    // rooted as they accumulate. Teardown truncates back to the entry depth
    // regardless of how many pushes were skipped.
    heap.root_scope(|heap| {
        let call_form_r = heap.root(call_form);
        let callee_r = heap.root(callee);
        let mut spine_r = heap.root(spine); // the cons-spine cursor, advanced in place
        let env_r = heap.root_env(env);
        let mut args: SmallVec<[Root; 8]> = SmallVec::new();
        loop {
            let cur = heap.read_root(spine_r);
            let form = match cur.unpack() {
                ValueRef::Nil => break,
                ValueRef::Pair(p) => heap.pair(p).0,
                _ => return Err(LispError::type_err("improper argument list in call")),
            };
            let env_now = heap.read_root_env(env_r);
            let v = eval_at(heap, form, env_now)?;
            args.push(heap.root(v));
            // Advance the cursor from the (possibly relocated) handle, not the stale
            // `cur` read before the eval.
            let next = match heap.read_root(spine_r).unpack() {
                ValueRef::Pair(p) => heap.pair(p).1,
                _ => Value::nil(),
            };
            spine_r = heap.advance_root(spine_r, next);
        }
        Ok((
            args.iter().map(|&r| heap.read_root(r)).collect(),
            heap.read_root(callee_r),
            heap.read_root(call_form_r),
            heap.read_root_env(env_r),
        ))
    })
}

pub fn apply(heap: &mut Heap, callee: Value, argv: &[Value], env: EnvId) -> LispResult {
    match callee.unpack() {
        ValueRef::Native(id) => call_native(heap, id, argv, env),
        ValueRef::Fn(id) => apply_closure(heap, id, argv),
        other => {
            // Same wording as the evaluator's direct-combination path (the
            // `cannot call non-function` message asserted by suite_test) so the two
            // engines — and direct vs `apply`-routed calls — never diverge.
            let shown = crate::syntax::printer::print(heap, other);
            Err(LispError::type_err(format!(
                "cannot call non-function: {}",
                shown
            )))
        }
    }
}

pub fn apply_closure(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> LispResult {
    // `bind_params` selects the arm for `argv`'s arity and returns its body.
    let (scope, body) = bind_params(heap, cl, argv)?;
    if body.is_empty() {
        return Ok(Value::nil());
    }
    // Each body-form eval can collect at ANY depth (ADR-061), so keep `scope` and
    // the remaining body forms on the operand stack across them (the intermediate
    // `result`s are dead the moment they're overwritten, so they need no slot).
    // Tag each body form's position on any error so the diagnostic points at the
    // failing line (same as the closure body branch in the main eval loop).
    heap.root_scope(|heap| {
        let scope_r = heap.root_env(scope);
        let body_r: SmallVec<[Root; 8]> = body.iter().map(|&f| heap.root(f)).collect();
        let mut result = Value::nil();
        for &fr in &body_r {
            let scope_now = heap.read_root_env(scope_r);
            let form = heap.read_root(fr);
            result = eval_at(heap, form, scope_now)?;
        }
        Ok(result)
    })
}

/// Thin-wrapper elision (perf). If the arm `cl` selects for an `argc`-argument
/// call is a **pure pass-through** — no `&optional`/`& rest`, and a single body
/// form `(head p_i p_j …)` whose arguments are all the arm's *parameters* used as
/// direct arguments — return `(head, map)` where `map[k]` is the `argv` index that
/// the inner call's `k`th argument forwards. The caller can then redirect the call
/// to `head` on the already-evaluated `argv` (remapped), skipping the scope alloc,
/// parameter bind, and body walk.
///
/// This is what makes the prelude operator wrappers cheap — `(+ a b)`'s arm is
/// `(%add a b)`, so `+` redirects straight to `%add` on the same args instead of
/// paying a second full call. The analysis itself is **precomputed once** at
/// closure-allocation time (`Heap::compute_passthrough`) and cached on the arm
/// (`ClosureArm::passthrough`), so this hot-path read is just an arm select plus a
/// field clone — no per-call body walk or param scan. Still hot-reload-safe: it
/// reads the *live* closure, and a redefinition rebuilds the closure (recomputing
/// the analysis). `head` is always an ordinary function reference (special forms
/// and params-as-head are excluded at precompute time), so the caller's
/// resolve-and-redirect is sound.
#[inline]
pub(crate) fn passthrough_arm(
    heap: &Heap,
    cl: ClosureId,
    argc: usize,
) -> Option<(Value, SmallVec<[usize; 4]>)> {
    let arm = heap.closure(cl).select_arm(argc)?;
    arm.passthrough.as_ref().map(|p| (p.head, p.map.clone()))
}

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
    heap.root_scope(|heap| {
        let scope_rt = heap.root_env(scope);
        let argv_r: SmallVec<[Root; 8]> = argv.iter().map(|&a| heap.root(a)).collect();
        let argn = argv.len();
        let opt_r: SmallVec<[Root; 8]> = optionals.iter().map(|&(_, d)| heap.root(d)).collect();
        let body_rt: SmallVec<[Root; 4]> = body.iter().map(|&f| heap.root(f)).collect();
        let mut idx = required;
        for j in 0..n_opt {
            let name = optionals[j].0;
            let scope_now = heap.read_root_env(scope_rt);
            if idx < argn {
                let arg = heap.read_root(argv_r[idx]);
                heap.env_define(scope_now, name, arg);
                idx += 1;
            } else {
                // Tag the default-form's source position on any error so a
                // diagnostic from inside an `&optional` default points at the
                // default's line (not at the enclosing top-level form's start).
                let default_form = heap.read_root(opt_r[j]);
                let value = eval_at(heap, default_form, scope_now)?;
                let scope_now = heap.read_root_env(scope_rt);
                heap.env_define(scope_now, name, value);
            }
        }
        if let Some(rs) = rest_sym {
            let mut rest_items: SmallVec<[Value; 8]> = SmallVec::new();
            for i in idx..argn {
                rest_items.push(heap.read_root(argv_r[i]));
            }
            let rest_list = heap.list_from_slice(&rest_items);
            let scope_now = heap.read_root_env(scope_rt);
            heap.env_define(scope_now, rs, rest_list);
        }
        Ok((
            heap.read_root_env(scope_rt),
            body_rt
                .iter()
                .map(|&r| heap.read_root(r))
                .collect::<SmallVec<_>>(),
        ))
    })
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

/// Construct an "unbound symbol: …" error, attaching the scheduler-race hint
/// when we're currently executing in a *green* (spawned) process. The hint
/// covers the under-load failure mode `docs/claude-demo-findings.md` flagged
/// — fan-out of ~20+ workers racing prelude lookups so internal names like
/// `acc`/`fold`/`%eq` spuriously look unbound. False positives are
/// tolerable: the hint conditions on "if this fired under fan-out, try
/// `-j 1`," not on every unbound being a race. (`docs/error-codes.md`.)
pub(crate) fn unbound_error(heap: &Heap, sym: Symbol) -> LispError {
    let name = value::symbol_name(sym);
    let e = LispError::unbound(format!("unbound symbol: {}", name));
    // A construct an LLM reached for from another Lisp that Brood doesn't have —
    // point at the Brood way (`set!` → process, `loop` → tail recursion, …).
    if let Some(hint) = foreign_construct_hint(&name) {
        return e.with_hint(hint);
    }
    // The post-ADR-065 footgun: a bare name that exists only as `mod/name`
    // because a `(require 'mod)` loaded the module but didn't refer it. Point
    // straight at the `(:use mod)` fix — the single most common mistake for code
    // (and LLMs) written against the old flat-namespace model.
    if let Some(hint) = unbound_namespace_hint(heap, sym) {
        return e.with_hint(hint);
    }
    // The scheduler-race hint is about *bare* prelude/internal names (`fold`,
    // `acc`, `%eq`) spuriously racing under fan-out. A *qualified* miss
    // (`mod/name`) is never that race — it's a genuinely-absent module global,
    // e.g. a `send`-ed closure's free global that exists only on the sending
    // node (late-bound on the receiver, and simply not there). Don't misdirect
    // those with the prelude-race story; `unbound_namespace_hint` already treats
    // a qualified miss as a different problem.
    //
    // AND only when the name is actually a **known global** (`global_defined`): if it
    // resolves in the global table now, this unbound was a *spurious* race; if it's
    // undefined everywhere (a typo / a macro that doesn't exist, e.g. `assert` instead
    // of `is`), the race story is actively misleading (it cost real debugging time
    // during the KI-4 hunt) — give the plain "unbound" error instead.
    if crate::process::in_green_process() && !name.contains('/') && heap.global_defined(sym) {
        e.with_hint(
            "this fired inside a spawned process — if it happens only under \
             fan-out load, the scheduler may be racing prelude lookups; \
             try -j 1 (or `nest test -j 1`) to bound concurrency",
        )
    } else {
        e
    }
}

/// Guidance for a name an LLM reaches for out of Clojure/Scheme/Common-Lisp
/// muscle memory that Brood deliberately doesn't have — surfaced as a hint on
/// both the unbound *error* (runtime) and the advisory checker's unbound
/// *warning* (write-time), so the Brood way is right there, not a doc lookup.
/// Only consulted once a name is known-unbound, so a user who defines one of
/// these sees no hint. Names Brood *does* provide are intentionally absent:
/// it aliases `car`/`cdr`, and has `lambda`/`let*`/`dotimes`/`doseq`/`dolist`/
/// `unless`/`when`/`ref` — those Just Work, so hinting them would be wrong.
pub(crate) fn foreign_construct_hint(name: &str) -> Option<&'static str> {
    Some(match name {
        "set!" | "setq" | "setf" | "set-car!" | "set-cdr!" | "vector-set!" | "string-set!" => {
            "Brood is immutable (ADR-026) — there is no in-place assignment. Hold \
             changing state in a process (spawn/send/receive), or `def` to rebind a \
             global; let/fn bindings never mutate."
        }
        "atom" | "swap!" | "reset!" | "deref" | "volatile!" | "vreset!" | "vswap!" => {
            "Brood has no atoms/cells — mutable state lives in a process \
             (spawn/receive) holding it in its loop, never a mutable value."
        }
        "while" | "until" => {
            "Brood has no `while` — loop with tail recursion (TCO guaranteed, O(1) \
             stack) or, for evolving state, a process (spawn/receive)."
        }
        "loop" | "recur" => {
            "Brood has no `loop`/`recur` — write a tail-recursive helper that carries \
             an accumulator; a self-call in tail position is O(1) stack."
        }
        "transient" | "persistent!" | "conj!" | "assoc!" | "disj!" | "pop!" => {
            "Brood collections are persistent and immutable — there are no \
             transients; `conj`/`assoc`/`dissoc`/`into` return fresh values."
        }
        "defrecord" | "deftype" | "definterface" | "reify" => {
            "Brood has no records/types — model data with plain maps. For \
             polymorphism, use `defprotocol`/`defimpl` (the `protocol` module), or \
             dispatch with `match`/`cond`."
        }
        "lazy-seq" | "lazy-cat" => {
            "Brood sequences are eager — use `map`/`filter`/`fold`; for streaming, a \
             process that `send`s values."
        }
        "case" | "condp" => "Brood has no `case`/`condp` — use `match` (patterns) or `cond`.",
        "progn" | "begin" => "Brood spells `progn`/`begin` as `do`.",
        "mapcar" => "Brood spells `mapcar` as `map`.",
        "null?" => "Brood tests nil with `nil?` (and `empty?` for an empty collection).",
        "eq?" | "eql" | "equalp" => "Brood compares with `=` (structural equality).",
        "defun" => "Brood spells `defun` as `defn`.",
        "defvar" | "defparameter" => {
            "Use `def` for a global, or `defdyn` for a dynamic var (rebindable with \
             `binding`)."
        }
        "letfn" => {
            "Brood has no `letfn` — bind local functions with `let` + `fn`, or define \
             them top-level with `defn`."
        }
        "with-meta" | "vary-meta" => "Brood values carry no metadata.",
        "#" => {
            "Brood has no `#` reader macros: `#(…)` lambda shorthand → `(fn (x) …)`; \
             `#{…}` set literal → `(set […])` after `(:use set)`."
        }
        _ => return None,
    })
}

/// If a bare unbound `sym` exists in the global table only under a namespace
/// (`mod/sym`), suggest the `(:use mod)` clause that would refer it bare. Runs
/// only on the error path, so the global scan costs nothing in the common case.
fn unbound_namespace_hint(heap: &Heap, sym: Symbol) -> Option<String> {
    let name = value::symbol_name(sym);
    if name.contains('/') {
        return None; // a qualified miss (`mod/foo`) is a different problem
    }
    let suffix = format!("/{}", name);
    let mut mods: Vec<String> = heap
        .global_symbols()
        .iter()
        .filter_map(|&g| {
            let spelling = value::symbol_name(g);
            spelling.strip_suffix(&suffix).map(str::to_string)
        })
        // a `--` name is private and wouldn't be referred by `(:use)` anyway.
        // A hierarchical module (`gui/window`, ADR-085) keeps its `/`: the
        // suffix strip removed only the final `/name`, so whatever remains is
        // the module path to suggest verbatim.
        .filter(|m| !m.is_empty() && !m.contains("--"))
        .collect();
    mods.sort();
    mods.dedup();
    match mods.as_slice() {
        [] => None,
        [m] => Some(format!(
            "`{name}` is defined as `{m}/{name}` — add `(:use {m})` to your \
             `defmodule` header to refer it bare, or call it qualified as `{m}/{name}`"
        )),
        _ => Some(format!(
            "`{name}` is defined in namespaces {list} — add the matching `(:use …)` \
             to your `defmodule` header, or call it qualified (e.g. `{first}/{name}`)",
            list = mods
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", "),
            first = mods[0],
        )),
    }
}

/// Thin-wrapper passthrough redirect, shared by both engines (`eval`'s
/// `'dispatch` loop and the VM's `dispatch`). Given the inner head a closure's
/// `passthrough_arm` resolved to, decide whether to redirect: a redirect is only
/// valid when `inner` is itself callable (`Fn`/`Native`); anything else means the
/// caller falls through to a normal call. When it *is* a redirect, this counts the
/// elided reduction (`tick`) and honours the eval deadline — the watchdog a
/// self-referential passthrough (`(defn ginf () (ginf))`) once escaped, since it
/// loops in `dispatch` and never returns to the `'tail:` top where the deadline is
/// otherwise checked. Returns `Err` if the deadline fired (the caller adds its own
/// `or_form_pos` / root-stack cleanup), `Ok(true)` to redirect, `Ok(false)` to
/// fall through. Keeping this one function means the two engines can't drift on
/// passthrough operator semantics. The inner-head *resolution* (a full `eval` in
/// the tree-walker vs. a direct `env_get` in the VM) and the per-engine argv
/// remap stay at the call sites — they're genuinely engine-specific.
pub(crate) fn passthrough_redirect_ok(inner: Value) -> Result<bool, LispError> {
    if !matches!(inner.unpack(), ValueRef::Fn(_) | ValueRef::Native(_)) {
        return Ok(false);
    }
    crate::process::tick();
    if crate::process::deadline_exceeded() {
        return Err(deadline_error());
    }
    Ok(true)
}

/// The three runaway-guard errors both engines (`eval` here and the VM's
/// `vm_apply_inner`) raise — one constructor each so their message/code/hint
/// stay byte-identical and can't drift. The caller adds engine-specific
/// trimmings (`eval` attaches `or_form_pos`; the VM truncates its root stacks).

/// "recursion too deep …" — the VM's call-frame-count guard (`MAX_BC_FRAMES`) for
/// runaway non-tail recursion. Distinct from [`stack_depth_error`], whose `used` is
/// a byte budget: here the limit is a *frame count*, so the message must not
/// mis-state it as bytes or cite the unrelated byte budget.
pub(crate) fn bc_frame_depth_error(frames: usize) -> LispError {
    LispError::runtime(format!(
        "recursion too deep: exceeded the VM's {frames}-frame non-tail-call \
         limit (runaway non-tail recursion?)",
    ))
    .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
    .with_hint(
        "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), \
         or drive the iteration with a process",
    )
}

/// "recursion too deep …" — the stack-budget guard for runaway non-tail recursion.
pub(crate) fn stack_depth_error(used: usize) -> LispError {
    LispError::runtime(format!(
        "recursion too deep: used {used} bytes of stack, over the \
         {}-byte budget (runaway non-tail recursion?)",
        crate::process::stack_budget()
    ))
    .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
    .with_hint(
        "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), \
         or raise the budget with BROOD_STACK_BUDGET",
    )
}

/// "memory limit exceeded …" — the ADR-043 soft-memory backstop.
pub(crate) fn memory_limit_error(used: usize) -> LispError {
    LispError::runtime(format!(
        "memory limit exceeded: {used} bytes allocated process-wide \
         exceeds the {}-byte soft limit (raise or unset BROOD_MEM_LIMIT)",
        crate::core::alloc::soft_limit()
    ))
    .with_code(crate::error::error_codes::MEMORY_LIMIT)
}

/// "evaluation exceeded its time limit …" — the `nest mcp` deadline watchdog.
pub(crate) fn deadline_error() -> LispError {
    LispError::runtime("evaluation exceeded its time limit (MCP tool watchdog)")
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

pub(crate) fn make_closure(
    heap: &mut Heap,
    name: Option<Symbol>,
    rest: Value,
    env: EnvId,
) -> LispResult {
    let parts = heap.list_to_vec(rest)?;
    // A closure defined at the global (parent-less) scope captures the env
    // symbolically (`None`), so it works in any process; otherwise it captures
    // its specific enclosing scope.
    let captured = if heap.is_global(env) { None } else { Some(env) };

    // Multi-arity? An optional leading docstring, then every remaining form a
    // `(param-list body…)` *arity* clause (pattern clauses were lowered to
    // `match*` by the compile pass, so they never reach here). Each clause
    // becomes a `ClosureArm`, dispatched by argument count at call time.
    let (lead_doc, clause_forms): (Option<Value>, &[Value]) =
        match parts.first().map(|v| v.unpack()) {
            Some(ValueRef::Str(_)) if parts.len() > 1 => (Some(parts[0]), &parts[1..]),
            _ => (None, &parts[..]),
        };
    if !clause_forms.is_empty()
        && clause_forms
            .iter()
            .all(|&f| crate::eval::macros::is_arity_clause(heap, f))
    {
        let doc = lead_doc.and_then(|d| match d.unpack() {
            ValueRef::Str(id) => Some(heap.string(id).to_string()),
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
                passthrough: None, // filled by `alloc_closure`
            });
        }
        let id = heap.alloc_closure(Closure {
            name,
            arms,
            doc,
            env: captured,
        });
        return Ok(Value::func(id));
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
    let doc = match body.first().map(|v| v.unpack()) {
        Some(ValueRef::Str(id)) if body.len() > 1 => {
            let d = heap.string(id).to_string();
            body.remove(0);
            Some(d)
        }
        _ => None,
    };
    let id = heap.alloc_closure(Closure::single(
        name, params, optionals, rest_param, body, doc, captured,
    ));
    Ok(Value::func(id))
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
        if let ValueRef::Sym(s) = items[i].unpack() {
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                if in_optional {
                    return Err(LispError::runtime("&optional may appear only once"));
                }
                in_optional = true;
                i += 1;
                continue;
            }
            if value::symbol_is(s, kw::AMP) {
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
    match form.unpack() {
        ValueRef::Sym(s) => Ok((s, Value::nil())),
        ValueRef::Pair(_) | ValueRef::Vector(_) => {
            let parts = heap.seq_items(form)?;
            let name = as_symbol(
                parts
                    .first()
                    .copied()
                    .ok_or_else(|| LispError::runtime("malformed &optional parameter"))?,
            )?;
            let default = parts.get(1).copied().unwrap_or(Value::nil());
            Ok((name, default))
        }
        _ => Err(LispError::type_err("malformed &optional parameter")),
    }
}

/// Attach a name to an anonymous closure when it's `def`'d.
fn name_value(heap: &mut Heap, val: Value, name: Symbol) -> Value {
    // Name both `fn` and `macro` closures (a macro is a closure the expander
    // calls) — `(def m (%make-macro (fn …)))`, which `defmacro` lowers to, must
    // name its macro just as the old `defmacro` special form did.
    let id = match val.unpack() {
        ValueRef::Fn(id) | ValueRef::Macro(id) => id,
        _ => return val,
    };
    if heap.closure(id).name.is_some() {
        return val;
    }
    let mut c = heap.closure(id).clone();
    c.name = Some(name);
    let fresh = heap.alloc_closure(c);
    match val.unpack() {
        ValueRef::Macro(_) => Value::macro_(fresh),
        _ => Value::func(fresh),
    }
}

fn as_symbol(v: Value) -> Result<Symbol, LispError> {
    match v.unpack() {
        ValueRef::Sym(s) => Ok(s),
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
    match v.unpack() {
        ValueRef::Pair(p) => heap.pair(p),
        _ => (Value::nil(), Value::nil()),
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
    heap.root_scope(|heap| {
        let scope_rt = heap.root_env(scope);
        let body_rt = heap.root(body);
        let binds_r: SmallVec<[Root; 8]> = binds.iter().map(|&b| heap.root(b)).collect();
        let n = binds.len();
        let mut i = 0;
        while i < n {
            let bind_name = as_symbol(heap.read_root(binds_r[i]))?;
            let rhs = heap.read_root(binds_r[i + 1]);
            let scope_now = heap.read_root_env(scope_rt);
            let val = eval_at(heap, rhs, scope_now)?;
            let scope_now = heap.read_root_env(scope_rt);
            heap.env_define(scope_now, bind_name, val);
            i += 2;
        }
        Ok((heap.read_root_env(scope_rt), heap.read_root(body_rt)))
    })
}

fn tail_of_cons(
    heap: &mut Heap,
    body: Value,
    env: EnvId,
) -> Result<Option<(Value, EnvId)>, LispError> {
    // Fast peek: empty body, or a single form (no eval → no GC → no rooting).
    match body.unpack() {
        ValueRef::Nil => return Ok(None),
        ValueRef::Pair(p) => {
            let (form, next) = heap.pair(p);
            if matches!(next.unpack(), ValueRef::Nil) {
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
    heap.root_scope(|heap| {
        let env_rt = heap.root_env(env);
        let mut cur_r = heap.root(body); // spine cursor
        loop {
            let cur = heap.read_root(cur_r);
            match cur.unpack() {
                ValueRef::Nil => return Ok(None),
                ValueRef::Pair(p) => {
                    let (form, next) = heap.pair(p);
                    if matches!(next.unpack(), ValueRef::Nil) {
                        return Ok(Some((form, heap.read_root_env(env_rt))));
                    }
                    let env_now = heap.read_root_env(env_rt);
                    eval_at(heap, form, env_now)?;
                    let next = match heap.read_root(cur_r).unpack() {
                        ValueRef::Pair(p2) => heap.pair(p2).1,
                        _ => Value::nil(),
                    };
                    cur_r = heap.advance_root(cur_r, next);
                }
                _ => return Err(LispError::type_err("improper body list")),
            }
        }
    })
}

/// Arity of a callable value (closure, macro, or native primitive), or `None`
/// for non-callables. Used by the `def` arity-change diagnostic to compare an
/// old binding's shape against a new one's.
fn value_arity(heap: &Heap, v: Value) -> Option<value::Arity> {
    match v.unpack() {
        ValueRef::Fn(id) | ValueRef::Macro(id) => {
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
        ValueRef::Native(id) => Some(heap.native(id).arity),
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
