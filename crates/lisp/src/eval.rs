//! The evaluator: a tree-walker with proper tail calls. Now heap-threaded — it
//! takes `&mut Heap` and addresses values by handle / `EnvId`.
//!
//! `macros` (quasiquote + the macroexpand compile pass) lives alongside it here:
//! the two are mutually recursive — the compile pass lowers the `fn`/`let`
//! pattern surfaces the evaluator runs, and the evaluator falls back to it.

pub mod compile; // the compiling-VM execution engine (ADR-076) — gated by BROOD_TIER
pub mod derive; // auto-derived stdlib imports (ADR-227 follow-up)
mod machine; // the tree-walker's frame stack (ADR-416)
pub(crate) use machine::{TwStart, TwSuspended, resume_captured, run_captured};
pub mod macros;

use std::sync::LazyLock;

use smallvec::SmallVec;

use crate::core::heap::{Heap, Root, SymbolMap};
use crate::core::keywords as kw;
use crate::core::registries as reg;
use crate::core::value::{
    self, Closure, ClosureId, ClosureTemplate, EnvId, NativeId, Symbol, Value, ValueRef,
};
use crate::error::{LispError, LispResult};

/// Truthiness: only `nil` and `false` are falsy.
///
/// A **failure is truthy**, deliberately: a falsy failure would let
/// `(or (string/->number p) 0)` silently turn `"x"` into `0` — the "an unable is
/// never defaulted" rule broken by default. Truthy, the same call site fails
/// loudly with a type error naming the failure.
///
/// So a failure behaves like any other value here: to treat one as absent you say
/// so, with `failure?`. Falsiness cannot distinguish "it failed" from "there is
/// nothing here" — `nil` means absence, a failure means it could not be produced,
/// and collapsing them is what this design set out to stop.
pub fn truthy(v: Value) -> bool {
    !matches!(v.unpack(), ValueRef::Nil | ValueRef::Bool(false))
}

/// The evaluator's special forms, as a closed enum. The hot path (every
/// combination) dispatches on the head symbol's interned id (a `u32`) to one of
/// these — or `None` for an ordinary call — then `match`es the *enum* (a jump on
/// a small discriminant) — no per-form string compares, a dense jump table, and
/// no `symbol_name` interner lock or `String` allocation.
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

/// Tree-walker frame bookkeeping for error stack traces. One `eval` invocation
/// is one logical call frame: the `'tail:` loop below may *enter* a closure body
/// (an ordinary call in tail position re-enters the loop without a new Rust
/// frame), and when an error unwinds out of this frame the wrapper appends one
/// `:trace` entry — the closure the frame ended up in, at the call site that
/// first entered one. Tail chains overwrite `name` but keep the first call site,
/// which is exactly the VM's frame-reuse behaviour, so both engines produce the
/// same trace for the same program.
struct TwFrame {
    name: Option<Symbol>,
    call_pos: Option<crate::error::Pos>,
    call_file: Option<std::sync::Arc<str>>,
    /// `Some` while the closure this frame is in is a contract shim. Boxed: every frame on
    /// the tree-walker's stack carries a `TwFrame`, and almost none is in a shim.
    shim: Option<Box<ShimEntry>>,
}

/// What a frame inside a contract shim needs to move the shim's positions to the user's call.
struct ShimEntry {
    /// The source file the shim's own forms carry (`std/contract.blsp`). See
    /// [`contract_shim_file`].
    file: std::sync::Arc<str>,
    /// The call form that entered the shim — the user's call of the contracted function,
    /// which a TAIL entry knows and `call_pos` (the frame's first entry) does not.
    call: Option<(crate::error::Pos, Option<std::sync::Arc<str>>)>,
}

impl TwFrame {
    /// The frame a native-boundary entry into a closure named `name` starts: the name, no
    /// call site (the caller is native code).
    fn boundary(name: Option<Symbol>) -> TwFrame {
        TwFrame {
            name,
            call_pos: None,
            call_file: None,
            shim: None,
        }
    }
}

/// The contract module's name as an interned symbol, for the per-entry shim test.
static CONTRACT_MODULE_SYM: LazyLock<Symbol> =
    LazyLock::new(|| value::intern(crate::eval::compile::CONTRACT_MODULE));

/// A contract shim — any closure authored by `std/contract.blsp` (ADR-385) — is machinery,
/// on this engine as on the VM (`exec_chunk::attach_vm_trace`, KI-184): never named in a
/// trace, never the position an error is reported at. For a shim, the file its own body's
/// forms were read from, so the error path can tell a position the shim's code tagged from
/// one a deeper user frame did; `None` for every other closure (one symbol compare).
fn contract_shim_file(
    heap: &Heap,
    module: Option<Symbol>,
    body: &[Value],
) -> Option<std::sync::Arc<str>> {
    if module != Some(*CONTRACT_MODULE_SYM) {
        return None;
    }
    body.iter()
        .find_map(|&f| heap.form_pos(f).and_then(|(_, file)| file))
}

/// Where an error leaving a shim frame is reported: the call form that entered the shim,
/// else the frame's own first call site.
fn shim_call_site(
    shim_call: Option<(crate::error::Pos, Option<std::sync::Arc<str>>)>,
    call_file: Option<std::sync::Arc<str>>,
    call_pos: Option<crate::error::Pos>,
) -> (Option<std::sync::Arc<str>>, Option<crate::error::Pos>) {
    match shim_call {
        Some((p, file)) => (file, Some(p)),
        None => (call_file, call_pos),
    }
}

/// An error unwinding out of a contract shim whose own forms live in `shim_file`. What the
/// shim's code tagged — the error's position, and the call sites of the frames it entered
/// (the original, entered at the shim's `(%contract-orig …)`) — is the template's line in a
/// file the user never wrote. Both move to `call_file`/`call_pos`: the user's call of the
/// contracted function. From a native boundary there is no such call; the error is left
/// untagged for the frame further out. The shim itself is not named. The VM's
/// `attach_vm_trace` applies the same rule (KI-184) — except after a TAIL call into the shim,
/// where the VM, having reused the frame, names the call that entered the frame the tail
/// chain began in, and this engine names the tail call itself.
fn untag_shim(
    e: &mut LispError,
    shim_file: Option<&str>,
    call_file: Option<std::sync::Arc<str>>,
    call_pos: Option<crate::error::Pos>,
) {
    if e.is_control() || shim_file.is_none() {
        return;
    }
    if e.file.as_deref() == shim_file {
        e.pos = call_pos;
        e.file = call_pos.and(call_file.as_deref().map(str::to_string));
    }
    for t in &mut e.trace {
        if t.file.as_deref() == shim_file {
            t.file = call_file.clone();
            t.pos = call_pos;
        }
    }
}

/// Record that this eval frame entered closure `id` via `call_form`. First entry
/// wins the call site (`form_pos` lookup, once per frame); later (tail) entries
/// only follow the name.
fn record_tw_entry(
    heap: &Heap,
    entered: &mut Option<TwFrame>,
    // The closure's name, captured by the caller *before* binding — whose `&optional`
    // defaults can run code that triggers a RUNTIME compaction and invalidates the
    // closure's `ClosureId`. A `Symbol` is an interned `u32`, GC-
    // stable, so it survives; re-reading `heap.closure(id)` here would deref a stale
    // handle (a use-after-GC crash on the tree-walker, kernel-audit #2's twin).
    name: Option<value::Symbol>,
    shim_file: Option<std::sync::Arc<str>>,
    call_form: Value,
) {
    // Only a shim entry looks its call form up on a tail entry: every other closure keeps
    // the one lookup per frame.
    let shim_call = shim_file.as_ref().and_then(|_| heap.form_pos(call_form));
    match entered {
        Some(fr) => {
            fr.name = name;
            fr.shim = shim_file.map(|file| {
                Box::new(ShimEntry {
                    file,
                    call: shim_call,
                })
            });
        }
        None => {
            let (call_pos, call_file) = match &shim_call {
                Some((p, file)) => (Some(*p), file.clone()),
                None => match heap.form_pos(call_form) {
                    Some((p, file)) => (Some(p), file),
                    None => (None, None),
                },
            };
            *entered = Some(TwFrame {
                name,
                call_pos,
                call_file,
                shim: shim_file.map(|file| {
                    Box::new(ShimEntry {
                        file,
                        call: shim_call,
                    })
                }),
            });
        }
    }
}

pub fn eval(heap: &mut Heap, expr: Value, env: EnvId) -> LispResult {
    // Tree-walked frames can't be captured: while this eval is live, a parking
    // `receive` must block its worker, not capture (see `TreeWalkGuard`).
    let _tw = crate::process::TreeWalkGuard::enter();
    let mut entered: Option<TwFrame> = None;
    eval_tail_loop(heap, expr, env, &mut entered).map_err(|e| close_frame(e, entered))
}

/// An error unwinding out of a tree-walker frame that entered a closure: append the frame's
/// `:trace` entry (innermost first; `push_trace` caps the list and ignores control signals),
/// or, for a contract shim, move what the shim tagged to the user's call instead.
fn close_frame(mut e: LispError, frame: Option<TwFrame>) -> LispError {
    match frame {
        Some(TwFrame {
            shim: Some(shim),
            call_file,
            call_pos,
            ..
        }) => {
            let (file, pos) = shim_call_site(shim.call, call_file, call_pos);
            untag_shim(&mut e, Some(&shim.file), file, pos);
        }
        Some(fr) => e.push_trace(crate::error::TraceFrame {
            name: fr.name.map(value::symbol_name_ref),
            file: fr.call_file,
            pos: fr.call_pos,
        }),
        None => {}
    }
    e
}

fn eval_tail_loop(
    heap: &mut Heap,
    expr: Value,
    env: EnvId,
    entered: &mut Option<TwFrame>,
) -> LispResult {
    // GC-block guard: increments `GC_BLOCK` for the lifetime of this run. The GC safepoint
    // does not gate on this depth (ADR-061: it collects at any depth — every frame roots its
    // transients on the operand stack); the guard feeds only the stack-overflow byte guard
    // below, which keys its base off the outermost run (`gc_block_depth() <= 1`). `Drop`
    // runs on every return path (including `?` and panic).
    let _gc_block = crate::process::GcBlockGuard::enter();

    // Stack-budget guard (ADR-043). A run's own frames live on its heap frame stack
    // (`machine`), so this bites only a run re-entered through native code — `eval`, or a
    // native callback (`apply_closure` carries the same guard) — each of which is real Rust
    // stack. An unbounded recursion through one would overflow the coroutine stack as a
    // SIGSEGV the host can't `catch_unwind`; probing the stack pointer against the budget
    // fails *here* with a clean, catchable error instead. (Bytes, not frame count — see
    // `process::stack_overflow_check` for why.)
    let stack_probe = 0u8;
    if let Some(used) = crate::process::stack_overflow_check(&stack_probe as *const u8 as usize) {
        return Err(stack_depth_error(used).or_form_pos(heap, expr));
    }
    machine::eval_trace(heap, expr);
    machine::Machine::new(expr, env, entered).run(heap)
}

/// A **keyword applied as an accessor** — `(:name person)` ≡ `(get person :name)`,
/// and `(:name person "unknown")` ≡ the 3-argument `get` (ADR-165).
///
/// A keyword is the *only* non-function value Brood lets you call. The exception is
/// paid for by one shape: `(map people :name)`, the accessor-as-a-value that a
/// higher-order op can take — without it every such call has to invent a throwaway
/// binder (`(fn (p) (get p :name))`), which is 67 sites across the workspace and the
/// most-reached-for shape in map-heavy code. Maps, vectors and sets stay
/// non-callable: `(m k)` would be a second spelling of `get`, and a callable
/// vector/set answers by index-or-membership, the ambiguity ADR-156 refused.
///
/// The receivers mirror `get` exactly, because divergence here would be a trap:
/// a **map** looks up by key, a **set** by membership (yielding the element), `nil`
/// is empty, and anything else is a type error *naming the keyword* — which is
/// strictly more specific than `get`'s, since the key is in the message already.
/// An integer-indexed collection is deliberately in that last group: `(:name deps)`
/// where `deps` is a list of maps is the single most likely misuse of this feature
/// (see ADR-164, which made the same shape loud for `get`).
fn apply_keyword(heap: &mut Heap, kw: Value, argv: &[Value]) -> LispResult {
    let shown = crate::syntax::printer::print(heap, kw);
    if argv.is_empty() || argv.len() > 2 {
        return Err(LispError::type_err(format!(
            "{shown}: a keyword accessor takes 1 or 2 arguments, got {}",
            argv.len()
        ))
        .with_hint(format!(
            "`({shown} coll)` is the value at {shown}, `({shown} coll default)`              substitutes `default` when it is absent."
        )));
    }
    let default = argv.get(1).copied().unwrap_or(Value::nil());
    match argv[0].unpack() {
        // A map: the value at the key. A set: the element itself when present, so
        // `(:a s)` agrees with `(contains? s :a)` — the ADR-156 rule.
        // A record (ADR-542): the field — its fields are a map, its id is none of them.
        ValueRef::Map(id) | ValueRef::Record { fields: id, .. } => {
            Ok(heap.map_get(id, kw).unwrap_or(default))
        }
        ValueRef::Set(id) => Ok(heap.map_get(id, kw).map(|_| kw).unwrap_or(default)),
        // `nil` is the empty collection, as everywhere else in the language.
        ValueRef::Nil => Ok(default),
        other => {
            // Same shape as `LispError::wrong_type` — `who: expected …, got tag (val)`
            // — but built by hand so `who` is the keyword the caller wrote.
            let got = crate::syntax::printer::print(heap, other);
            let tag = crate::core::value::tag(other).name();
            Err(LispError::type_err(format!(
                "{shown}: expected a map, record, set or nil to look up in, got {tag} ({got})"
            ))
            .with_hint(format!(
                "a keyword reads a KEY, so the argument has to be keyed. If that is a                  collection OF maps, you want one element of it (or `(map {shown} coll)`                  for all of them); for a nested key use `(map/get-in coll [{shown} …])`."
            )))
        }
    }
}

/// The `cannot call non-function` error for a value in head position, with a hint
/// chosen by what the value *is*. Apart from a keyword accessor (see
/// [`apply_keyword`]) Brood has no callable data — a map/vector/set is not a
/// function — so the reflexes that expect one need naming, exactly as the reader's
/// `#(…)`/`#"…"` hints do. One helper, used by both engines, so the message and hint
/// can't drift apart.
pub(crate) fn not_a_function_error(heap: &Heap, v: Value) -> LispError {
    let shown = crate::syntax::printer::print(heap, v);
    let err = LispError::type_err(format!("cannot call non-function: {}", shown));
    match v.unpack() {
        // (A keyword never lands here — it is callable, ADR-165.)
        // `({:a 1} :a)` — Clojure's map-as-function.
        ValueRef::Map(_) => err.with_hint(
            "a map isn't a function in Brood: look a key up with `(get m k)` \
             (or `(map/get-in m path)` for a nested one).",
        ),
        ValueRef::Set(_) => err
            .with_hint("a set isn't a function in Brood: test membership with `(contains? s x)`."),
        // `([10 20] 1)` — Clojure's vector-as-function.
        ValueRef::Vector(_) => err.with_hint(
            "a vector isn't a function in Brood: index it with `(nth v i)` \
             (or `(get v i)` / `(subvec v a b)`).",
        ),
        // A literal (string / number / bool) in head position almost always means
        // C-style call syntax: `f(x)` reads in Brood as two forms — `f` then `(x)`
        // — so the inner `(x)` tries to call the *value* of `x`.
        ValueRef::Str(_) | ValueRef::Int(_) | ValueRef::Float(_) | ValueRef::Bool(_) => err
            .with_hint(
                "a value can't be called — in Brood the function goes inside the \
                 parens: write (f x), not f(x). (`name(args)` reads as two separate \
                 forms, so the `(args)` tries to call a value.)",
            ),
        _ => err,
    }
}

/// How many tree-walker→VM re-entries may be live at once — see
/// [`Heap::tw_reentry_depth`]. Small on purpose: each level is a real Rust frame, and
/// past the budget the call simply tree-walks (today's behaviour), so a
/// mixed-eligibility mutual tail loop stays flat instead of recursing natively.
pub(crate) const TW_REENTRY_BUDGET: u32 = 32;

/// Route a tree-walked closure application onto the VM when the callee has a
/// VM-eligible arm for this arity. `None` = not routable — tree-walk it as before.
/// Default ON (ADR-318); `BROOD_NO_TW_REENTRY=1` opts out.
///
/// Why: without it, ONE deferred call tree-walks everything beneath it transitively at
/// ~10× cost (`BROOD_DEFER_DBG` names the entry points). A deferred arm still tree-walks
/// its OWN body, but every VM-eligible callee it invokes runs at engine speed.
///
/// Safety ledger:
/// - **PTC**: bounded by [`TW_REENTRY_BUDGET`] — see [`Heap::tw_reentry_depth`].
/// - **Stack**: `vm_apply` fails with the catchable native-stack error at its own guard.
/// - **Capture**: a `receive` inside the routed run dirty-blocks, exactly as it does
///   tree-walked — `capture_top_level` is already false under the `TreeWalkGuard`, and
///   this router runs strictly inside tree-walked contexts.
/// - **Suspend/IC**: `vm_apply` is the same nested-run chokepoint `dispatch` uses
///   (IC-base save/restore, §8.1 suspend re-raise).
fn tw_vm_route(heap: &mut Heap, id: ClosureId, argv: &[Value]) -> Option<LispResult> {
    if compile::tier_ceiling() < compile::Tier::Bytecode
        || !crate::diagnostics::debug_flags::tree_walker_reentry()
        || heap.tw_reentry_depth.get() >= TW_REENTRY_BUDGET
    {
        return None;
    }
    // A passthrough wrapper stays on the tree-walker's own elision path (its caller
    // already unwraps redirects before reaching here); anything without a compiled
    // arm for this arity tree-walks as before.
    if passthrough_arm(heap, id, argv.len()).is_some() {
        return None;
    }
    let arm = compile::compiled_arm_for(heap, id, argv.len())?;
    // The %receive fence, router edition — see `arm_calls_receive`. A receive-bearing
    // callee stays on the tree-walker, whose blocking-receive path is the one these
    // shapes have always run (routing it made the receive a NESTED vm run: KI-88's
    // wedge). Callees that merely CALL something that receives are fine — the receive
    // then executes in the callee's own dispatch context, not this nested run.
    if compile::arm_calls_receive(&arm) {
        return None;
    }
    {
        // `BROOD_ROUTE_DBG`: name each routed callee.
        if crate::diagnostics::debug_flags::route_debug() {
            let name = heap
                .closure(id)
                .name
                .map(value::symbol_name_ref)
                .unwrap_or("<anon>");
            eprintln!("[route] {name} argc={}", argv.len());
        }
    }
    let cenv = heap.closure(id).env.unwrap_or_else(|| heap.global());
    heap.tw_reentry_depth.set(heap.tw_reentry_depth.get() + 1);
    let r = compile::vm_apply(heap, arm, argv, cenv);
    heap.tw_reentry_depth.set(heap.tw_reentry_depth.get() - 1);
    Some(r)
}

pub fn apply(heap: &mut Heap, callee: Value, argv: &[Value], env: EnvId) -> LispResult {
    // No TreeWalkGuard here: `apply` is also the VM dispatch fallback, and its
    // Native branch is a thin shim — `(%receive …)` reached through it is still
    // bytecode-reachable and MUST keep its capture eligibility (clearing here
    // turned every VM receive into a worker-blocking one and killed
    // migration). The guard lives in `apply_closure`/`eval`, the two places
    // frames genuinely become tree-walked.
    match callee.unpack() {
        ValueRef::Native(id) => call_native(heap, id, argv, env),
        ValueRef::Fn(id) => apply_closure(heap, id, argv),
        // A keyword accessor (ADR-165). Handled *here*, in the shared apply, rather
        // than in the compiler: that is what makes a keyword a first-class value the
        // higher-order ops can take — `(map people :name)`, `(apply :name (list p))`
        // — which is the entire reason for the exception. A compile-time rewrite of
        // `(:k m)` would have covered only the syntactic head.
        ValueRef::Keyword(_) => apply_keyword(heap, callee, argv),
        other => {
            // Shared with the evaluator's direct-combination path, so the two engines
            // — and direct vs `apply`-routed calls — never diverge in message or hint.
            Err(not_a_function_error(heap, other))
        }
    }
}

pub fn apply_closure(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> LispResult {
    // Tree-walker→VM router (see `tw_vm_route`): a native-boundary callback (a
    // `map`/`try`/expander invocation) with a VM-eligible arm runs on the engine.
    // Checked BEFORE the TreeWalkGuard: the routed run is a VM run, and capture
    // stays gated by the caller's own context, not by this frame.
    if let Some(r) = tw_vm_route(heap, cl, argv) {
        return r;
    }
    // The closure body runs on the tree-walker: its frames can't be captured,
    // so a `receive` parking inside must block its worker, never capture (see
    // `TreeWalkGuard` — clearing at this boundary rather than in `apply`
    // keeps native calls shimmed through `apply`, e.g. `%receive` itself,
    // capture-eligible).
    let _tw = crate::process::TreeWalkGuard::enter();
    // A native calling back into Brood is real Rust stack, so this entry carries the same
    // byte-budget guard as `eval_tail_loop`.
    let _gc_block = crate::process::GcBlockGuard::enter();
    let stack_probe = 0u8;
    if let Some(used) = crate::process::stack_overflow_check(&stack_probe as *const u8 as usize) {
        let name = heap.closure(cl).name;
        return Err(close_frame(
            stack_depth_error(used),
            Some(TwFrame::boundary(name)),
        ));
    }
    // Error-trace bookkeeping: this native-boundary entry into `cl` (a `map`/`try`/
    // macro-expansion callback) is one logical frame, mirrored on the VM by a
    // `vm_apply` driver whose arm0 is `cl`. The call site is native code, so the
    // frame's entry carries the name only, and a tail call out of `cl` renames the
    // entry rather than adding one — the VM's frame-reuse behaviour exactly. The whole
    // body, `&optional` defaults included, runs on one machine (`Machine::run_closure`).
    let mut entered: Option<TwFrame> = None;
    heap.root_scope(|heap| machine::Machine::run_closure(heap, cl, argv, &mut entered))
        .map_err(|e| close_frame(e, entered))
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
    let cl_ref = heap.closure(cl);
    let arm = cl_ref.select_arm(argc)?;
    arm.passthrough.as_ref().map(|p| (p.head, p.map.clone()))
}

/// What binding a call's arguments leaves for the machine: the closure's new frame, the
/// selected arm's body, and the `&optional` defaults the call did not supply. Each default is
/// evaluated in `scope`, in order, as a frame of its own; `rest` is the rest parameter still
/// to bind after them — to the empty list, since an optional left unsupplied means no argument
/// was left over. With no default owed, the rest parameter is already bound and `rest` is
/// `None`.
struct Bound {
    scope: EnvId,
    body: SmallVec<[Value; 4]>,
    defaults: SmallVec<[(Symbol, Value); 4]>,
    rest: Option<Symbol>,
}

/// Select `cl`'s arm for `argv` and bind it in a fresh frame. Runs nothing, so it cannot
/// collect: `argv` needs no rooting.
fn bind_params(heap: &mut Heap, cl: ClosureId, argv: &[Value]) -> Result<Bound, LispError> {
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

    let scope = heap.new_env(Some(cl_env));
    // The tree-walker's half of the contract boundary (ADR-383): a frame carries its
    // closure's module, so a name this body resolves to a contract shim of that module
    // reads the original (`boundary_resolve`), as a compiled arm's alias reference does.
    crate::builtins::contracts::bind_frame_module(heap, scope, cl);
    for (i, &arg) in argv.iter().enumerate().take(required) {
        heap.env_define(scope, params[i], arg);
    }
    let mut idx = required;
    let mut defaults: SmallVec<[(Symbol, Value); 4]> = SmallVec::new();
    for &(name, default) in &optionals {
        if idx < argv.len() {
            heap.env_define(scope, name, argv[idx]);
            idx += 1;
        } else {
            defaults.push((name, default));
        }
    }
    let mut rest = rest_sym;
    if defaults.is_empty()
        && let Some(rs) = rest.take()
    {
        let rest_list = heap.list_from_slice(&argv[idx..]);
        heap.env_define(scope, rs, rest_list);
    }
    Ok(Bound {
        scope,
        body,
        defaults,
        rest,
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
        return LispError::arity(arity_message(
            &who,
            arm.min_arity(),
            arm.max_arity(),
            got,
            &arm_param_names(arm),
        ));
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
            &nat.params.join(" "),
        )));
    }
    let func = nat.func;
    // Dirty-CPU diagnostic (natives charged in reductions), armed by
    // `BROOD_STALL_MS`: in a green process, time the call, charge long natives
    // against the reduction budget (`scheduler::charge_native`) and name the
    // builtin that held the worker. Gated behind the stall tracer because
    // always-on per-call timing costs heavily on message-heavy workloads
    // while buying almost nothing: the
    // reduction budget already bounds post-native hogging to ~one quantum
    // (~1 ms of Brood work) — the un-preemptible time *inside* a long native
    // is only fixed by the M4 dirty-CPU offload pool. When the tracer is off
    // (the default), this is one TLS bool + one cached-env load.
    if crate::core::heap::stall_threshold_ms().is_some() && crate::process::in_capture_run() {
        let t0 = web_time::Instant::now();
        let r = func(argv, env, heap);
        if let Some(ms) = crate::process::charge_native(t0) {
            eprintln!("[stall] native {} took {}ms", heap.native(id).name, ms);
        }
        r
    } else {
        func(argv, env, heap)
    }
}

/// [`call_native`] from an engine that can make the call a builtin hands back (ADR-505): the
/// builtin may answer [`Control::Redirect`](crate::error::Control::Redirect) instead of
/// calling Brood from Rust, and the caller takes the call with [`Heap::take_redirect`] and
/// makes it in the builtin's place. Every other caller of `call_native` offers nothing, so
/// the builtin keeps its nested call there.
pub(crate) fn call_native_redirectable(
    heap: &mut Heap,
    id: NativeId,
    argv: &[Value],
    env: EnvId,
) -> LispResult {
    let previous = heap.offer_redirect(argv);
    let result = call_native(heap, id, argv, env);
    heap.withdraw_redirect(previous);
    result
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
    // KI-120 diagnostic, under `BROOD_SCOPE_DBG` (the tracer for this whole class): a
    // qualified name whose module is one of this binary's baked-in `std/` modules AND is
    // recorded in `*features*` as loaded, yet is not bound. That is the "recorded loaded,
    // globals missing" state the wrapper's `editor/serve/serve-manager` deaths imply —
    // but it is also what a plain typo on a std name looks like from here, which is why
    // it is not default-on. Printed at the death site, with the pid and isolate scope,
    // because the `process N died:` one-liner that follows carries neither.
    if let Some(slash) = name.rfind('/') {
        let m = &name[..slash];
        if crate::diagnostics::debug_flags::scope_debug()
            && crate::builtins::modules::is_embedded_module(m)
            && crate::builtins::modules::module_is_provided(heap, m)
        {
            eprintln!(
                "[unbound] std module `{m}` is recorded as loaded but `{name}` is not bound — \
                 pid={:?} scope={}",
                crate::process::current_pid(),
                crate::process::self_isolate_scope(),
            );
        }
    }
    // A deliberate rename (ADR-304's ledger) is appended to the MESSAGE, not carried as
    // a hint: a `(try … (catch e (error-message e)))` sees the message only, and the
    // caller most in need of the pointer is exactly the one that swallowed the error.
    // A name `reflect/undef` removed (ADR-391) is said so in the message, for the same
    // reason: the code raising this was compiled while the name was bound, and the fix —
    // re-evaluating that code so its references resolve afresh — is not guessable from a
    // bare "unbound".
    let removed = if heap.was_undefined(sym) {
        " (removed by reflect/undef; code compiled while it was bound still names it — \
         re-evaluate that code to resolve the name afresh)"
    } else {
        ""
    };
    let e = LispError::unbound(format!(
        "unbound symbol: {}{}{}",
        name,
        crate::renames::rename_hint(&name).unwrap_or_default(),
        removed
    ));
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
            "Brood has no `loop`/`recur` — it has proper tail calls, so recursion is \
             just calling a name. For a local loop use `letrec`: `(letrec (go (fn (i) \
             … (go …))) (go 0))`; or call a `defn` by its own name. A tail self-call \
             is O(1) stack."
        }
        "transient" | "persistent!" | "conj!" | "assoc!" | "disj!" | "pop!" => {
            "Brood collections are persistent and immutable — there are no \
             transients; `conj`/`disj`/`assoc`/`dissoc`/`into` return fresh values."
        }
        // Polymorphism/type constructs from other languages → point at `ability`
        // (`defprotocol`/`defimpl` were retired in its favour). `deftype` is NOT here: it
        // is Brood's own form since ADR-327 — a structural type alias for the checker,
        // not Clojure's nominal class — so a bare `(deftype …)` is a real call.
        "definterface" | "reify" | "defprotocol" | "defimpl" | "defmulti" | "defmethod" => {
            "Brood has no nominal types — for a named, optionally-typed record use \
             `defrecord` (sugar over a plain map, so records stay structural). For \
             polymorphism, `defability`/`impl`/`defrecord` are built in (core, no \
             import) — open generic functions dispatching on the first argument's \
             identity (its `type-of` kind, or a `defrecord` record's nominal id), \
             extensible from any module; a driver is just a value you dispatch on. Or \
             dispatch with `match`/`cond`; multi-clause `defn` dispatches on arity or \
             pattern. (`defprotocol`/`defimpl` were retired in favour of `ability`.)"
        }
        "lazy-seq" | "lazy-cat" => {
            "Brood has no `lazy-seq` thunk. `map`/`filter` are EAGER; for a fusing, \
             single-pass pipeline use the lazy seq-view combinators `seq/lmap`/`seq/lfilter`/\
             `seq/lkeep`/`seq/lremove` threaded with `->` (ADR-111), and `(range n)` is \
             already a lazy O(1) value. For an unbounded/streaming source, a process that \
             `send`s values."
        }
        // Deleted by ADR-308: every collection function takes its collection first, so one
        // pipe threads them all.
        "->>" | "some->>" | "cond->>" => {
            "Brood has one pipe, `->`: every collection function takes its collection \
             first (ADR-308), so the threaded value is always the first argument — \
             `(-> xs (map f) (seq/filter even?))`. Where a step's subject is not first, \
             `$` places the value: `(-> n (- 100 $))`. `cond->` is the conditional pipe."
        }
        "progn" | "begin" => "Brood spells `progn`/`begin` as `do`.",
        "mapcar" => "Brood spells `mapcar` as `map`.",
        "null?" => "Brood tests nil with `nil?` (and `empty?` for an empty collection).",
        "eq?" | "eql" | "equalp" => "Brood compares with `=` (structural equality).",
        "defun" => "Brood spells `defun` as `defn`.",
        "defvar" | "defparameter" => {
            "Use `def` for a global, or `defdyn` for a dynamic var (rebindable with \
             `binding`)."
        }
        // `let` is sequential, so a `let`-bound `fn` can't call itself — which is the
        // whole reason `letfn` exists. Point at `letrec`, which can.
        // Retired in ADR-162: `fn` is the only spelling (the alias had zero non-test
        // uses in the tree, and `docs/language.md` had claimed it was gone for months).
        "lambda" => "Brood spells `lambda` as `fn`: `(fn (x) …)`.",
        "letfn" => {
            "Brood has no `letfn` — bind local functions with `letrec`, whose names \
             are all visible in every right-hand side (so they may recurse and call \
             each other): `(letrec (go (fn (n) … (go …))) (go 0))`. A plain `let` is \
             sequential, so a `let`-bound `fn` cannot call itself."
        }
        "with-meta" | "vary-meta" => "Brood values carry no metadata.",
        // ADR-154 renames + removals. Without these a downstream caller (or an LLM
        // writing from Clojure habit) gets a bare "unbound symbol" for a name that
        // has a direct replacement. `some?` matters most: "any element matches"
        // vs Clojure's non-nil test are both plausible readings, so the error
        // must say which one you want.
        "car" => "Brood spells `car` as `first` (ADR-154 removed the Lisp lineage names).",
        "cdr" => "Brood spells `cdr` as `rest` (ADR-154 removed the Lisp lineage names).",
        "concat" => "Brood spells `concat` as `append` (any number of lists/vectors → a list).",
        "some?" => {
            "Brood has no `some?`. For \"does any element match\" use `(any? pred \
             coll)`; for Clojure's non-nil test write `(not (nil? x))`. (`some?` was \
             freed deliberately — it used to mean the former and reads as the latter.)"
        }
        "length" => "Brood spells `length` as `count` (works on any collection, plus strings).",
        "entries" => "Brood spells `entries` as `%map-pairs` (a map's [k v] pairs as a list).",
        "flat-map" => "Brood spells `flat-map` as `mapcat`.",
        "string-contains?" => {
            "Brood spells `string-contains?` as `includes?` (strings and sequences)."
        }
        "string-index-of" => {
            "Brood spells `string-index-of` as `index-of` (with an `&optional from`)."
        }
        "string-upcase" => "Brood spells `string-upcase` as `string/upper`.",
        "string-downcase" => "Brood spells `string-downcase` as `string/lower`.",
        "string-capitalize" => "Brood spells `string-capitalize` as `string/capitalize`.",
        "read-file" => "Brood spells `read-file` as `file/slurp` (`file/slurp-bytes` for bytes).",
        "write-file" => "Brood spells `write-file` as `file/spit` (`file/spit-append` to append).",
        "path-exists?" => "Brood spells `path-exists?` as `file/exists?`.",
        "working-dir" => "Brood spells `working-dir` as `file/cwd`.",
        // The `#` arm fires when `#` itself is read as a bare symbol — i.e. for a
        // reader macro Brood lacks. `#{…}` sets (ADR-060) and `#b"…"` bytes
        // (ADR-139) are real literals and must not be listed as missing.
        "#" => {
            "Brood has only two `#` literals — `#{…}` (a set) and `#b\"…\"` (bytes). \
             It has no `#(…)` lambda shorthand (write `(fn (x) …)`), no `#_` form \
             discard (use `(comment …)` or `;`), no `#\"…\"` regex literal (`(require \
             'regex)`), and no `#'` var quote (a plain `'foo` is a symbol)."
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
        // A private target (`mod` defined it with `defn-`/`def-`) can't be referred bare by `(:use)`, so
        // never suggest one — consult the recorded privacy fact (ADR-146), not the
        // name. (This is name-privacy; the `!m.contains("--")` below is the separate
        // private-*module*-path heuristic.)
        .filter(|&&g| !heap.is_private(g))
        .filter_map(|&g| {
            let spelling = value::symbol_name(g);
            spelling.strip_suffix(&suffix).map(str::to_string)
        })
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
///
/// `proc/interruptible` is never redirected to, under any name: a wrapper over it is
/// entered, so its call is the VM's interrupt region rather than the builtin running its
/// thunk nested (ADR-494 §3). `compute_passthrough` refuses the name at closure creation;
/// this catches an alias bound to the builtin under another name (ADR-505).
pub(crate) fn passthrough_redirect_ok(heap: &Heap, inner: Value) -> Result<bool, LispError> {
    match inner.unpack() {
        ValueRef::Fn(_) => {}
        ValueRef::Native(id) if !heap.native(id).is_interruptible => {}
        _ => return Ok(false),
    }
    // The same tick also honours an untrappable hard `:kill` (checked only on the
    // reduction rollover — `tick_nested`). This site is load-bearing for
    // killability: in a tree-walked loop whose
    // operators are thin wrappers (`>`/`-`/`+` are Brood defns over `%`-prims), the
    // hot path ticks *here*, so a check only at the `'tail:` loop top can be starved —
    // which is how a process spinning inside `eval`/`reflect/eval-string` escaped `(exit pid
    // :kill)` entirely. The raised control signal is untrappable (`%try` re-raises)
    // and the body driver converts it to process death.
    crate::process::tick_nested()?;
    Ok(true)
}

// The three runaway-guard errors both engines (`eval` here and the VM's
// `vm_apply_inner`) raise — one constructor each so their message/code/hint
// stay byte-identical and can't drift. The caller adds engine-specific
// trimmings (`eval` attaches `or_form_pos`; the VM truncates its root stacks).

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
         or raise the budget with BROOD_STACK_BUDGET. NOTE a self-call through \
         `eval` is NOT a tail call however it is written — `(defn go (n) (eval \
         (list 'go (- n 1))))` looks tail-recursive but `eval` is a native \
         function that re-enters the evaluator on the Rust stack, so each round \
         adds a frame. Recurse directly, or drive the loop with a process",
    )
}

/// Native-stack exhaustion at a **nested activation** (`vm_apply`) — the VM's
/// counterpart to [`stack_depth_error`], which guards the tree-walker.
///
/// Same catchable [`STACK_DEPTH_EXCEEDED`] code, different remedy in the hint:
/// this one is not the `BROOD_STACK_BUDGET` byte budget (that guard measures
/// bytes since the outermost `eval`, and a VM re-entry never reaches `eval`), it
/// is real Rust stack running out. Raising the budget cannot help; restructuring
/// the recursion can.
pub(crate) fn native_stack_error(remaining: usize) -> LispError {
    LispError::runtime(format!(
        "recursion too deep: {remaining} bytes of native stack left, under the \
         {}-byte reserve — non-tail recursion through a `try` body, an `&optional` \
         default, or a native callback re-enters the VM on the Rust stack",
        crate::process::NATIVE_STACK_MARGIN_BYTES
    ))
    .with_code(crate::error::error_codes::STACK_DEPTH_EXCEEDED)
    .with_hint(
        "rewrite as a tail-recursive loop (proper tail calls are O(1) stack), or move the \
         recursion out of the `try` body — a plain VM call uses heap frames, not the \
         native stack, and recurses far deeper. `eval` counts as a native callback here: \
         a self-call made THROUGH `eval` is not a tail call however it is written",
    )
}

/// "memory limit exceeded …" — the ADR-043 soft-memory backstop.
/// "process heap limit exceeded …" — this process's live heap stayed over its
/// own `(proc/flag :max-heap n)` cap after a collection. Catchable; uncaught
/// it kills just this process.
pub(crate) fn proc_memory_limit_error(live: usize, limit: usize) -> LispError {
    LispError::runtime(format!(
        "process heap limit exceeded: {live} live bytes after collection \
         exceeds this process's {limit}-byte :max-heap limit"
    ))
    .with_code(crate::error::error_codes::PROC_MEMORY_LIMIT)
    .with_hint(
        "raise or clear the limit with (proc/flag :max-heap n), \
         or hold less live data (stream instead of accumulating)",
    )
}

pub(crate) fn proc_mailbox_limit_error(len: usize, limit: usize) -> LispError {
    LispError::runtime(format!(
        "mailbox limit exceeded: {len} queued messages exceeds this process's \
         :max-mailbox bound of {limit}"
    ))
    .with_code(crate::error::error_codes::PROC_MAILBOX_LIMIT)
    .with_hint(
        "drain the backlog (receive in a loop), raise or clear the bound with \
         (proc/flag :max-mailbox n), or let it crash under a supervisor; \
         senders are never blocked and nothing was dropped",
    )
}

/// The error an interrupt raises at a safepoint (ADR-491) — `{:kind :interrupted …}` to a
/// `catch`. `by_ceiling`: the interrupt window's own ceiling passed, rather than a quit key
/// or `proc/interrupt`.
pub(crate) fn interrupted_error(by_ceiling: bool) -> LispError {
    let message = if by_ceiling {
        "the interrupt window's ceiling passed"
    } else {
        "quit"
    };
    LispError::new(crate::error::ErrorKind::Interrupted, message)
        .with_code(crate::error::error_codes::INTERRUPTED)
}

/// The interrupt poll of a builtin that iterates natively over a large input (ADR-491): the
/// sort's chunks, a string split's parts, a regex's matches, a rope scan's characters, a JSON
/// value's nodes, a diff's edit distance. `tick` once per item; every [`InterruptPoll::EVERY`]
/// items it makes the safepoint's probe — one thread-local borrow and a relaxed load — and
/// raises a due interrupt. Between probes it is a counter decrement, so the loop it sits in
/// does not get measurably slower.
pub(crate) struct InterruptPoll {
    left: u32,
    /// Unit tests only: how many probes this poll has made, and whether each raises as if an
    /// interrupt were due ([`InterruptPoll::raising`]) — so a test can hand a loop's second
    /// phase a pending interrupt without a process, a window or a timing guess.
    #[cfg(test)]
    probes: u32,
    #[cfg(test)]
    raise: bool,
}

impl InterruptPoll {
    /// Items between probes: a few microseconds of the cheapest per-item work, so an
    /// interrupt lands within well under a millisecond of any of these loops.
    pub(crate) const EVERY: u32 = 4096;

    pub(crate) const fn new() -> Self {
        InterruptPoll {
            left: Self::EVERY,
            #[cfg(test)]
            probes: 0,
            #[cfg(test)]
            raise: false,
        }
    }

    /// A poll whose every probe finds a request due, as if `proc/interrupt` had landed just
    /// before the loop it is handed to started.
    #[cfg(test)]
    pub(crate) const fn raising() -> Self {
        let mut poll = Self::new();
        poll.raise = true;
        poll
    }

    /// How many probes this poll has made.
    #[cfg(test)]
    pub(crate) fn probes(&self) -> u32 {
        self.probes
    }

    /// One item.
    #[inline]
    pub(crate) fn tick(&mut self) -> Result<(), LispError> {
        self.advance(1)
    }

    /// `n` items at once — a byte count, a chunk.
    #[inline]
    pub(crate) fn advance(&mut self, n: u32) -> Result<(), LispError> {
        match self.left.checked_sub(n) {
            Some(left) if left > 0 => {
                self.left = left;
                Ok(())
            }
            _ => {
                self.left = Self::EVERY;
                #[cfg(test)]
                {
                    self.probes += 1;
                    if self.raise {
                        return Err(interrupted_error(false));
                    }
                }
                check_interrupt()
            }
        }
    }
}

/// Raise the calling process's due interrupt, if any — the probe every safepoint that can
/// raise makes once per quantum (ADR-491).
#[inline]
pub(crate) fn check_interrupt() -> Result<(), LispError> {
    match crate::process::take_current_interrupt() {
        Some(by_ceiling) => Err(interrupted_error(by_ceiling)),
        None => Ok(()),
    }
}

pub(crate) fn memory_limit_error(used: usize) -> LispError {
    LispError::runtime(format!(
        "memory limit exceeded: {used} bytes allocated process-wide \
         exceeds the {}-byte soft limit (raise or unset BROOD_MEM_LIMIT)",
        crate::core::alloc::soft_limit()
    ))
    .with_code(crate::error::error_codes::MEMORY_LIMIT)
}

/// The exit reason of the process memory pressure stopped (`process/pressure.rs`): it held
/// the most when the runtime crossed its soft limit.
pub(crate) fn memory_holder_error(held: usize, live: usize, limit: usize) -> LispError {
    LispError::runtime(format!(
        "memory limit exceeded: this process held the most memory ({held} bytes) when \
         {live} bytes allocated process-wide crossed the {limit}-byte soft limit"
    ))
    .with_code(crate::error::error_codes::MEMORY_LIMIT)
    .with_hint(
        "hold less (stream instead of accumulating), cap the process with \
         (proc/flag :max-heap n), or raise BROOD_MEM_LIMIT",
    )
}

/// Render an arity error — `"{who}: expected {N | N to M | at least N}
/// argument(s), got {got}"`. The one formatter for both builtins (from their
/// declared [`Arity`](crate::core::value::Arity)) and user closures (from their parameter list): a closure
/// with `min..=max` required/optional params passes `Some(max)`; `& rest` (and a
/// variadic builtin) passes `None`.
/// The arity error text, with the callee's parameter NAMES: `first: expected 1 argument
/// (coll), got 0`. "expected 1 argument, got 0" states the count and nothing else; the
/// names say WHAT is missing, which is the actionable half and was already recorded —
/// every primitive registers them (`first` declares `&["coll"]`) and every closure arm
/// carries them. `params` is pre-rendered and may be empty, in which case the message is
/// exactly what it was before. `types::check::walk` builds the same string for the
/// checker's warning, so the two never disagree in wording again.
pub(crate) fn arity_message(
    who: &str,
    min: usize,
    max: Option<usize>,
    got: usize,
    params: &str,
) -> String {
    let (expected, n) = match max {
        Some(m) if min == m => (min.to_string(), min),
        Some(m) => (format!("{} to {}", min, m), m),
        None => (format!("at least {}", min), min),
    };
    let noun = if n == 1 { "argument" } else { "arguments" };
    if params.is_empty() {
        format!("{}: expected {} {}, got {}", who, expected, noun, got)
    } else {
        format!(
            "{}: expected {} {} ({}), got {}",
            who, expected, noun, params, got
        )
    }
}

/// The parameter list of a closure arm, as the source spells it: `acc x`, `x & more`,
/// `n &optional m`. Used by the arity error so it can name what is missing.
fn arm_param_names(arm: &crate::core::value::ClosureArm) -> String {
    arm.source_param_text()
}

/// Build a closure from a `(fn …)` form's cdr. `module` is the module whose source the
/// form was written in (`Closure::module`, ADR-383): the tree-walker passes
/// [`top_level_module`], a compiled arm the module it was itself compiled under.
pub(crate) fn make_closure(
    heap: &mut Heap,
    name: Option<Symbol>,
    rest: Value,
    env: EnvId,
    module: Option<Symbol>,
) -> LispResult {
    let tpl = parse_closure_template(heap, rest)?;
    Ok(build_closure(heap, name, &tpl, env, module))
}

/// The module a `(fn …)` form the TREE-WALKER evaluates belongs to (ADR-383): the
/// module being loaded, when the form is one of its top-level forms — `env` is the global
/// scope and a `defmodule` file is open — else the module the enclosing tree-walked
/// frame carries (`bind_frame_module`), else unknown. A tree-walked closure BODY runs in
/// a frame, so a closure it builds at run time is never credited to whatever module
/// happens to be loading then (a lazy load can open one mid-execution); unknown means
/// contracted like a stranger's, which only ever checks more.
pub(crate) fn top_level_module(heap: &Heap, env: EnvId) -> Option<Symbol> {
    if heap.is_global(env) {
        heap.compile_ns()
    } else {
        crate::builtins::contracts::frame_module(heap, env)
    }
}

/// The hot closure-creation path: same as [`make_closure`] with `name = None`, but the
/// parsed [`ClosureTemplate`] is memoised per `MakeClosure` site (keyed by the `fn_rest`
/// AST handle) so a closure built in a loop — a `receive` matcher, a per-frame callback —
/// skips re-parsing its param lists and re-walking the (RUNTIME) body cons list on every
/// creation. Each creation is then just the arm clone + env attach. The cache is
/// gen-invalidated (see [`Heap::lookup_closure_template`]); a `fn_rest` with no stable
/// handle key (never happens for a real `(fn …)` — its cdr is a pair) falls back to a
/// plain parse. `module` is the building arm's own (ADR-383): every closure a site builds
/// is credited to it, including the memoised constant one — a site has one module.
pub(crate) fn make_closure_cached(
    heap: &mut Heap,
    rest: Value,
    env: EnvId,
    module: Option<Symbol>,
) -> LispResult {
    // Cache only a RUNTIME `fn_rest` handle: the cache is invalidated by the RUNTIME
    // `gen_version` (see `Heap::lookup_closure_template`), which tracks *only* RUNTIME
    // relocation. A LOCAL handle's slot can be reused for a different object by a minor
    // GC without bumping that version, so a LOCAL (or PRELUDE) key isn't safe to key on —
    // parse it directly. Every VM `MakeClosure` site's `fn_rest` is an immovable RUNTIME
    // sub-form, so this is the fast path in practice.
    let key = match rest.as_pair() {
        Some(p) if p.region() == crate::core::value::RUNTIME => p,
        _ => {
            let tpl = parse_closure_template(heap, rest)?;
            return Ok(build_closure(heap, None, &tpl, env, module));
        }
    };
    // A **capture-free** closure (`env == GLOBAL`: no lexical captures, no self-name — it
    // late-binds globals but captures nothing) is a *constant*, fully determined by its
    // `fn_rest`. Re-building and re-`promote`ing an identical one every evaluation — a `spawn`
    // thunk in a fan-out — piles up RUNTIME garbage the collector must reclaim. So build +
    // promote it **once** to a stable RUNTIME handle and reuse it. Late binding is preserved
    // (globals still resolve live through `EnvId::GLOBAL`); the enclosing form's hot reload
    // changes `fn_rest` → a fresh key, and a RUNTIME relocation bumps `gen_version` → the cache
    // clears.
    //
    // **Promote only on the *second* sighting** (gated on the template already being cached, our
    // "seen before" signal). A literal evaluated once — the overwhelmingly common `(def f (fn
    // …))` right-hand side — then stays a plain LOCAL closure and flows into `def`'s own
    // promote-and-dedup untouched, so an identical redefinition still dedups (no spurious RUNTIME
    // version). Only a literal in a loop pays the one-time promote, on its way to reuse.
    let is_const = env == crate::core::value::EnvId::GLOBAL;
    if is_const && let Some(c) = heap.lookup_const_closure(key) {
        return Ok(c);
    }
    // Promote on the Nth sighting, not the second: a spawned process that evaluates a
    // `receive` twice also sees its matcher twice, and promoting then would grow the
    // append-only RUNTIME region by one entry per short-lived process (the growth shape
    // `runtime-frontier.md` A3 rejects). A genuinely hot literal loop crosses 8 sightings
    // in microseconds and still gets its stable promoted handle.
    const PROMOTE_AFTER_SIGHTINGS: u32 = 8;
    let (closure, promote_now) = if let Some((tpl, seen)) = heap.lookup_closure_template(key) {
        (
            build_closure(heap, None, &tpl, env, module),
            seen >= PROMOTE_AFTER_SIGHTINGS,
        )
    } else {
        let tpl = std::sync::Arc::new(parse_closure_template(heap, rest)?);
        heap.store_closure_template(key, std::sync::Arc::clone(&tpl));
        (build_closure(heap, None, &tpl, env, module), false)
    };
    if is_const && promote_now {
        let promoted = heap.promote(closure);
        heap.store_const_closure(key, promoted);
        return Ok(promoted);
    }
    Ok(closure)
}

/// Instantiate a closure from a parsed [`ClosureTemplate`]: clone its arms/doc, attach
/// the captured environment, and intern. A closure defined at the global (parent-less)
/// scope captures the env symbolically (`None`), so it works in any process; otherwise
/// it captures its specific enclosing scope. The arms already carry their `passthrough`,
/// so this uses [`Heap::alloc_closure_pre`] (no re-analysis).
fn build_closure(
    heap: &mut Heap,
    name: Option<Symbol>,
    tpl: &ClosureTemplate,
    env: EnvId,
    module: Option<Symbol>,
) -> Value {
    let captured = if heap.is_global(env) { None } else { Some(env) };
    let id = heap.alloc_closure_pre(Closure {
        name,
        arms: tpl.arms.clone(),
        doc: tpl.doc.clone(),
        env: captured,
        module,
        identity: crate::core::value::fresh_closure_identity(),
    });
    Value::func(id)
}

/// Parse a `(fn …)` form's cdr (`rest`) into a reusable [`ClosureTemplate`] — the arity
/// arms (each with its `passthrough` analysed) and docstring. A pure function of the
/// (immutable) AST, independent of the capturing environment, so it is memoised per site
/// by [`make_closure_cached`]. Pattern clauses were lowered to `match*` by the compile
/// pass, so only plain *arity* clauses reach here.
fn parse_closure_template(heap: &Heap, rest: Value) -> Result<ClosureTemplate, LispError> {
    let parts = heap.list_to_vec(rest)?;

    // Multi-arity? An optional leading docstring, then every remaining form a
    // `(param-list body…)` *arity* clause. Each clause becomes a `ClosureArm`,
    // dispatched by argument count at call time.
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
        // The compile pass checks this in `lower_fn`; a `fn` built at runtime (a quasiquote,
        // a macro expanded lazily) reaches here without it.
        if let Some(e) = crate::eval::macros::shadowed_clause_error(heap, clause_forms) {
            return Err(e);
        }
        let doc = lead_doc.and_then(|d| match d.unpack() {
            ValueRef::Str(id) => Some(heap.string(id).to_string()),
            _ => None,
        });
        let mut arms = Vec::with_capacity(clause_forms.len());
        for &clause in clause_forms {
            let cparts = heap.list_to_vec(clause)?;
            let (params, optionals, rest_param) = parse_params(heap, cparts[0])?;
            let mut arm = value::ClosureArm {
                params,
                optionals,
                rest: rest_param,
                body: cparts[1..].to_vec(),
                passthrough: None,
            };
            arm.passthrough = heap.compute_passthrough(&arm);
            arms.push(arm);
        }
        return Ok(ClosureTemplate {
            arms: arms.into(),
            doc,
        });
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
    let mut arm = value::ClosureArm {
        params,
        optionals,
        rest: rest_param,
        body,
        passthrough: None,
    };
    arm.passthrough = heap.compute_passthrough(&arm);
    Ok(ClosureTemplate {
        arms: std::sync::Arc::from([arm]),
        doc,
    })
}

/// A parsed parameter list: required params, `&optional` params with their
/// default forms, and an optional `&` rest param.
type ParamSpec = (Vec<Symbol>, Vec<(Symbol, Value)>, Option<Symbol>);

fn parse_params(heap: &Heap, form: Value) -> Result<ParamSpec, LispError> {
    if let ValueRef::Vector(_) = form.unpack() {
        return Err(vector_binding_container_error("fn"));
    }
    let items = heap
        .seq_items(form)
        .map_err(|_| LispError::type_err("parameter list must be a list: (fn (x y) …)"))?;

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
                rest = Some(as_symbol(heap, "& rest parameter", r)?);
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
            required.push(as_symbol(heap, "parameter", items[i])?);
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
                heap,
                "&optional parameter",
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
    // `clone()` above *shares* the arms `Arc` with the original closure. Give the
    // named copy its OWN arms: the shared-arms invariant (a shared arms holds only
    // RUNTIME handles, so the minor-flush skips it) must not be violated by a pair of
    // live LOCAL closures sharing one arms — the original may still be rooted when the
    // next collection runs. Restores the pre-`Arc` deep-copy behaviour; def-time only.
    c.arms = c.arms.iter().cloned().collect();
    let fresh = heap.alloc_closure(c);
    match val.unpack() {
        ValueRef::Macro(_) => Value::macro_(fresh),
        _ => Value::func(fresh),
    }
}

/// A binding position must hold a symbol. `who` names what was being parsed, and the
/// offending value is printed — a bare "expected a symbol" told the reader neither which
/// form was wrong nor what arrived there, which for `(defn (expr) expr)` is the whole
/// question (the parameter list is sitting where the name belongs).
fn as_symbol(heap: &Heap, who: &str, v: Value) -> Result<Symbol, LispError> {
    match v.unpack() {
        ValueRef::Sym(s) => Ok(s),
        _ => Err(LispError::wrong_type(heap, who, "a symbol", v)),
    }
}

fn as_binding_vec(heap: &Heap, who: &str, v: Value) -> Result<Vec<Value>, LispError> {
    if let ValueRef::Vector(_) = v.unpack() {
        return Err(vector_binding_container_error(who));
    }
    heap.seq_items(v).map_err(|_| {
        LispError::type_err(format!(
            "{who}: bindings must be a list: ({who} (a 1 b 2) …)"
        ))
    })
}

/// A **vector** where a binding container belongs — `(let [a 1] …)`,
/// `(fn [x y] …)`. Brood's rule is *lists for code, vectors for data* (ADR-010),
/// and accepting a vector here is what turned the Clojure spellings into silent
/// misreads rather than errors: `(let [[a 1] [b 2]] …)` destructured `[a 1]`
/// against the value `[b 2]` and reported `unbound symbol: b`, and
/// `(defn f ([x] :one) ([x y] :two))` became one 2-parameter function with an
/// empty body. Rejecting the container makes both a diagnostic at the mistake.
pub(crate) fn vector_binding_container_error(who: &str) -> LispError {
    let (what, fixed) = match who {
        "let" => ("bindings", "(let (a 1 b 2) …)"),
        "letrec" => ("bindings", "(letrec (f (fn …) g (fn …)) …)"),
        "binding" => ("bindings", "(binding (*a* 1 *b* 2) …)"),
        _ => ("parameter list", "(fn (x y) …)"),
    };
    LispError::type_err(format!("{who}: {what} must be a list, not a vector")).with_hint(format!(
        "Brood is lists-for-code, vectors-for-data (unlike Clojure): write `{fixed}`. \
         A vector *inside* a binding position is still destructuring — \
         `(let ([x y] p) …)` unpacks a 2-vector."
    ))
}

/// Whether `v` is a literal-atom value — a leaf that can only be a *value*, never
/// a binding pattern or a call. Used to tell Scheme/Clojure's nested binding form
/// `(let ((a 1) (b 2)) …)` (literal value slots) apart from a genuine bare-list
/// destructure `(let ((a b) '(1 2)) …)` (symbol / compound value slots), so the
/// even-length nested-hint can't fire on valid code.
fn is_literal_atom(v: Value) -> bool {
    matches!(
        v.unpack(),
        ValueRef::Int(_)
            | ValueRef::Float(_)
            | ValueRef::Str(_)
            | ValueRef::Keyword(_)
            | ValueRef::Bool(_)
            | ValueRef::Nil
            | ValueRef::Bytes(_)
            | ValueRef::Decimal(_)
    )
}

/// Whether an *even*-length flat binding vec is really Scheme/Clojure's nested
/// form with literal values — every element a `(sym <literal-atom>)` 2-list, e.g.
/// `((a 1) (b 2))`. The literal-atom guard is what makes this safe on the even
/// path (unlike odd-length, which errors regardless): a real bare-list destructure
/// (`((a b) '(1 2))`) has a symbol or compound in every value slot, so it is never
/// flagged.
pub(crate) fn even_bindings_look_scheme(heap: &Heap, binds: &[Value]) -> bool {
    !binds.is_empty()
        && binds.iter().all(|&b| {
            matches!(b.unpack(), ValueRef::Pair(_))
                && heap
                    .seq_items(b)
                    .map(|it| {
                        it.len() == 2
                            && matches!(it[0].unpack(), ValueRef::Sym(_))
                            && is_literal_atom(it[1])
                    })
                    .unwrap_or(false)
        })
}

/// The `let`/`letrec` bindings error, with a teaching hint when the bindings look
/// like Scheme/Clojure's **nested-pair** form — `(let ((a 1) (b 2)) …)` instead of
/// Brood's flat `(let (a 1 b 2) …)`. The hint fires when every binding element is a
/// `(name value)` 2-list. Reached two ways: the odd-length guard (which errors
/// regardless) calls this directly, and the compile pass / tree-walker route the
/// *even*-length literal-valued shape here via [`even_bindings_look_scheme`]
/// (LLM-native errors, `docs/llm-native.md`).
pub(crate) fn scheme_binding_error(heap: &Heap, who: &str, binds: &[Value]) -> LispError {
    let looks_nested = !binds.is_empty()
        && binds.iter().all(|&b| {
            matches!(b.unpack(), ValueRef::Pair(_))
                && heap
                    .seq_items(b)
                    .map(|it| it.len() == 2 && matches!(it[0].unpack(), ValueRef::Sym(_)))
                    .unwrap_or(false)
        });
    let e = LispError::runtime(format!("{who}: bindings must be name/value pairs"));
    if looks_nested {
        e.with_hint(format!(
            "Brood `{who}` bindings are FLAT, not nested like Scheme/Clojure: \
             write `({who} (a 1 b 2) …)`, not `({who} ((a 1) (b 2)) …)`."
        ))
    } else {
        e
    }
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

/// Whether `def` should report a hot-reload arity/macro change on stderr.
/// `BROOD_NO_RELOAD_DIAG=1` silences it, for a tool that rebinds globals
/// *deliberately* and en masse — `nest test --cover` wraps every project function
/// in a variadic counting shim, which legitimately changes every arity, so the
/// diagnostic would be hundreds of lines of expected noise. Off-switch only: the
/// default stays on, so an accidental reload mismatch is still surfaced.
fn reload_diagnostics_enabled(heap: &Heap, root: EnvId) -> bool {
    static ENV_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let env_on = !crate::diagnostics::debug_flags::is_armed(&ENV_ON, "BROOD_NO_RELOAD_DIAG");
    if !env_on {
        return false;
    }
    // The in-language switch. `def` is not a hot path, so one global lookup here is
    // free, and it lets Brood code that rebinds globals *deliberately and en masse*
    // — coverage instrumentation, a reload tool, a test exercising the diagnostic —
    // turn the chatter off for its own scope without an env var (there is no
    // `setenv` primitive, and the env read above is cached process-wide anyway).
    // Unbound means on, so the default is unchanged.
    let name = value::intern(reg::RELOAD_DIAGNOSTICS);
    match heap.env_get(root, name) {
        Some(v) => truthy(v),
        None => true,
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
