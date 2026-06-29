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

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::value::{self, Arity, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

use super::annot;
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
    #[allow(non_upper_case_globals)]
    const int: Ty = Ty::of(Tag::Int);
    // `const` (not `let`): `Ty` is non-`Copy` (ADR-078), and these shorthands are
    // each reused by value across the loops below — a `const` mention inlines a
    // fresh value, so no `.clone()` is needed.
    #[allow(non_upper_case_globals)]
    const num: Ty = Ty::NUMBER;
    #[allow(non_upper_case_globals)]
    const any: Ty = Ty::ANY;
    #[allow(non_upper_case_globals)]
    const nil_ty: Ty = Ty::of(Tag::Nil);
    // Maps are seqable in the stdlib (`seq`/`fold` coerce them via `map-pairs`),
    // so the higher-order combinators accept maps too — without this the
    // checker would warn on `(map f some-map)` even though it runs fine.
    #[allow(non_upper_case_globals)]
    const seq: Ty = Ty::of_tags(&[Tag::Nil, Tag::Pair, Tag::Vector, Tag::Map]);
    #[allow(non_upper_case_globals)]
    const bool_ty: Ty = Ty::of(Tag::Bool);
    // `count`/`length` accept a string, map, or sequence (the prelude `count`
    // dispatches string?/map?/else-fold) — but not a number/keyword/etc.
    #[allow(non_upper_case_globals)]
    const countable: Ty =
        Ty::of_tags(&[Tag::Str, Tag::Map, Tag::Nil, Tag::Pair, Tag::Vector]);
    #[allow(non_upper_case_globals)]
    const str_ty: Ty = Ty::of(Tag::Str);
    #[allow(non_upper_case_globals)]
    const sym_ty: Ty = Ty::of(Tag::Sym);
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
    // Common helpers the checker can't infer (branchy / nested-param bodies),
    // hand-vetted against std/prelude.blsp — same soundness basis as the rest of
    // this table. Conservative on the domain (widest type the body accepts) so a
    // tighter type never false-positives:
    //   even?/odd? — body reduces via `rem`/`=`; require a number → bool.
    //   abs        — `(if (< n 0) (- n) n)`: numeric in and out.
    //   not/zero?  — accept any value (truthiness / `=`), but pin the `bool`
    //                result, so a non-bool sink like `(+ 1 (not x))` is catchable.
    //   count/len  — a string, map, or sequence → int.
    put("even?", Sig::new(vec![num], bool_ty));
    put("odd?", Sig::new(vec![num], bool_ty));
    put("abs", Sig::new(vec![num], num));
    put("not", Sig::new(vec![any], bool_ty));
    put("zero?", Sig::new(vec![any], bool_ty));
    put("count", Sig::new(vec![countable], int));
    put("length", Sig::new(vec![countable], int));
    // Output fns: println/eprintln/eprint are Brood closures with rest params,
    // so infer_sig bails — pin their nil result so `(+ 1 (println x))` is caught.
    for n in ["println", "eprintln", "eprint"] {
        put(n, Sig::variadic(any, nil_ty));
    }
    // min/max: at least one number arg (fixed) plus variadic number rest → number.
    // Variadic via rest; infer_sig bails on rest-param closures, so curate.
    for n in ["min", "max"] {
        put(n, Sig::with_rest(vec![num], num, num));
    }
    // higher-order: the first arg is a callback of a *known arity* — what the
    // combinator calls it with. The arrow's parameter count drives the
    // callback-arity check (ADR-078): `(map f xs)` calls `(f x)` → 1-ary;
    // `(reduce f init xs)` / `(fold f init xs)` call `(f acc x)` → 2-ary. The
    // arrow's tags are still `fn | native`, so the existing "non-function
    // argument" check is unchanged; the arrow only *adds* the arity refinement.
    let cb1 = Ty::arrow(Sig::new(vec![any], any));
    let cb2 = Ty::arrow(Sig::new(vec![any, any], any));
    for n in ["map", "filter"] {
        put(n, Sig::new(vec![cb1.clone(), seq], seq));
    }
    put("reduce", Sig::new(vec![cb2.clone(), any, seq], any));
    put("fold", Sig::new(vec![cb2, any, seq], any));
    // Predicates: branchy / `or`-expanded bodies that infer_sig can't walk.
    // All are widest-safe domains (any/any) so a tighter call never warns falsely.
    //   number? — body is (or (int? x) (float? x)); or-expansion hides the pattern.
    //   empty?  — cascading if chain over type-of.
    //   list?   — body is (or (nil? x) (pair? x)).
    for n in ["number?", "empty?", "list?"] {
        put(n, Sig::new(vec![any], bool_ty));
    }
    //   contains? — map-key probe (map or a live transient); bool result.
    //   member?   — linear scan over a sequence; first arg is the needle.
    put("contains?", Sig::new(vec![Ty::of(Tag::Map), any], bool_ty));
    put("member?", Sig::new(vec![any, seq], bool_ty));
    // some?/every?: both take a 1-ary callback and a sequence, return bool.
    // Curated because the body is a cond-recursive closure; infer_sig bails.
    for n in ["some?", "every?"] {
        put(n, Sig::new(vec![cb1.clone(), seq], bool_ty));
    }
    // String operations: branchy or `apply`-based bodies; infer_sig bails.
    //   symbol->string — branches on (symbol? s), returns (name s) which is string.
    //                    Domain is `symbol` so (symbol->string "x") is catchable.
    //   join           — complex if/apply body; always returns a string.
    //   string-capitalize — if-branches, both arms produce strings.
    //   string-split   — accumulator recursion; returns a list of strings
    //                    (unrefined list — list<string> would warn on (first …) = nil).
    put("symbol->string", Sig::new(vec![sym_ty], str_ty));
    put("join", Sig::new(vec![any, seq], str_ty));
    put("string-capitalize", Sig::new(vec![str_ty], str_ty));
    put("string-split", Sig::new(vec![str_ty, str_ty], Ty::LIST));
    // Equality: `=`/`not=` are multi-arm closures; infer_sig bails on multi-arm.
    // Pin the bool result so `(+ 1 (= x y))` is caught.
    for n in ["=", "not="] {
        put(n, Sig::variadic(any, bool_ty));
    }
    // String conversions: branchy bodies or `apply` — infer_sig bails.
    //   number->string — (str n): `str` has any domain; curate tighter (num → str).
    //   string->symbol — if-guard over (string? s).
    put("number->string", Sig::new(vec![num], str_ty));
    put("string->symbol", Sig::new(vec![str_ty], sym_ty));
    // String predicates: nested calls or let+branch bodies.
    //   starts-with?/ends-with? — let + and + branch.
    //   string-contains?        — (>= (index-of s needle) 0): nested call.
    //   blank?                  — let + cond recursion.
    for n in ["starts-with?", "ends-with?", "string-contains?"] {
        put(n, Sig::new(vec![str_ty, str_ty], bool_ty));
    }
    put("blank?", Sig::new(vec![str_ty], bool_ty));
    // String transforms: all call recursive helpers or use `apply`; infer_sig bails.
    //   trim/triml/trimr   — call tail-recursive aux helpers.
    //   replace            — if-branch over join/string-split.
    //   string-repeat      — (apply str (repeat n s)).
    //   pad-left/pad-right — let + if.
    //   char-at            — (substring s i (inc i)): nested call.
    for n in ["trim", "triml", "trimr"] {
        put(n, Sig::new(vec![str_ty], str_ty));
    }
    put("replace", Sig::new(vec![str_ty, str_ty, str_ty], str_ty));
    put("string-repeat", Sig::new(vec![str_ty, int], str_ty));
    for n in ["pad-left", "pad-right"] {
        put(n, Sig::new(vec![str_ty, int], str_ty));
    }
    put("char-at", Sig::new(vec![str_ty, int], str_ty));
    // String/list conversions: recursive helpers or `apply`.
    //   string->list           — calls string->list--acc (recursive).
    //   list->string           — (apply str cs).
    //   string-codepoints      — (into [] (map char->int (string->list s))).
    //   string-from-codepoints — (apply str (map int->char cs)).
    put("string->list", Sig::new(vec![str_ty], Ty::LIST));
    put("list->string", Sig::new(vec![seq], str_ty));
    put("string-codepoints", Sig::new(vec![str_ty], Ty::of(Tag::Vector)));
    put("string-from-codepoints", Sig::new(vec![seq], str_ty));
    // format: variadic with a required string template arg and a string result.
    put("format", Sig::with_rest(vec![str_ty], any, str_ty));
    // Search → int: all have branchy/recursive/optional-param bodies.
    //   index-of      — multi-clause cond over collection type.
    //   index-where   — tail-recursive helper; 1-ary predicate.
    //   string-index-of — &optional from param; infer_sig bails.
    put("index-of", Sig::new(vec![any, any], int));
    put("index-where", Sig::new(vec![cb1, seq], int));
    put("string-index-of", Sig::new(vec![str_ty, str_ty], int));
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

/// Try to peel a `(let (alias orig) inner)` wrapper where `orig` is a closure
/// parameter. Returns the inner body and a one-entry `{alias → orig}` map on
/// success, or the original body with an empty map. One level only.
fn unwrap_let_alias(
    heap: &Heap,
    body: Value,
    params: &[Symbol],
) -> (Value, HashMap<Symbol, Symbol>) {
    let empty: HashMap<Symbol, Symbol> = HashMap::new();
    let Some(items) = list_items(heap, body) else {
        return (body, empty);
    };
    // Must be exactly (let <bindings> <inner>).
    if items.len() != 3 {
        return (body, empty);
    }
    let Value::Sym(head) = items[0] else {
        return (body, empty);
    };
    if !value::symbol_is(head, "let") {
        return (body, empty);
    }
    // Bindings must be a single (alias orig) pair.
    let Some(binding) = list_items(heap, items[1]) else {
        return (body, empty);
    };
    if binding.len() != 2 {
        return (body, empty);
    }
    let (Value::Sym(alias), Value::Sym(orig)) = (binding[0], binding[1]) else {
        return (body, empty);
    };
    // `orig` must be a closure param; `alias` must not be (else it's a re-bind).
    if !params.contains(&orig) || params.contains(&alias) {
        return (body, empty);
    }
    let mut map = HashMap::new();
    map.insert(alias, orig);
    (items[2], map)
}

/// Inferred signature for a **user closure** named `sym` whose body is one
/// straight-line expression — a single call to a callee with a known
/// primitive/curated sig. Each closure parameter inherits the type the callee
/// expects at the position(s) where the parameter is passed directly; the
/// closure's return is the callee's.
///
/// Also handles a single let-alias wrapper: `(let (y x) (callee y))` is treated
/// as `(callee x)` — the alias is resolved back to the closure parameter before
/// matching. One level only; deeper nesting isn't worth the complexity.
///
/// Deliberately *narrow*. Skipped when:
/// - the body isn't exactly one expression (branches, multi-step bodies);
/// - the closure takes `&optional` / rest params;
/// - the body isn't a known-callee call (literal/variable / macro head / recursion).
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

    // Optionally unwrap a single let-alias: `(let (y x) call)` where `x` is a
    // closure param.  The alias `y` is resolved back to `x` in the arg loop.
    let (call_form, alias_map) = unwrap_let_alias(heap, body, &params);

    let items = list_items(heap, call_form)?;
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
        // Resolve alias → original closure param (identity if not aliased).
        let arg_sym = alias_map.get(&arg_sym).copied().unwrap_or(arg_sym);
        let Some(pos) = params.iter().position(|&p| p == arg_sym) else {
            continue;
        };
        let Some(expected) = callee_sig.param(i) else {
            continue;
        };
        param_tys[pos] = param_tys[pos].clone().intersect(expected);
    }
    Some(Sig::new(param_tys, callee_sig.ret))
}

/// A **user-declared** signature for `sym` — the `(sig name (A -> B))` the author
/// wrote, recorded on the heap (keyed by the module-qualified global) by the
/// `%register-sig` primitive when the `(sig …)` form evaluated at load time. Read
/// *first* by [`sig_of`], so the author's stated contract overrides body inference
/// — the whole point of the cross-module/intra-module authoritative-sig path. Only
/// an arrow type-expr yields a caller sig (a value `(sig x int)` records nothing
/// here, mirroring [`annot::parse_sig_decl`]). The file-local `ctx.declared_sig`
/// (walk.rs) still wins ahead of this for a bare file; this is the store that makes
/// a declared sig authoritative where the file-local ctx misses (a qualified
/// intra-module call, or a cross-module caller).
pub(super) fn declared_heap_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let type_value = heap.declared_sig_value(sym)?;
    annot::parse_type(heap, type_value)?.as_arrow().cloned()
}

/// The signature for `sym`, from any of the sources (user-declared → primitive →
/// curated → inferred). A user `(sig …)` declaration is **authoritative** — read
/// first so it overrides the body-inferred sig (e.g. a `number`-inferring body the
/// author declared `int`). The non-inferring middle half is exposed as
/// [`primitive_sig`] + [`curated_sig`] so [`infer_sig`] can consult the callee's
/// sig *without* kicking off another inference (the rule says inference is one step
/// deep).
pub(super) fn sig_of(heap: &Heap, sym: Symbol) -> Option<Sig> {
    declared_heap_sig(heap, sym)
        .or_else(|| primitive_sig(heap, sym))
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
