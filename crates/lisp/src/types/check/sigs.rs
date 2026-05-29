//! How the checker finds out *what shape a name has*. Three sources,
//! simplest-first (see `docs/types.md` Step 3) — **no inference engine**:
//!
//! 1. **Primitive** — every [`NativeFn`](crate::core::value::NativeFn) carries
//!    its `Sig` and `Arity` in the global env (contract point #6, enforced
//!    at construction). [`primitive_sig`] / [`arity_of`] just read it.
//! 2. **Curated stdlib** — a small hand-vetted table for variadic /
//!    `reduce`-based / higher-order closures the checker can't infer but
//!    that matter (`+ - * /`, `map`, `filter`, `reduce`, …). See
//!    [`curated_sig`].
//! 3. **One-step inference** for a closure whose body is exactly one direct
//!    call to a known sig (no `if`/`cond`/`let`/`match`/recursion). The
//!    parameter types are pinned to the callee's expectation at the
//!    position(s) where each parameter is passed. Sound because a
//!    straight-line use is unconditional. See [`infer_sig`].
//!
//! `arity_of` is independent: it works for any callable (primitive or
//! closure) without needing a sig.

use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::value::{self, Arity, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

use super::walk::list_items;

/// Curated stdlib sigs, keyed by interned `Symbol`. Built once at first
/// use — every entry's name is interned via `value::intern`, and a lookup
/// is a `SymbolMap` (FxHash-on-`u32`) probe rather than a string compare
/// chain. Pre-consolidation the checker walked every call form, allocated
/// a `String` via `symbol_name`, then matched `name.as_str()` against this
/// table — that allocation was the review's hottest finding for the
/// type-check walk. (`SymbolMap` is the same hasher `eval::SPECIAL_IDS`
/// uses.)
static CURATED_SIGS: LazyLock<SymbolMap<Sig>> = LazyLock::new(|| {
    let int = Ty::of(Tag::Int);
    let num = Ty::NUMBER;
    let any = Ty::ANY;
    // Maps are seqable in the stdlib (`seq`/`fold` coerce them via `map-pairs`),
    // so the higher-order combinators accept maps too — without this the
    // checker would warn on `(map f some-map)` even though it runs fine.
    let seq = Ty::LIST.union(Ty::of(Tag::Vector)).union(Ty::of(Tag::Map));
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));
    let mut m: SymbolMap<Sig> = SymbolMap::default();
    let mut put = |name: &str, sig: Sig| {
        m.insert(value::intern(name), sig);
    };
    // variadic arithmetic: every argument must be a number
    for n in ["+", "-", "*", "/"] {
        put(n, Sig::variadic(num, num));
    }
    // variadic comparison: numeric args, boolean result
    for n in ["<", "<=", ">", ">="] {
        put(n, Sig::variadic(num, Ty::of(Tag::Bool)));
    }
    // `mod` is Brood (over `rem`), but its types are fixed
    put("mod", Sig::new(vec![int, int], int));
    // higher-order: first arg callable, second a sequence (map included; see above)
    for n in ["map", "filter"] {
        put(n, Sig::new(vec![callable, seq], seq));
    }
    put("reduce", Sig::new(vec![callable, any, seq], any));
    m
});

/// The signature of a **primitive** bound to `sym` — read from its `NativeFn`
/// (contract point #6, enforced). `None` when `sym` has no binding, or its
/// binding isn't a primitive (a Brood closure goes through [`curated_sig`]
/// or [`infer_sig`] instead).
///
/// Lookup goes through `heap.global()`, not `EnvId::GLOBAL` directly: in a real
/// runtime that's `EnvId::GLOBAL` (routed to the shared `runtime.globals`
/// table), but in the prelude-builder / test heap it's a *local* env that
/// `builtins::register` populated — `env_get` walks both transparently.
pub(super) fn primitive_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    match heap.env_get(heap.global(), sym)? {
        Value::Native(id) => Some(heap.native(id).sig.clone()),
        _ => None,
    }
}

/// Signatures for the stable stdlib **closures** the checker can't infer but
/// that matter: the arithmetic/comparison kernel (variadic over numbers) and the
/// core higher-order fns. Hand-vetted, so sound — this is what makes `(+ 1 "x")`
/// catchable even though `+` is `(reduce %add 0 xs)`.
pub(super) fn curated_sig(sym: Symbol) -> Option<Sig> {
    CURATED_SIGS.get(&sym).cloned()
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
fn infer_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let Value::Fn(cid) = heap.env_get(heap.global(), sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    // Only infer for a plain single-arity, single-body closure (no optionals /
    // rest). A multi-arity closure has no single signature to infer — bail.
    if closure.arms.len() != 1 {
        return None;
    }
    let arm = &closure.arms[0];
    if arm.body.len() != 1 || !arm.optionals.is_empty() || arm.rest.is_some() {
        return None;
    }
    let body = arm.body[0];
    // Copy out before we ask sig_of (which borrows the heap again).
    let params: Vec<Symbol> = arm.params.clone();
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
    let callee_sig = primitive_sig(heap, callee).or_else(|| curated_sig(callee))?;

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

/// The signature for `sym`, from any of the three sources (primitive → curated
/// → inferred). The non-inferring half is exposed as [`primitive_sig`] +
/// [`curated_sig`] so [`infer_sig`] can consult the callee's sig *without*
/// kicking off another inference (the rule says inference is one step deep).
pub(super) fn sig_of(heap: &Heap, sym: Symbol) -> Option<Sig> {
    primitive_sig(heap, sym)
        .or_else(|| curated_sig(sym))
        .or_else(|| infer_sig(heap, sym))
}

/// The arity of the callable bound to `sym` — `NativeFn.arity` for primitives,
/// derived from `Closure.{params, optionals, rest}` for Brood closures. `None`
/// when the name resolves to a non-callable, doesn't exist, or no callable is
/// visible (e.g. a file-local `defn` checked in the read-only `--check` path
/// — there's nothing to inspect, so no arity check fires).
///
/// Brood's closure params are: `params.len()` required + `optionals.len()`
/// optional + an optional rest tail (`Symbol`). So min = required, max =
/// required + optional unless there's a rest (then no max).
pub(super) fn arity_of(heap: &Heap, sym: Symbol) -> Option<Arity> {
    match heap.env_get(heap.global(), sym)? {
        Value::Native(id) => Some(heap.native(id).arity),
        Value::Fn(cid) => {
            // Across arms: smallest min, largest max (unbounded if any has rest).
            let c = heap.closure(cid);
            let min = c.arms.iter().map(|a| a.min_arity()).min().unwrap_or(0);
            let max = c
                .arms
                .iter()
                .try_fold(0usize, |acc, a| a.max_arity().map(|m| acc.max(m)));
            Some(Arity { min, max })
        }
        _ => None,
    }
}

/// A human-readable rendering of an `Arity` for a "wrong number of args"
/// warning — `exact(2)` → "2"; `range(2,3)` → "2 to 3"; `at_least(2)` → "2 or
/// more".
pub(super) fn arity_str(a: Arity) -> String {
    match a.max {
        Some(m) if m == a.min => a.min.to_string(),
        Some(m) => format!("{} to {}", a.min, m),
        None => format!("{} or more", a.min),
    }
}

/// Does `sym` resolve to *any* value in the global env? Broader than
/// `sig_of` / `arity_of` (which only return for callables they know how to
/// describe). A `Value::Macro`, a constant, or anything else that's actually
/// bound counts as "in scope" for the unbound-symbol check — we don't warn
/// just because the checker can't say much about the binding's *shape*.
pub(super) fn is_globally_bound(heap: &Heap, sym: Symbol) -> bool {
    heap.env_get(heap.global(), sym).is_some()
}
