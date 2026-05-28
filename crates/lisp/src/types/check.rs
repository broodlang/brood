//! Step 4: a small **advisory** type checker — the consumer of the `Ty` lattice,
//! so the type system actually *does* something.
//!
//! It walks a macro-expanded form and warns when a call passes an argument that
//! is *provably* the wrong type — its type is **disjoint** from what the callee
//! accepts (`(first 5)`, `(+ 1 "x")`). Disjointness (not subtyping) is the rule,
//! so a superset (`number` where `int` is wanted), an `any` result, or an
//! unknown argument all overlap the expected type and are never flagged — **no
//! false positives**. It never raises and never gates — it returns warnings
//! (contract point #5).
//!
//! ## Where signatures come from (Step 3)
//!
//! Three sources, simplest-first — *no inference engine* (`docs/types.md`):
//!
//! 1. **Primitives** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    a `Sig` ([contract point #6, enforced](../docs/types.md#compatibility-contract))
//!    so the checker just reads it from the global env (see [`primitive_sig`]).
//!    There is no parallel table to maintain.
//! 2. **Curated stdlib** — a small hand-vetted table for the variadic /
//!    `reduce`-based / higher-order Brood closures the checker can't infer but
//!    that matter (`+ - * / < <= > >= mod map filter reduce`; see
//!    [`curated_sig`]). Each is a Brood `defn`, but its sig is pinned by hand.
//! 3. **Basic inference** for a closure whose body is **one straight-line
//!    expression** (a single direct call to a known sig; no `if`/`cond`/`let`/
//!    `match`/recursion). Each closure parameter inherits the type the callee
//!    expects at the position(s) where the parameter is passed; the closure's
//!    return is the callee's. Sound because a straight-line use is
//!    unconditional — no control-flow analysis (see [`infer_sig`]).
//!
//! Argument types in a call come from literals, nested calls with a known
//! return type, and **a context-tracked map of local-variable narrowings**:
//!
//! - A `let`/`let*` binding's RHS contributes its `expr_ty` as the variable's
//!   type (so `(let (x 1) (first x))` flags `first` — `x` is known `int`).
//! - An `if`'s test is matched against the predicate-narrowing table
//!   ([`Ty::tested_by`]). On a `(pred? sym)` test the *then*-branch narrows
//!   `sym` to `tested_by(pred)`, the *else*-branch to its complement; a leading
//!   `(not …)` flips the assertion. Bindings inside a branch override the
//!   narrowing as ordinary shadowing.
//!
//! Vocabulary is `Option<Ty>` (known / unknown), not `GradualTy` — the
//! disjointness check only needs "do I know this type?". Forms inside `try` /
//! `error-of` / `assert-error` are skipped (they deliberately exercise failures).
//!
//! ## Beyond type misuse
//!
//! The walk also emits two non-type diagnostics, sharing the same scope
//! infrastructure:
//!
//! - **Arity**: a call whose argument count isn't admitted by the callee's
//!   declared `Arity` (from [`NativeFn`](crate::core::value::NativeFn) for a
//!   primitive, or from `Closure.{params, optionals, rest}` for a Brood
//!   closure). See [`arity_of`].
//! - **Unbound symbols**: a call head that resolves to nothing — not a
//!   primitive, not a curated stdlib closure, not in local scope (fn/let), not
//!   a file-local def, not a syntactic keyword, and not in the heap's globals.
//!   Driven by [`Ctx::is_local`] (the local + file-global view) plus a
//!   global-env lookup. Scope is honoured: `fn`/`lambda`/`defn`/`defmacro`
//!   bind their params into `Ctx` before walking the body, and
//!   [`check_file`] accumulates top-level `def`/`defn`/`defmacro` / `defdyn`
//!   names across the forms in a file.
//!
//! Not yet (later increments): inference through `cond`/`match`, structured /
//! `and`/`or`-chained guards, recursion, higher-order; running automatically
//! in `brood <file>` / `nest test`. Today the entry points are `brood
//! --check` (CLI; see [`check_file`]) and the `(check 'form)` builtin.

use std::collections::{HashMap, HashSet};

use crate::core::heap::Heap;
use crate::core::value::{self, Arity, Symbol, Tag, Value};
use crate::error::Pos;
use crate::types::{Sig, Ty};

/// Locally-known types for variables in scope — populated by `let`/`let*`
/// bindings and by an enclosing `if`'s guard. Globals are never tracked here
/// (they're redefinable under hot reload — `dynamic()`, not `Any`).
///
/// `Ty::ANY` and "absent" both mean "no useful info"; we keep absent variables
/// out of the map so the printer in tests stays uncluttered.
///
/// **Guard aliases.** When a `let` binds a name to a recognised guard call —
/// `(let (cond (int? x)) (if cond …))` — we also remember that the bound name
/// *is* the result of testing that variable, so the inner `if cond` can
/// narrow `x` (not the bool `cond` itself). The aliasing is sound because
/// Brood is immutable: between the let and the if, neither `x` nor `cond` can
/// change, so the assertion the guard recorded still applies.
#[derive(Clone, Default)]
struct Ctx {
    types: HashMap<Symbol, Ty>,
    /// `bound-name → (variable, type-it-asserts)`: a `let`-stored guard result.
    guards: HashMap<Symbol, (Symbol, Ty)>,
    /// Every locally-bound name in scope — fn/lambda params and let bindings.
    /// Distinct from `types`: a fn-param has *no known type* (`ANY` by default)
    /// but is *in scope*, so it must not be flagged unbound. `types` records
    /// narrowings on top; `locals` records existence.
    locals: HashSet<Symbol>,
    /// Top-level names defined earlier in the same file (`def`/`defn`/
    /// `defmacro` accumulated by [`check_file`]). The file isn't being
    /// evaluated, so these aren't in `heap`'s global table — we track them
    /// here so a later form doesn't flag them as unbound.
    file_globals: HashSet<Symbol>,
}

impl Ctx {
    /// The locally-known type for `sym`, or `None` if it isn't tracked.
    fn get(&self, sym: Symbol) -> Option<Ty> {
        self.types.get(&sym).copied()
    }
    /// The guard (variable + asserted type) `sym` was bound to, if any.
    fn guard(&self, sym: Symbol) -> Option<(Symbol, Ty)> {
        self.guards.get(&sym).copied()
    }
    /// Is `sym` in scope here? — a local binder (fn-param or let), a recorded
    /// narrowing or guard alias, or an accumulated file-global. Bindings in the
    /// surrounding heap (prelude, builtins, earlier-defined globals in a real
    /// runtime) are checked separately by the caller — this is the *local*
    /// view only.
    fn is_local(&self, sym: Symbol) -> bool {
        self.locals.contains(&sym)
            || self.types.contains_key(&sym)
            || self.guards.contains_key(&sym)
            || self.file_globals.contains(&sym)
    }
    /// **Narrow** `sym` to the intersection with `ty` (a guard refinement —
    /// the same lexical variable in the same scope getting tighter). The
    /// caller already knows `sym` lives in this scope (e.g. it's a free
    /// variable inside an `if`'s branch); for an unknown one we treat the
    /// prior as `ANY`, so the intersection is just `ty`.
    fn narrow(&self, sym: Symbol, ty: Ty) -> Ctx {
        let mut c = self.clone();
        let prior = c.types.get(&sym).copied().unwrap_or(Ty::ANY);
        c.types.insert(sym, prior.intersect(ty));
        c
    }
    /// **Bind** `sym` to `ty`, overwriting any prior entry — a fresh let-bound
    /// or fn-param variable shadows the outer. `None` clears the type entry so
    /// a shadowing binding of unknown type doesn't keep an outer narrowing
    /// (but the name is still in scope via `locals`, so an unbound check
    /// doesn't fire on it). Always clears any guard-alias entry for `sym`
    /// (a fresh binding doesn't inherit one).
    fn bind(&self, sym: Symbol, ty: Option<Ty>) -> Ctx {
        let mut c = self.clone();
        match ty {
            Some(t) => {
                c.types.insert(sym, t);
            }
            None => {
                c.types.remove(&sym);
            }
        }
        c.locals.insert(sym);
        c.guards.remove(&sym);
        c
    }
    /// Record that `sym` was let-bound to the result of testing `target` for
    /// `ty` — so a later `(if sym then else)` narrows `target` accordingly.
    /// Self-aliasing (`(let (x (int? x)) …)` would shadow the outer `x` the
    /// guard means to narrow) is rejected.
    fn add_guard(&self, sym: Symbol, target: Symbol, ty: Ty) -> Ctx {
        if sym == target {
            return self.clone();
        }
        let mut c = self.clone();
        c.guards.insert(sym, (target, ty));
        c
    }
    /// Record a top-level `(def/defn/defmacro name …)` so subsequent forms in
    /// the same file see `name` as bound (the file isn't being evaluated, so
    /// `name` won't appear in `heap`'s global table). In-place mutation; the
    /// accumulator threads through [`check_file`].
    fn add_file_global(&mut self, sym: Symbol) {
        self.file_globals.insert(sym);
    }
}

/// Names that have *syntactic* meaning but aren't bound values — never flag
/// these as unbound. Mirrors `eval::SPECIAL_NAMES` plus the macros that the
/// reader / un-expanded forms may carry (the CLI's `--check` doesn't
/// macroexpand). `catch` is the carrier-form for `try`'s catcher, not a
/// callable; `&` / `&optional` are parameter-list markers.
fn is_syntactic_keyword(name: &str) -> bool {
    matches!(
        name,
        "quote"
            | "quasiquote"
            | "unquote"
            | "unquote-splicing"
            | "if"
            | "do"
            | "def"
            | "fn"
            | "lambda"
            | "let"
            | "let*"
            | "letrec"
            | "defmacro"
            | "defn"
            | "defdyn"
            | "defmodule"
            | "module-doc"
            | "when"
            | "unless"
            | "cond"
            | "and"
            | "or"
            | "->"
            | "->>"
            | "match"
            | "case"
            | "try"
            | "catch"
            | "throw"
            | "binding"
            | "for"
            | "loop"
            | "recur"
            | "spawn"
            | "&"
            | "&optional"
            | "&rest"
    )
}

/// The signature of a **primitive** named `name` — read from its `NativeFn`
/// (contract point #6, enforced). `None` when no global of that name exists,
/// or when it isn't a primitive (a Brood closure goes through [`curated_sig`]
/// or [`infer_sig`] instead).
///
/// Lookup goes through `heap.global()`, not `EnvId::GLOBAL` directly: in a real
/// runtime that's `EnvId::GLOBAL` (routed to the shared `runtime.globals`
/// table), but in the prelude-builder / test heap it's a *local* env that
/// `builtins::register` populated — `env_get` walks both transparently.
fn primitive_sig(heap: &Heap, name: &str) -> Option<Sig> {
    let sym = value::intern_existing(name)?;
    match heap.env_get(heap.global(), sym)? {
        Value::Native(id) => Some(heap.native(id).sig.clone()),
        _ => None,
    }
}

/// Signatures for the stable stdlib **closures** the checker can't infer but
/// that matter: the arithmetic/comparison kernel (variadic over numbers) and the
/// core higher-order fns. Hand-vetted, so sound — this is what makes `(+ 1 "x")`
/// catchable even though `+` is `(reduce %add 0 xs)`.
fn curated_sig(name: &str) -> Option<Sig> {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let any = Ty::ANY;
    let seq = Ty::LIST.union(Ty::of(Tag::Vector));
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));
    Some(match name {
        // variadic arithmetic: every argument must be a number
        "+" | "-" | "*" | "/" => Sig::variadic(num, num),
        // variadic comparison: numeric args, boolean result
        "<" | "<=" | ">" | ">=" => Sig::variadic(num, Ty::of(Tag::Bool)),
        // `mod` is Brood (over `rem`), but its types are fixed
        "mod" => Sig::new(vec![int, int], int),
        // higher-order: first arg callable, second a sequence
        "map" | "filter" => Sig::new(vec![callable, seq], seq),
        "reduce" => Sig::new(vec![callable, any, seq], any),
        _ => return None,
    })
}

/// Inferred signature for a **user closure** named `sym` whose body is one
/// straight-line expression — a single call to a callee with a known
/// primitive/curated sig. Each closure parameter inherits the type the callee
/// expects at the position(s) where the parameter is passed directly; the
/// closure's return is the callee's.
///
/// Deliberately *narrow*. Skipped when:
/// - the body isn't exactly one expression (branches, lets, multi-step bodies);
/// - the closure takes `&optional` / rest params (the call's positional arity is
///   already past where the simple rule pays off);
/// - the body isn't a call (a lone literal/variable doesn't pay for itself);
/// - the head is anything but a primitive or curated stdlib closure (in
///   particular, the closure's own name → recursion is ignored, per the rule).
///
/// Sound because a straight-line use is unconditional — no false positives.
fn infer_sig(heap: &Heap, name: &str) -> Option<Sig> {
    let sym = value::intern_existing(name)?;
    let Value::Fn(cid) = heap.env_get(heap.global(), sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    if closure.body.len() != 1 || !closure.optionals.is_empty() || closure.rest.is_some() {
        return None;
    }
    let body = closure.body[0];
    // Copy out before we ask sig_of (which borrows the heap again).
    let params: Vec<value::Symbol> = closure.params.clone();
    let self_name = closure.name;

    let items = list_items(heap, body)?;
    let Value::Sym(callee) = items.first().copied()? else {
        return None;
    };
    // No recursion — neither direct (the closure calls itself by name) nor
    // through inference (`sig_of` is the *non-inferring* lookup so a chain
    // like `defn a (x) (b x)` / `defn b (x) (a x)` can't loop).
    if self_name == Some(callee) {
        return None;
    }
    let callee_name = value::symbol_name(callee);
    let callee_sig = primitive_sig(heap, &callee_name).or_else(|| curated_sig(&callee_name))?;

    // Each closure parameter takes the type the callee expects where the
    // parameter is used. Multiple positions → intersect (the param must satisfy
    // every use). Unmentioned parameters stay `ANY`.
    let mut param_tys = vec![Ty::ANY; params.len()];
    for (i, &arg) in items[1..].iter().enumerate() {
        let Value::Sym(arg_sym) = arg else { continue };
        let Some(pos) = params.iter().position(|&p| p == arg_sym) else {
            continue;
        };
        let Some(expected) = callee_sig.param(i) else {
            continue;
        };
        param_tys[pos] = param_tys[pos].intersect(expected);
    }
    Some(Sig::new(param_tys, callee_sig.ret))
}

/// The signature for `name`, from any of the three sources (primitive → curated
/// → inferred). The non-inferring half is exposed as [`primitive_sig`] +
/// [`curated_sig`] so [`infer_sig`] can consult the callee's sig *without*
/// kicking off another inference (the rule says inference is one step deep).
fn sig_of(heap: &Heap, name: &str) -> Option<Sig> {
    primitive_sig(heap, name)
        .or_else(|| curated_sig(name))
        .or_else(|| infer_sig(heap, name))
}

/// The arity of the callable named `name` — `NativeFn.arity` for primitives,
/// derived from `Closure.{params, optionals, rest}` for Brood closures. `None`
/// when the name resolves to a non-callable, doesn't exist, or no callable is
/// visible (e.g. a file-local `defn` checked in the read-only `--check` path
/// — there's nothing to inspect, so no arity check fires).
///
/// Brood's closure params are: `params.len()` required + `optionals.len()`
/// optional + an optional rest tail (`Symbol`). So min = required, max =
/// required + optional unless there's a rest (then no max).
fn arity_of(heap: &Heap, name: &str) -> Option<Arity> {
    let sym = value::intern_existing(name)?;
    match heap.env_get(heap.global(), sym)? {
        Value::Native(id) => Some(heap.native(id).arity),
        Value::Fn(cid) => {
            let c = heap.closure(cid);
            let min = c.params.len();
            let max = if c.rest.is_some() {
                None
            } else {
                Some(min + c.optionals.len())
            };
            Some(Arity { min, max })
        }
        _ => None,
    }
}

/// A human-readable rendering of an `Arity` for a "wrong number of args"
/// warning — `exact(2)` → "2"; `range(2,3)` → "2 to 3"; `at_least(2)` → "2 or
/// more".
fn arity_str(a: Arity) -> String {
    match a.max {
        Some(m) if m == a.min => a.min.to_string(),
        Some(m) => format!("{} to {}", a.min, m),
        None => format!("{} or more", a.min),
    }
}

/// Does `name` resolve to *any* value in the global env? Broader than
/// `sig_of` / `arity_of` (which only return for callables they know how to
/// describe). A `Value::Macro`, a constant, or anything else that's actually
/// bound counts as "in scope" for the unbound-symbol check — we don't warn
/// just because the checker can't say much about the binding's *shape*.
fn is_globally_bound(heap: &Heap, name: &str) -> bool {
    value::intern_existing(name)
        .and_then(|sym| heap.env_get(heap.global(), sym))
        .is_some()
}

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    match form {
        // A bare symbol is a variable reference — looked up in the local ctx
        // (let-bound RHS / if-guard narrowing). A miss = unknown, not flagged.
        Value::Sym(s) => ctx.get(s),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            match items.first().copied() {
                Some(Value::Sym(s)) => {
                    let head = value::symbol_name(s);
                    if head == "quote" {
                        return items.get(1).map(|&d| Ty::of_value(d));
                    }
                    sig_of(heap, &head).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // Int / Float / Str / Keyword / Bool / Nil / Vector: self-evaluating.
        other => Some(Ty::of_value(other)),
    }
}

/// Check one form, returning a warning per provable misuse. Empty when nothing is
/// provably wrong (which includes "not enough static info").
pub fn check_form(heap: &Heap, form: Value) -> Vec<String> {
    check_located(heap, form)
        .into_iter()
        .map(|(_, msg)| msg)
        .collect()
}

/// Like [`check_form`], but each warning carries the source `Pos` of the call it
/// was found in (when known) — for `file:line:col:` diagnostics from `brood
/// --check` / `nest check`. The position is the *call form*'s, recorded by the
/// reader; an unrecorded form (e.g. one a macro synthesised) yields `None`.
pub fn check_located(heap: &Heap, form: Value) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    check_into(heap, form, &Ctx::default(), &mut out);
    out
}

/// Check a sequence of top-level forms together, threading file-local
/// definitions across them so a `(defn foo …)` at the top isn't flagged when
/// a later form calls `foo`. This is the entry point for `brood --check
/// <file>` / `nest check`.
///
/// Each form is **macro-expanded first** (like the `(check 'form)` builtin),
/// so threading macros (`->`/`->>`), pattern syntax (`match`), test framework
/// wrappers (`test`/`describe`/…), and any user macro that rearranges code
/// are checked against their *expanded* shape — not the surface syntax that
/// would otherwise mistake `(map inc)` inside `(->> xs (map inc))` for a
/// 1-arg call. Source positions survive expansion where the macro rebuilds
/// through `rebuild_list` (the common case); positions on macro-introduced
/// new code are absent.
///
/// File-local def names are accumulated by a **recursive** scan over the
/// expanded forms, so a `(defn foo …)` nested inside a macro body
/// (e.g. inside `(test … (defn foo …) …)`) still shields a later `(foo …)`
/// — `def`s define globally in Brood regardless of nesting position
/// (`docs/language.md`).
///
/// A form whose macroexpansion fails (a malformed macro call) falls back to
/// its un-expanded shape — the eval path will surface the same parse-time
/// error later anyway, so the checker just stays quiet there.
pub fn check_file(heap: &mut Heap, forms: &[Value]) -> Vec<(Option<Pos>, String)> {
    let mut out = Vec::new();
    // Pass 1: macroexpand each form (recording the expanded shape we'll also
    // walk in pass 2). A macroexpand failure isn't this pass's job to report,
    // so we fall back to the un-expanded form silently.
    let root = heap.global();
    let expanded: Vec<Value> = forms
        .iter()
        .map(|&f| crate::eval::macros::macroexpand_all(heap, f, root).unwrap_or(f))
        .collect();
    // Pass 2: collect every `(def name …)` in the expanded tree (top level
    // *or* nested — `defn` inside `test`/`describe`/`when`/… still defines a
    // global once it runs, so the checker honours that). `defmacro` stays a
    // special form (it doesn't expand to `def`), so we match it too.
    let mut ctx = Ctx::default();
    for &form in &expanded {
        collect_def_names(heap, form, &mut ctx);
    }
    // Pass 3: check each expanded form with the accumulated file-globals.
    for &form in &expanded {
        check_into(heap, form, &ctx, &mut out);
    }
    out
}

/// Walk `form` recursively, adding to `ctx.file_globals` every name introduced
/// by a `(def name …)` or `(defmacro name …)` — at any depth, since Brood's
/// `def` always binds globally regardless of where it textually sits (a
/// `(when … (def x 1))` makes `x` a global when the `when` runs).
///
/// Recursion stops at forms whose body is data, not code (`quote` /
/// `quasiquote`) — a `(quote (def x …))` is a literal list, not a binder.
/// Doesn't recurse into a `fn`/`lambda` body either: a `def` *inside* a
/// closure body only fires when the closure is called, but since the body
/// runs later and Brood's `def` is global, the result is the same — we still
/// want the name in scope. So we *do* recurse there. The only thing we skip
/// is `quote`/`quasiquote`.
fn collect_def_names(heap: &Heap, form: Value, ctx: &mut Ctx) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&Value::Sym(head)) = items.first() else {
        return;
    };
    let head_name = value::symbol_name(head);
    if matches!(head_name.as_str(), "quote" | "quasiquote") {
        return;
    }
    if matches!(head_name.as_str(), "def" | "defmacro") {
        if let Some(&Value::Sym(name)) = items.get(1) {
            ctx.add_file_global(name);
        }
    }
    for &item in &items[1..] {
        collect_def_names(heap, item, ctx);
    }
}

/// Forms whose contents are data (`quote`/`quasiquote`) or deliberately
/// exercise failures (`try` / `error-of` / `assert-error` pre-expansion;
/// `%try` post-expansion — they all bottom out at the same primitive). Don't
/// look inside.
///
/// **Post-expansion matters.** `check_file` macroexpands first so threading
/// macros and `match` patterns get their real shape — but that also rewrites
/// `(try …)` to `(%try (fn () body) (fn (e) handler))`. Without `%try` here,
/// the walk would descend into the user's "I expect this to fail" body and
/// flag the very errors they're asserting on (every `(error-of (cons 1))` in
/// the test suite would warn). `assert-error` / `error-of` expand *through*
/// `try`, so `%try` covers them too.
fn skips_body(name: &str) -> bool {
    matches!(
        name,
        "quote" | "quasiquote" | "try" | "error-of" | "assert-error" | "%try"
    )
}

/// If `test` is a recognisable type guard over a single variable, return the
/// `(sym, asserted_type)` pair — the type `sym` provably has when `test` is
/// truthy. A leading `(not …)` flips the assertion via [`Ty::negate`]. A bare
/// `Sym` is looked up in `ctx`'s guard-alias table (a `let`-stored guard
/// result — `(let (cond (int? x)) (if cond …))`). `None` for any test that
/// isn't a pure pattern-matchable guard (so we never narrow on something we
/// can't soundly invert in the else-branch).
fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<(Symbol, Ty)> {
    if let Value::Sym(s) = test {
        return ctx.guard(s);
    }
    let items = list_items(heap, test)?;
    if items.len() != 2 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // (not <inner>) — invert the inner assertion; everything else proceeds.
    if head_name == "not" {
        let (sym, ty) = guard_assertion(heap, items[1], ctx)?;
        return Some((sym, ty.negate()));
    }
    let ty = Ty::tested_by(&head_name)?;
    match items[1] {
        Value::Sym(s) => Some((s, ty)),
        _ => None,
    }
}

fn check_into(
    heap: &Heap,
    form: Value,
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let Value::Pair(_) = form else { return };
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(&head) = items.first() else { return };

    // Special-cased forms that introduce scope or refine types. Each handles
    // its own argument-walking and returns; the generic path below doesn't run.
    if let Value::Sym(s) = head {
        let name = value::symbol_name(s);
        if skips_body(&name) {
            return;
        }
        match name.as_str() {
            "if" => {
                check_if(heap, &items, ctx, out);
                return;
            }
            "let" | "let*" => {
                check_let(heap, &items, ctx, out, false);
                return;
            }
            "letrec" => {
                // `letrec` pre-binds every name to `nil` so all bindings are
                // visible in every RHS — that's the mutual-recursion reason
                // letrec exists. The checker mirrors this: it pre-binds the
                // names into the inner scope *before* walking the RHSs, so a
                // self-recursive or mutually-recursive call doesn't get
                // flagged unbound.
                check_let(heap, &items, ctx, out, true);
                return;
            }
            "fn" | "lambda" => {
                check_fn(heap, &items, ctx, out);
                return;
            }
            "def" => {
                check_def(heap, &items, ctx, out);
                return;
            }
            "defn" | "defmacro" => {
                check_defn(heap, &items, ctx, out);
                return;
            }
            _ => {}
        }

        // Resolve the callee's signature + arity (separate concerns; either
        // may be available without the other).
        let sig = sig_of(heap, &name);
        let arity = arity_of(heap, &name);
        // Unbound-symbol diagnostic: warn only when the head is **truly not
        // resolvable** — not local, not a syntactic keyword, not in the global
        // env (which includes `Value::Macro`s like `test` / `assert=` that
        // `arity_of` doesn't describe), and not in the curated stdlib table.
        // The unbound check is independent of "is the sig informative" —
        // a macro is bound even though it has no value-type sig.
        if !ctx.is_local(s)
            && !is_syntactic_keyword(&name)
            && !is_globally_bound(heap, &name)
            && curated_sig(&name).is_none()
        {
            out.push((heap.form_pos(form), format!("unbound symbol: {}", name)));
            // Still recurse into args below — they may carry their own issues.
        }

        // Arity check (independent of sig — they're separate concerns).
        if let Some(a) = arity {
            let argc = items.len() - 1;
            if !a.accepts(argc) {
                out.push((
                    heap.form_pos(form),
                    format!(
                        "{}: wrong number of arguments — expected {}, got {}",
                        name,
                        arity_str(a),
                        argc,
                    ),
                ));
            }
        }

        if let Some(sig) = sig {
            for (i, &arg) in items[1..].iter().enumerate() {
                let Some(param) = sig.param(i) else { continue };
                // Warn only on a *provable* mismatch: the argument's type shares
                // no tag with what the callee accepts. A superset, an `any`
                // result, or an unknown argument (`None`) overlaps the param, so
                // it's never flagged — no false positives.
                //
                // A `NEVER` arg type means "this branch is unreachable" — every
                // intersection with NEVER is NEVER, so it'd warn against
                // *every* param. That's all noise: the code can't execute, so
                // there's no real misuse. Skip. This shows up in pattern-match
                // lowering where a guard has narrowed a variable to a type
                // that has no inhabitants for the current branch.
                if let Some(arg_ty) = expr_ty(heap, arg, ctx) {
                    if arg_ty.is_never() {
                        continue;
                    }
                    if arg_ty.is_disjoint(param) {
                        let msg = format!(
                            "{}: argument {} expects {}, got {} ({})",
                            name,
                            i + 1,
                            param,
                            arg_ty,
                            crate::syntax::printer::print(heap, arg),
                        );
                        // Locate to the call form (a Pair the reader positioned).
                        out.push((heap.form_pos(form), msg));
                    }
                }
            }
        }
    }

    // Recurse: arguments (and any nested forms) may themselves be calls.
    for &item in &items {
        check_into(heap, item, ctx, out);
    }
}

/// `(fn (params...) docstring? body...)` (and `lambda` — the same closure
/// shape) — parse the parameter list, bind each into `ctx`, then walk the body
/// in the extended scope. Parameter positions (`& rest`, `&optional`) are
/// binders, not references, so they're not flagged as unbound.
fn check_fn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&params_form) = items.get(1) else {
        return;
    };
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    // Skip a leading docstring (a lone string when more body follows).
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3,
        _ => 2,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// `(def name value)` — the binder is in position 1, the value in 2. Don't
/// flag `name` as an unbound *reference* (it's a binder); walk `value` as an
/// expression. `name` is added to the file-globals accumulator inside
/// [`check_file`], not here (which checks one form in isolation).
fn check_def(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&value_form) = items.get(2) else {
        // `(def name)` — degenerate; skip.
        return;
    };
    check_into(heap, value_form, ctx, out);
}

/// `(defn name (params...) docstring? body...)` and the structurally identical
/// `defmacro` — the body lives in a fresh scope with `params` bound. Like
/// `def`, the `name` is a binder, not a reference; file-global accumulation
/// happens in [`check_file`].
fn check_defn(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let Some(&params_form) = items.get(2) else {
        return;
    };
    let mut scope = ctx.clone();
    for p in fn_params(heap, params_form) {
        scope = scope.bind(p, None);
    }
    let body_start = match (items.get(3), items.get(4)) {
        (Some(Value::Str(_)), Some(_)) => 4,
        _ => 3,
    };
    for &body_form in &items[body_start..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// The set of parameter-binder symbols introduced by a `fn`/`defn`/`defmacro`
/// parameter list. Handles the three Brood shapes uniformly:
///
/// - positional: `(x y z)` → `{x, y, z}`
/// - optional:   `(x &optional (y 0))` → `{x, y}`
/// - rest:       `(x & ys)` → `{x, ys}`
///
/// `&` / `&optional` themselves are markers, not binders, so they're filtered
/// out. The result is *just* what would be in scope — used to seed `Ctx`
/// without false-flagging the inner body's references.
fn fn_params(heap: &Heap, form: Value) -> Vec<Symbol> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Sym(s) => {
                let name = value::symbol_name(s);
                if name == "&" || name == "&optional" || name == "&rest" {
                    continue;
                }
                out.push(s);
            }
            // `&optional` defaults: `(name default)` — the binder is at [0].
            Value::Pair(_) | Value::Vector(_) => {
                let inner = match item {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => list_items(heap, item).unwrap_or_default(),
                };
                if let Some(&Value::Sym(s)) = inner.first() {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
    out
}

/// `(if test then else?)` — check the test in the outer ctx, then descend
/// into each branch with the ctx narrowed by what the test would assert.
/// `else` defaults to `nil` (matches the evaluator), so absent or non-pair
/// branches simply contribute no warnings.
fn check_if(heap: &Heap, items: &[Value], ctx: &Ctx, out: &mut Vec<(Option<Pos>, String)>) {
    let test = items.get(1).copied().unwrap_or(Value::Nil);
    let then_form = items.get(2).copied().unwrap_or(Value::Nil);
    let else_form = items.get(3).copied().unwrap_or(Value::Nil);

    check_into(heap, test, ctx, out);

    let (then_ctx, else_ctx) = match guard_assertion(heap, test, ctx) {
        Some((sym, ty)) => (ctx.narrow(sym, ty), ctx.narrow(sym, ty.negate())),
        None => (ctx.clone(), ctx.clone()),
    };
    check_into(heap, then_form, &then_ctx, out);
    check_into(heap, else_form, &else_ctx, out);
}

/// `(let bindings body…)` / `(let* …)` / `(letrec …)` — walk the bindings,
/// then check the body in the extended ctx. `letrec` pre-binds every name to
/// "in scope, type unknown" before walking RHSs, matching the evaluator's
/// nil-pre-bind so a self/mutual-recursive call inside a RHS isn't flagged
/// unbound. `let`/`let*` walk sequentially — each RHS sees only the
/// previously-bound names. (The let-vs-let* scope distinction doesn't affect
/// the unbound check since we only widen names; type-flow stays sound.)
///
/// Quietly skips a malformed bindings shape (a pattern-target `let`, an
/// improper list, an odd number of binding items): those are evaluator-level
/// errors and aren't this checker's job.
fn check_let(
    heap: &Heap,
    items: &[Value],
    ctx: &Ctx,
    out: &mut Vec<(Option<Pos>, String)>,
    letrec: bool,
) {
    let Some(&binds_form) = items.get(1) else {
        return;
    };
    let Some(binds) = bindings(heap, binds_form) else {
        // Unknown shape — just recurse generically so we still check nested calls.
        for &item in items {
            check_into(heap, item, ctx, out);
        }
        return;
    };
    if binds.len() % 2 != 0 {
        return;
    }
    let mut scope = ctx.clone();
    // letrec: pre-bind every name to `None` (in scope, no known type) so each
    // RHS can refer to its peers (and to itself).
    if letrec {
        let mut j = 0;
        while j < binds.len() {
            if let Value::Sym(name) = binds[j] {
                scope = scope.bind(name, None);
            }
            j += 2;
        }
    }
    let mut i = 0;
    while i < binds.len() {
        let Value::Sym(name) = binds[i] else {
            // Pattern-target binding (post-Step 4 work) — skip narrowing for it
            // but still check the RHS as an expression.
            check_into(heap, binds[i + 1], &scope, out);
            i += 2;
            continue;
        };
        let rhs = binds[i + 1];
        check_into(heap, rhs, &scope, out);
        let rhs_ty = expr_ty(heap, rhs, &scope);
        let rhs_guard = guard_assertion(heap, rhs, &scope);
        scope = scope.bind(name, rhs_ty);
        if let Some((target, gty)) = rhs_guard {
            scope = scope.add_guard(name, target, gty);
        }
        i += 2;
    }
    for &body_form in &items[2..] {
        check_into(heap, body_form, &scope, out);
    }
}

/// Parse a `let` bindings form — accepts both `(name val name val …)` lists
/// and `[name val name val …]` vectors, the two shapes the reader emits.
fn bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
    }
}

/// The elements of a proper list, or `None` for an improper list / non-list.
fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    loop {
        match v {
            Value::Nil => return Some(out),
            Value::Pair(p) => {
                let (head, tail) = heap.pair(p);
                out.push(head);
                v = tail;
            }
            _ => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::reader;

    /// A full `Interp` — primitives + the loaded prelude. We need the prelude
    /// in the global env so the new unbound-symbol diagnostic doesn't false-
    /// flag every Brood-side stdlib name (`list`, `int?`, `zero?`, `inc`, …);
    /// the previous primitives-only setup worked when the checker silently
    /// skipped unknown callees, but Step 4's unbound check has to know what's
    /// genuinely bound.
    fn warnings(src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        check_form(&interp.heap, form)
    }

    #[test]
    fn flags_literal_misuse_of_primitives() {
        assert!(warnings("(first 5)")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
        assert!(warnings("(string-length :k)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("keyword")));
        assert!(warnings("(%add 1 \"x\")")
            .iter()
            .any(|w| w.contains("%add")));
        assert!(warnings("(vector-ref [1 2] :k)")
            .iter()
            .any(|w| w.contains("vector-ref")));
    }

    #[test]
    fn no_false_positives_when_type_is_unknown_or_right() {
        assert!(warnings("(first (list 1 2))").is_empty()); // arg is a non-sig call → dynamic
        assert!(warnings("(first xs)").is_empty()); // variable → dynamic
        assert!(warnings("(first [1 2 3])").is_empty()); // vector is allowed
        assert!(warnings("(%add 1 2)").is_empty());
        assert!(warnings("(string-length \"hi\")").is_empty());
    }

    #[test]
    fn propagates_primitive_result_types() {
        // string-length returns int; first wants a list/vector → flag the int.
        assert!(warnings("(first (string-length \"a\"))")
            .iter()
            .any(|w| w.contains("first") && w.contains("int")));
    }

    #[test]
    fn an_any_result_is_not_a_false_positive() {
        // vector-ref's result type is `any` (unknown), so feeding it to
        // string-length (wants string) must NOT warn — `any` overlaps `string`.
        assert!(warnings("(string-length (vector-ref [1] 0))").is_empty());
    }

    #[test]
    fn does_not_descend_into_quote() {
        assert!(warnings("(quote (first 5))").is_empty());
    }

    #[test]
    fn curated_closures_are_checked() {
        // `+`, `<`, `map` are Brood closures, but their curated sigs let us flag
        // provable misuse — the headline cases.
        assert!(warnings("(+ 1 \"x\")")
            .iter()
            .any(|w| w.contains('+') && w.contains("number")));
        assert!(warnings("(< 1 :k)").iter().any(|w| w.contains('<')));
        // map's first argument must be callable; an int is not.
        assert!(warnings("(map 1 xs)")
            .iter()
            .any(|w| w.contains("map") && w.contains("argument 1")));
        // Correct uses, and an unknown (variable) callable, stay silent.
        assert!(warnings("(+ 1 2)").is_empty());
        assert!(warnings("(map inc xs)").is_empty()); // inc is a variable → unknown
    }

    #[test]
    fn skips_error_testing_forms() {
        // `try` and the error-asserting helpers deliberately exercise failures,
        // so misuse inside them is not flagged.
        assert!(warnings("(try (first 5) (catch e e))").is_empty());
        assert!(warnings("(error-of (first 5))").is_empty());
        assert!(warnings("(assert-error (first 5))").is_empty());
        // ...but a sibling form outside the skipped one is still checked.
        assert!(!warnings("(do (first 5) (try (first 6) (catch e e)))").is_empty());
    }

    #[test]
    fn covers_the_other_signed_primitives() {
        assert!(warnings("(mod 7 3)").is_empty());
        assert!(warnings("(mod 7 \"x\")").iter().any(|w| w.contains("mod")));
        assert!(warnings("(rem :a 3)").iter().any(|w| w.contains("rem")));
        assert!(warnings("(vector-length 5)")
            .iter()
            .any(|w| w.contains("vector-length")));
        assert!(warnings("(substring \"hi\" \"a\" 1)")
            .iter()
            .any(|w| w.contains("substring") && w.contains("argument 2")));
        assert!(warnings("(%lt 1 :k)").iter().any(|w| w.contains("%lt")));
    }

    #[test]
    fn reports_each_bad_argument() {
        // Both args provably wrong → two distinct warnings (one per position).
        let w = warnings("(mod \"a\" :b)");
        assert_eq!(w.len(), 2, "{:?}", w);
        assert!(w.iter().any(|s| s.contains("argument 1")));
        assert!(w.iter().any(|s| s.contains("argument 2")));
    }

    #[test]
    fn nested_misuse_is_found() {
        // A wrong call buried inside an argument is still reported.
        let w = warnings("(vector-length (cons (first 5) 2))");
        assert!(w.iter().any(|s| s.contains("first")));
    }

    #[test]
    fn atoms_and_malformed_forms_do_not_panic() {
        for src in ["5", "foo", "\"s\"", ":k", "()", "(5 6 7)", "(first)"] {
            // No panic, and no spurious warning on a bare atom / non-symbol head /
            // missing argument.
            let _ = warnings(src);
        }
        assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol — no diagnostics
        // `(first)` is now an arity diagnostic (0 args; first needs 1).
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1")));
    }

    // ------------- Step 3: sigs sourced from NativeFn, closure inference --------------

    /// The eight test cases below need real user-defined closures, which means
    /// running a `defn` against the global table. The `Interp` builds the full
    /// prelude (curated stdlib closures and all) on top of the primitive kernel
    /// — exactly the surface a checker is supposed to see.
    fn check_with_defs(defs: &[&str], src: &str) -> Vec<String> {
        let mut interp = crate::Interp::new();
        for d in defs {
            interp.eval_str(d).expect("def");
        }
        let form =
            crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse expression");
        // Macro-expand so any prelude wrappers (defn → fn, etc.) are gone, like
        // `brood --check`/the `check` builtin do before calling check_form.
        let form =
            crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
        check_form(&interp.heap, form)
    }

    #[test]
    fn primitive_sigs_are_read_from_native_fn() {
        // The point of Step 3: there is no parallel `primitive_sig` table.
        // The sig the checker uses for `string-length` *is* the one declared
        // next to its `Arity` in `builtins.rs`. If we ever drop the sig field
        // (or set it wrong), this catches it.
        let interp = crate::Interp::new();
        let sig = primitive_sig(&interp.heap, "string-length")
            .expect("string-length is a primitive");
        assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
        assert_eq!(sig.ret, Ty::of(Tag::Int));
        // The "no useful info" lane: a variadic any-arg primitive (str) returns
        // a Sig that param-overlaps every input, so it never warns.
        let any_sig = primitive_sig(&interp.heap, "str").expect("str is a primitive");
        assert_eq!(any_sig.rest, Some(Ty::ANY));
    }

    #[test]
    fn infers_a_straight_line_wrapper() {
        // (defn inc (x) (+ x 1)) → x : number (from +'s rest type).
        // So `(inc :k)` is a provable misuse.
        let w = check_with_defs(&["(defn inc (x) (+ x 1))"], "(inc :k)");
        assert!(
            w.iter().any(|s| s.contains("inc") && s.contains("number")),
            "expected an `inc :k` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_return_type_propagates() {
        // (defn inc (x) (+ x 1)) returns the number `+` returns; feeding it into
        // `string-length` (wants string) is a provable misuse.
        let w = check_with_defs(
            &["(defn inc (x) (+ x 1))"],
            "(string-length (inc 1))",
        );
        assert!(
            w.iter().any(|s| s.contains("string-length")),
            "expected a `string-length` warning, got {:?}",
            w
        );
    }

    #[test]
    fn inferred_params_intersect_across_positions() {
        // (defn add (x y) (+ x y)) — both x and y at + positions → number.
        let w = check_with_defs(&["(defn add (x y) (+ x y))"], "(add \"a\" 2)");
        assert!(w.iter().any(|s| s.contains("add")), "got {:?}", w);
    }

    #[test]
    fn does_not_infer_through_branches_or_lets() {
        // A body with `if`/`let` is *not* a single straight-line expression —
        // inference must skip it, leaving the closure as untyped (no warning).
        // The point: zero false positives from inference's lack of control flow.
        for (defs, call) in &[
            ("(defn maybe (x) (if (int? x) (+ x 1) x))", "(maybe :k)"),
            ("(defn shadow (x) (let (y x) (+ y 1)))", "(shadow :k)"),
        ] {
            let w = check_with_defs(&[defs], call);
            assert!(
                w.is_empty(),
                "branchy / binding bodies must not infer (so no warning): {:?}",
                w
            );
        }
    }

    #[test]
    fn does_not_infer_through_recursion() {
        // A self-recursive call has no fixed sig to read from — must skip,
        // even though the body is structurally a single call.
        let w = check_with_defs(&["(defn loop (x) (loop x))"], "(loop :k)");
        assert!(w.is_empty(), "recursive defns must not infer: {:?}", w);
    }

    #[test]
    fn skips_inference_for_variadic_or_optional_closures() {
        // A variadic-tail closure isn't a "fixed-arity straight-line" — skip.
        let w = check_with_defs(&["(defn vlist (& xs) (first xs))"], "(vlist 1 2 3)");
        assert!(w.is_empty(), "variadic defns must not infer: {:?}", w);
    }

    // ------------- Step 4: scope tracking + guard narrowing --------------

    #[test]
    fn let_binding_propagates_its_rhs_type() {
        // The RHS is a literal int — `(first x)` should flag, because x : int
        // shadows "unknown" in the body. (This is the basic let-tracking.)
        let w = warnings("(let (x 1) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_from_nested_call_propagates() {
        // RHS is a known primitive whose return type is int. So `x : int`,
        // and `(first x)` flags.
        let w = warnings("(let (x (string-length \"hi\")) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected a `first x` warning where x : int, got {:?}",
            w
        );
    }

    #[test]
    fn let_binding_of_unknown_rhs_stays_silent() {
        // RHS is a variable (unknown), so x stays unknown — `(first x)` must
        // not warn. (No false positives from let-tracking.)
        let w = warnings("(let (x foo) (first x))");
        assert!(w.is_empty(), "got {:?}", w);
    }

    #[test]
    fn inner_let_shadows_outer_binding() {
        // The outer x : int; the inner x : string. `(first x)` in the body
        // refers to the inner, which is a string — and `first` accepts list /
        // vector, disjoint from string. So a warning is still expected, but
        // the *narrowing message* must be "string", not "int". This is the
        // shadowing-correctness check (outer narrowing must not leak in).
        let w = warnings("(let (x 1) (let (x \"hi\") (first x)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("string")),
            "expected the inner string to be the source, got {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "outer int must not leak through shadowing: {:?}",
            w
        );
    }

    #[test]
    fn shadowing_with_unknown_rhs_clears_prior_narrowing() {
        // Outer x : int; inner x : <unknown var>. Inside the inner let, x is
        // unknown — `(first x)` must NOT warn (the outer narrowing must not
        // leak through the shadow).
        let w = warnings("(let (x 1) (let (x foo) (first x)))");
        assert!(w.is_empty(), "shadow must clear the prior type: {:?}", w);
    }

    #[test]
    fn vector_let_bindings_are_recognised() {
        // `(let [x 1] …)` (vector shape) must work the same as `(let (x 1) …)`.
        let w = warnings("(let [x 1] (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "vector-form let bindings must populate the ctx: {:?}",
            w
        );
    }

    #[test]
    fn let_star_behaves_like_let_for_typing() {
        // `let*` shares the sequential-binding semantics; the checker handles
        // both via the same path. Verify it still tracks types.
        let w = warnings("(let* (x 1) (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "let* must populate the ctx like let: {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_lets_a_then_branch_flag_a_misuse() {
        // In the then-branch of `(if (int? x) …)`, x : int — `(first x)` flags.
        let w = warnings("(if (int? x) (first x) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected guard narrowing to flag (first x) when x : int, got {:?}",
            w
        );
    }

    #[test]
    fn guard_narrowing_does_not_leak_into_the_else_branch() {
        // The else-branch narrows x to `not int`, which overlaps list / vector;
        // so `(first x)` must NOT warn there.
        let w = warnings("(if (int? x) nil (first x))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "else branch must not have x narrowed to int: {:?}",
            w
        );
    }

    #[test]
    fn negated_guard_flips_the_narrowing() {
        // (if (not (int? x)) …) — the then-branch narrows x to `not int`, the
        // else-branch to int.
        let w = warnings("(if (not (int? x)) nil (first x))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "the else of a negated guard must narrow to the inner type: {:?}",
            w
        );
    }

    #[test]
    fn guards_for_number_and_list_unions_narrow_to_the_union() {
        // (if (number? x) (first x) …) — x : number = int|float in the then,
        // which is disjoint from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (first x) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("number")),
            "number? must narrow to int|float: {:?}",
            w
        );
        // The list? guard should *not* warn in the then (list overlaps first's
        // expected type).
        let w = warnings("(if (list? x) (first x) nil)");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "list? must not produce a false positive on (first x): {:?}",
            w
        );
    }

    #[test]
    fn non_guard_tests_dont_narrow() {
        // The test isn't a recognised type predicate, so x stays unknown in
        // both branches — `(first x)` must not warn.
        let w = warnings("(if (zero? x) (first x) (first x))");
        assert!(w.is_empty(), "non-tag-guard test must not narrow: {:?}", w);
    }

    #[test]
    fn nested_guards_compose_their_narrowings() {
        // (if (number? x) (if (int? x) … (first x)) …) — in the inner else,
        // x is narrowed to `number ∩ ¬int` = float, which is still disjoint
        // from list/vector, so `(first x)` flags.
        let w = warnings("(if (number? x) (if (int? x) nil (first x)) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("float")),
            "nested guards must compose to float (= number ∩ ¬int): {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_when_used_as_an_if_test() {
        // The user-written shape `(let (cond (int? x)) (if cond …))` — Brood is
        // immutable, so `cond` faithfully reflects `(int? x)` until the let
        // ends. The guard-alias table maps `cond → (x, int)`, and the inner
        // `if cond` narrows x to int in the then-branch.
        let w = warnings("(let (cond (int? x)) (if cond (first x) nil))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected let-bound guard to flag (first x) in the then: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_narrows_in_the_else_branch_too() {
        // Else-branch sees x as `not int`, which overlaps list / vector, so
        // no warning — same as the direct-test case.
        let w = warnings("(let (cond (int? x)) (if cond nil (first x)))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "the else of a let-bound guard must narrow to ¬int, not int: {:?}",
            w
        );
    }

    #[test]
    fn let_bound_guard_can_be_negated_in_the_if() {
        // `(if (not cond) …)` flips the narrowing — same as `(not (int? x))`.
        let w = warnings("(let (cond (int? x)) (if (not cond) nil (first x)))");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("int")),
            "expected negation to flip the let-bound guard: {:?}",
            w
        );
    }

    #[test]
    fn rebinding_the_guard_name_clears_the_alias() {
        // After `(let (cond <unknown>) …)` shadowing, `cond` no longer aliases
        // the int-guard, so `(if cond …)` must not narrow x.
        let w = warnings(
            "(let (cond (int? x)) (let (cond foo) (if cond (first x) nil)))",
        );
        assert!(
            w.is_empty(),
            "shadowing must drop the guard alias: {:?}",
            w
        );
    }

    #[test]
    fn rebinding_to_a_non_guard_value_clears_the_alias() {
        // Same as above but with an int literal rather than an unknown var.
        let w = warnings(
            "(let (cond (int? x)) (let (cond 1) (if cond (first x) nil)))",
        );
        assert!(
            w.is_empty(),
            "shadowing with a non-guard value must drop the alias: {:?}",
            w
        );
    }

    #[test]
    fn self_aliased_guard_is_not_recorded() {
        // `(let (x (int? x)) …)` shadows the outer x with a bool; the inner
        // body's `x` is the bool, not the original — narrowing the original
        // would be unsound (it's no longer reachable), so we must not record
        // the guard. (No assertion about a warning either way — the point is
        // we don't crash and don't introduce a stale alias.)
        let w = warnings("(let (x (int? x)) (if x x nil))");
        assert!(
            !w.iter().any(|s| s.contains("first")),
            "self-aliased guards must not propagate to inner uses: {:?}",
            w
        );
    }

    #[test]
    fn let_inside_a_then_branch_can_shadow_a_narrowing() {
        // Outer narrowing: x : int. Inner shadow: x : string. The body now
        // sees x as string, so the narrowing message names string.
        let w = warnings("(if (int? x) (let (x \"hi\") (first x)) nil)");
        assert!(
            w.iter().any(|s| s.contains("first") && s.contains("string")),
            "shadow must override the guard narrowing: {:?}",
            w
        );
        assert!(
            !w.iter().any(|s| s.contains("got int")),
            "the int narrowing must not leak through the shadow: {:?}",
            w
        );
    }

    // ---------------- Step 4: arity + unbound-symbol diagnostics ----------------

    #[test]
    fn flags_too_few_arguments() {
        // `first` expects exactly 1; 0 is wrong.
        assert!(warnings("(first)")
            .iter()
            .any(|w| w.contains("first") && w.contains("expected 1") && w.contains("got 0")));
        // `string-length` expects exactly 1.
        assert!(warnings("(string-length)")
            .iter()
            .any(|w| w.contains("string-length") && w.contains("expected 1")));
    }

    #[test]
    fn flags_too_many_arguments() {
        // `rem` is `exact(2)`; calling with 3 is wrong.
        assert!(warnings("(rem 1 2 3)")
            .iter()
            .any(|w| w.contains("rem") && w.contains("expected 2") && w.contains("got 3")));
    }

    #[test]
    fn arity_message_handles_range_and_variadic() {
        // `map-get` is `range(2, 3)` → "expected 2 to 3".
        assert!(warnings("(map-get {})")
            .iter()
            .any(|w| w.contains("map-get") && w.contains("2 to 3")));
        // `apply` is `at_least(2)` → "expected 2 or more"; 1 is too few.
        assert!(warnings("(apply f)")
            .iter()
            .any(|w| w.contains("apply") && w.contains("2 or more")));
    }

    #[test]
    fn arity_pass_is_silent_for_correct_calls() {
        assert!(warnings("(first [1 2])")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        assert!(warnings("(rem 7 3)")
            .iter()
            .all(|w| !w.contains("number of arguments")));
        // Variadic: any count is fine.
        for n in 0..=5 {
            let args = (0..n)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            let w = warnings(&format!("(+ {})", args));
            assert!(
                w.iter().all(|s| !s.contains("number of arguments")),
                "(+ {}…) should not warn arity: {:?}",
                n,
                w
            );
        }
    }

    #[test]
    fn flags_unbound_call_heads() {
        assert!(warnings("(frobnicate 1)")
            .iter()
            .any(|w| w.contains("unbound symbol: frobnicate")));
        assert!(warnings("(typo-name :hi)")
            .iter()
            .any(|w| w.contains("unbound symbol: typo-name")));
    }

    #[test]
    fn unbound_is_silent_for_in_scope_names() {
        // fn/lambda params don't look unbound when used as call heads or
        // referenced in the body.
        assert!(warnings("(fn (f) (f 1 2))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // let bindings: same.
        assert!(warnings("(let (g (fn (x) x)) (g 1))")
            .iter()
            .all(|w| !w.contains("unbound")));
        // Syntactic keywords aren't bound but are never "unbound".
        for src in &["(do 1 2 3)", "(when true 1)", "(cond)", "(and)", "(or)"] {
            assert!(
                warnings(src).iter().all(|w| !w.contains("unbound")),
                "syntactic keyword must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn unbound_is_silent_for_prelude_names() {
        // The prelude is loaded in our test heap (via Interp::new()), so
        // stdlib names resolve. `inc`, `list`, `int?`, `even?`, … are all fine.
        for src in &[
            "(inc 1)",
            "(list 1 2 3)",
            "(int? 5)",
            "(zero? 0)",
            "(map (fn (x) x) [1 2 3])",
        ] {
            assert!(
                warnings(src)
                    .iter()
                    .all(|w| !w.contains("unbound")),
                "prelude name must not be flagged unbound: {} → {:?}",
                src,
                warnings(src)
            );
        }
    }

    #[test]
    fn file_globals_make_later_forms_see_earlier_defs() {
        // `check_file` accumulates top-level def names. Without that,
        // `(my-fn 1)` in form 2 would be flagged unbound — `my-fn` isn't in
        // the heap (no eval), only in the file.
        let interp = crate::Interp::new();
        let src = "(defn my-fn (x) (+ x 1))\n(my-fn 1)";
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
        let out = check_file(&mut heap, &forms);
        let msgs: Vec<_> = out.into_iter().map(|(_, m)| m).collect();
        assert!(
            msgs.iter().all(|m| !m.contains("unbound symbol: my-fn")),
            "file-local defns must shield later calls: {:?}",
            msgs
        );
    }

    #[test]
    fn fn_params_with_rest_and_optional_dont_leak() {
        // The marker symbols `&`/`&optional` themselves are *not* binders;
        // the names that follow them are.
        assert!(warnings("(fn (x & ys) (cons x ys))")
            .iter()
            .all(|w| !w.contains("unbound")));
        assert!(warnings("(fn (x &optional (y 0)) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn defn_body_sees_its_params_in_scope() {
        // A user defn whose body references its params must not flag them as
        // unbound. (The `defn` macro hasn't been expanded — the CLI checks
        // un-expanded forms — so this tests the un-expanded surface path.)
        assert!(warnings("(defn my-fn (x y) (+ x y))")
            .iter()
            .all(|w| !w.contains("unbound")));
    }

    #[test]
    fn arity_check_works_for_user_defns_in_a_real_interp() {
        // Once a defn is evaluated, its arity is derivable from its Closure.
        // `inc` (prelude) is `(defn inc (n) …)` → exact(1).
        let w = check_with_defs(&[], "(inc 1 2)");
        assert!(
            w.iter()
                .any(|s| s.contains("inc") && s.contains("expected 1")),
            "user defn arity should be enforced: {:?}",
            w
        );
    }
}
