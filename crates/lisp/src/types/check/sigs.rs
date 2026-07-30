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
//! 3. **Body inference** ([`infer_sig`]), two sound tiers. (a) *Precise* — a body
//!    that is one direct call to a known sig pins each parameter to the callee's
//!    expectation (sound: a straight-line use is unconditional). (b) *Return-only*
//!    — for any other single-arm body, the return type is `expr_ty` of the body
//!    tail (parameters left `ANY`), so a multi-step/branchy function's *result* is
//!    typed. Sound (`expr_ty` over-approximates and unions branches) and — by not
//!    constraining parameters — free of the guarded-use false positive that full
//!    parameter inference would create. Complete parameter inference stays out
//!    (needs occurrence-typing; ADR-011).
//!
//! `arity_of` is independent: it works for any callable (primitive or
//! closure) without needing a sig.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

use super::annot;
use super::ctx::Ctx;
use super::infer::expr_ty;
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
    // checker would warn on `(map f some-map)` even though it runs fine. `bytes`
    // is likewise seqable — `count`/`first`/`rest`/`map`/`every?` all iterate its
    // octets at runtime — so it belongs in the domain too.
    #[allow(non_upper_case_globals)]
    const seq: Ty = Ty::of_tags(&[
        Tag::Nil,
        Tag::Pair,
        Tag::Vector,
        Tag::Map,
        Tag::Set,
        Tag::Bytes,
    ]);
    #[allow(non_upper_case_globals)]
    const bool_ty: Ty = Ty::of(Tag::Bool);
    // `count`/`length` accept a string, map, bytes, or sequence (the prelude
    // `count` dispatches string?/map?/else-fold, and bytes counts its octets) —
    // but not a number/keyword/etc.
    #[allow(non_upper_case_globals)]
    const countable: Ty = Ty::of_tags(&[
        Tag::Str,
        Tag::Map,
        Tag::Set,
        Tag::Nil,
        Tag::Pair,
        Tag::Vector,
        Tag::Bytes,
    ]);
    #[allow(non_upper_case_globals)]
    const str_ty: Ty = Ty::of(Tag::Str);
    #[allow(non_upper_case_globals)]
    const sym_ty: Ty = Ty::of(Tag::Sym);
    let mut m: SymbolMap<Sig> = SymbolMap::default();
    let mut put = |name: &str, sig: Sig| {
        m.insert(value::intern(name), sig);
    };
    // Variadic arithmetic. A numeric arg — or a `Num` RECORD, which the kernel's
    // `%add`/`%sub`/`%mul`/`%div` fallback dispatches to the type's `Num` ability (ADR-172
    // §7) — so `(+ money money)` is legal. A record is a map, so the domain/result widen to
    // `number | map`; the RESULT of a pure-numeric call is still typed precisely by
    // `numeric_call_ty` (int/float), which defers to this sig only once an operand is a
    // record/unknown — so numeric precision is unaffected, while record arithmetic and its
    // result (`(get (+ a b) :field)`) type-check cleanly.
    let num_or_record = num.union(Ty::of(Tag::Map));
    for n in ["+", "-", "*", "/"] {
        put(
            n,
            Sig::variadic(num_or_record.clone(), num_or_record.clone()),
        );
    }
    // variadic comparison: a numeric arg OR an `Ord` record — `< <= > >=` route through the
    // `compare-to` multimethod on the record cold path (ADR-179), so `(< (usd 1) (usd 2))` is
    // legal; boolean result. (A record is a map, hence the `num_or_record` domain.)
    for n in ["<", "<=", ">", ">="] {
        put(n, Sig::variadic(num_or_record.clone(), Ty::of(Tag::Bool)));
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
    // min/max: at least one number-or-`Ord`-record arg (fixed) plus a variadic rest of the
    // same → same domain (they route through `compare-to` for records, ADR-179).
    // Variadic via rest; infer_sig bails on rest-param closures, so curate.
    for n in ["min", "max"] {
        put(
            n,
            Sig::with_rest(
                vec![num_or_record.clone()],
                num_or_record.clone(),
                num_or_record.clone(),
            ),
        );
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
    put(
        "contains?",
        Sig::new(vec![Ty::of_tags(&[Tag::Map, Tag::Set]), any], bool_ty),
    );
    put("member?", Sig::new(vec![any, seq], bool_ty));
    //   get — the polymorphic accessor, and it had NO signature at all: it is
    //   multi-arity (3 arms) and `infer_sig` bails on multi-arm closures, so its
    //   domain was unconstrained and `(get 5 :k)` went unwarned while `(count 5)` and
    //   `(first 5)` were caught. Same `countable` domain as `count` — every kind `get`
    //   can index or key — with the result left `any` (the value at a key is anything)
    //   and the optional `default` slot variadic. ADR-167.
    put("get", Sig::with_rest(vec![countable, any], any, any));
    // any?/every?: both take a 1-ary callback and a sequence, return bool.
    // Curated because the body is a cond-recursive closure; infer_sig bails.
    for n in ["any?", "every?"] {
        put(n, Sig::new(vec![cb1.clone(), seq], bool_ty));
    }
    // String operations: branchy or `apply`-based bodies; infer_sig bails.
    //   symbol->string — branches on (symbol? s), returns (name s) which is string.
    //                    Domain is `symbol` so (symbol->string "x") is catchable.
    //   join           — complex if/apply body; always returns a string.
    //   capitalize     — if-branches, both arms produce strings.
    //   string-split   — accumulator recursion; returns a list of strings
    //                    (unrefined list — list<string> would warn on (first …) = nil).
    put("symbol->string", Sig::new(vec![sym_ty], str_ty));
    put("join", Sig::new(vec![any, seq], str_ty));
    put("capitalize", Sig::new(vec![str_ty], str_ty));
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
    //   blank?                  — let + cond recursion.
    for n in ["starts-with?", "ends-with?"] {
        put(n, Sig::new(vec![str_ty, str_ty], bool_ty));
    }
    //   includes? — polymorphic membership `(>= (index-of coll x) 0)` over
    //   list/vector/string (substring) and map (values); both args stay `any`.
    put("includes?", Sig::new(vec![any, any], bool_ty));
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
    //   string->list        — (string-split s "").
    //   list->string        — (apply str cs).
    //   codepoints->string  — (apply str (map int->char cs)).
    // (string->codepoints is a primitive now — its sig rides on the NativeFn.)
    put("string->list", Sig::new(vec![str_ty], Ty::LIST));
    put("list->string", Sig::new(vec![seq], str_ty));
    put("codepoints->string", Sig::new(vec![seq], str_ty));
    // format: variadic with a required string template arg and a string result.
    put("format", Sig::with_rest(vec![str_ty], any, str_ty));
    // Search → int: all have branchy/recursive/optional-param bodies.
    //   index-of      — multi-clause cond over collection type; &optional from.
    //   index-where   — tail-recursive helper; 1-ary predicate.
    //   last-index-of — &optional before param; infer_sig bails.
    put("index-of", Sig::new(vec![any, any], int));
    put("index-where", Sig::new(vec![cb1, seq], int));
    put("last-index-of", Sig::new(vec![str_ty, str_ty], int));
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
    match super::deps::obs_global(heap, sym)? {
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

thread_local! {
    /// Symbols whose signature is currently being inferred **on this thread** — a
    /// re-entry guard so the return-type inference (which runs [`expr_ty`], hence
    /// `sig_of` → `infer_sig`, over the body) can't loop on a recursive or
    /// mutually-recursive call graph. A cycle yields `None` (no inference), which
    /// is sound and conservative. Per-thread, so it stays correct under the
    /// parallel `nest check` worker pool (each worker has its own set).
    static INFERRING: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());

    /// Completed signature inferences for this check pass, keyed by symbol — the memo
    /// behind [`infer_sig`]. Per-thread (sound under the parallel checker), and cleared
    /// per file by [`clear_sig_memo`]. Stores the final `Option<Sig>` (including a
    /// deliberate `None` for "inferred nothing"), never an in-progress cycle result.
    static SIG_MEMO: RefCell<HashMap<Symbol, Option<Sig>>> = RefCell::new(HashMap::new());
}

/// Reset the per-pass inference memo. `check_file` calls this at the start of each file so
/// one file's inferred signatures never leak into the next — and, in the long-lived LSP,
/// so an edit re-infers rather than serving a stale cached sig.
pub(super) fn clear_sig_memo() {
    SIG_MEMO.with(|m| m.borrow_mut().clear());
}

/// Max cross-function depth for return-type inference. Tier-2 inference recurses
/// along the *call graph* (`infer_sig` → `expr_ty(body)` → `sig_of` → `infer_sig`
/// for a tail call into another function), and the [`INFERRING`] set only breaks
/// *cycles* — a deep acyclic chain (or one that slips a cycle via a qualified vs
/// bare name) would recurse unbounded and overflow the stack. This caps the chain:
/// beyond it, inference bails to `None` (sound — the deep function's return just
/// stays unknown). Realistic return-inference chains (a public fn delegating to an
/// internal one, …) are 2–3 deep, so this loses nothing in practice.
const MAX_INFER_DEPTH: usize = 8;

/// RAII marker for "inferring `sym` right now". [`enter`](Self::enter) returns
/// `None` when `sym` is already in progress (a cycle) **or** the inference chain
/// is already `MAX_INFER_DEPTH` deep (overflow guard) — the caller then bails —
/// and `Drop` clears the mark, so *every* early return from `infer_sig` is
/// covered without hand-threaded cleanup.
struct InferGuard(Symbol);
impl InferGuard {
    fn enter(sym: Symbol) -> Option<InferGuard> {
        INFERRING
            .with(|s| {
                let mut set = s.borrow_mut();
                if set.len() >= MAX_INFER_DEPTH {
                    return false;
                }
                set.insert(sym)
            })
            .then_some(InferGuard(sym))
    }
}
impl Drop for InferGuard {
    fn drop(&mut self) {
        INFERRING.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

/// Inferred signature for a **user closure** named `sym`. Two tiers, both sound:
///
/// 1. **Precise (params + return)** — a single-expression body that's one direct
///    call to a callee with a known primitive/curated sig (optionally through one
///    let-alias). Each parameter inherits the type the callee expects at the
///    position(s) it's passed *directly*; the return is the callee's. Sound
///    because a straight-line use is unconditional. See [`infer_from_single_call`].
/// 2. **Return-only (sound, not complete)** — for any other single-arm body, infer
///    just the *return* type as [`expr_ty`] of the body's tail, with parameters
///    bound to `ANY`. This never constrains a parameter, so it **cannot** produce
///    the guarded-use false positive that full parameter inference would (a param
///    used as a number only inside `(if (number? x) …)` must NOT be typed number).
///    Sound because `expr_ty` is a proven over-approximation (soundness oracle)
///    and already unions branch results — so even a branchy body's return is safe.
///
/// Skipped for a multi-arity closure or one taking `&optional` / rest params (no
/// single signature / arity to state cleanly). Recursion — direct or mutual — is
/// broken by [`InferGuard`], so a cyclic call graph just declines to infer.
fn infer_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    // Memoize completed inferences: a function's inferred signature is deterministic for
    // the read-only heap of a single check pass, so cache it. Without the cache every
    // re-request re-walks the whole body — and when the cycle guard is *slipped* (a self-
    // call stored as a bare name vs the qualified name the caller resolved), those re-walks
    // compound into an exponential that hangs `nest check`/the LSP (KI-13, the `deriv`
    // benchmark: ~400k body walks). The memo caps it at one walk per distinct name. It is
    // cleared per `check_file` (see [`clear_sig_memo`]) so a later edit re-infers.
    if let Some(cached) = SIG_MEMO.with(|m| m.borrow().get(&sym).cloned()) {
        return cached;
    }
    // Break inference cycles (direct/mutual recursion via `expr_ty`). A re-entry *while*
    // `sym` is being inferred yields `None` and is deliberately NOT cached — the in-
    // progress result isn't final, and a sibling call after this one finishes must get the
    // real inferred sig, not the cycle's `None`.
    // `_guard` is a named binding, not a bare `_`: it must live to the end of this function
    // so the guard is released only after `infer_sig_inner` returns.
    let _guard = InferGuard::enter(sym)?;
    let result = infer_sig_inner(heap, sym);
    SIG_MEMO.with(|m| m.borrow_mut().insert(sym, result.clone()));
    result
}

fn infer_sig_inner(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let Value::Fn(cid) = super::deps::obs_global(heap, sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    let self_name = closure.name;
    // A **complex** closure — multi-arity, or with optionals / a rest param — has no single
    // *parameter* signature to pin, but its RETURN is still the union of each arm's tail,
    // which flows to callers. Infer that (a params-less sig), since arity is checked
    // independently (`arity_of`) and a union of arm returns is a sound over-approximation —
    // a supertype can only *under*-flag a caller, never false-positive.
    let simple = closure.arms.len() == 1
        && closure.arms[0].optionals.is_empty()
        && closure.arms[0].rest.is_none();
    if !simple {
        return infer_return_only(heap, cid, self_name);
    }
    let arm = &closure.arms[0];
    if arm.body.is_empty() {
        return None;
    }
    // Copy out before we ask `sig_of` / `expr_ty` (which borrow the heap again).
    let params: Vec<Symbol> = arm.params.clone();

    // Tier 1: precise params + return from a single known-callee call.
    if arm.body.len() == 1 {
        if let Some(sig) = infer_from_single_call(heap, arm.body[0], &params, self_name) {
            return Some(sig);
        }
    }

    // Tier 1.5: parameters from **unconditional** type-demands across the whole
    // body — the sound generalisation of Tier 1 beyond a single top-level call
    // (a `do`, a nested call argument, a `let`-RHS). A param used only inside a
    // branch/guard is left `ANY` (see `collect_param_demands`), so this cannot
    // produce the guarded-use false positive. Params with no demand stay `ANY`,
    // recovering the old return-only behaviour exactly.
    let param_tys = collect_param_demands(heap, &arm.body, &params);

    // Tier 2: sound return-only inference. Bind parameters to `ANY` (in scope, no
    // constraint) and read the body tail's type — the return, unconditionally.
    // (Kept independent of the param demands above so the return type — and every
    // test pinned to it — is byte-identical to before.)
    let tail = *arm.body.last()?;
    // Mark the ctx as inferring `self_name`, so a self-recursive call in a branch result is
    // skipped in the return union (see `infer::branch_union`) — this is what lets a
    // tail-recursive function's return infer from its base cases instead of deferring.
    let mut ctx = match self_name {
        Some(name) => Ctx::default().with_inferring_self(name),
        None => Ctx::default(),
    };
    for &p in &params {
        ctx = ctx.bind(p, Some(Ty::ANY));
    }
    let ret = expr_ty(heap, tail, &ctx)?;
    Some(Sig::new(param_tys, ret))
}

/// **Return-only inference** for a complex closure (multi-arity, optionals, or a rest
/// param). Each arm's return is the type of its tail form with the arm's binders bound to
/// `ANY`; the closure's return is their union. Yields a *params-less* [`Sig`] — it flows the
/// return to callers but imposes no argument constraint (which would be wrong, since the
/// params vary per arm); arity is still checked by [`arity_of`]. Defers (`None`) if any arm's
/// return can't be typed, so an under-approximation never leaks. Sound: a union of arm
/// returns is a supertype of whatever a given call actually returns, so it can only
/// *under*-flag a caller, never false-positive.
fn infer_return_only(
    heap: &Heap,
    cid: crate::core::value::ClosureId,
    self_name: Option<Symbol>,
) -> Option<Sig> {
    // Collect each arm's binders + tail (owned) before calling `expr_ty`, which re-borrows
    // the heap. An empty-body arm makes the whole thing undecidable — bail.
    let arms: Vec<(Vec<Symbol>, Value)> = {
        let c = heap.closure(cid);
        let mut out = Vec::with_capacity(c.arms.len());
        for a in c.arms.iter() {
            let &tail = a.body.last()?;
            let mut binders = a.params.clone();
            // `optionals` are `(name, default-expr)` pairs — bind just the names.
            binders.extend(a.optionals.iter().map(|(name, _)| *name));
            if let Some(r) = a.rest {
                binders.push(r);
            }
            out.push((binders, tail));
        }
        out
    };
    if arms.is_empty() {
        return None;
    }
    let mut ret: Option<Ty> = None;
    for (binders, tail) in &arms {
        let mut ctx = match self_name {
            Some(name) => Ctx::default().with_inferring_self(name),
            None => Ctx::default(),
        };
        for &p in binders {
            ctx = ctx.bind(p, Some(Ty::ANY));
        }
        let t = expr_ty(heap, *tail, &ctx)?;
        ret = Some(match ret {
            Some(a) => a.union(t),
            None => t,
        });
    }
    Some(Sig::new(vec![], ret?))
}

/// **Same-file return inference** from a `(fn …)` *form* (not a loaded closure): the union of
/// each arm's tail type, with the arm's params bound to `ANY` in a clone of `base_ctx` — so a
/// call to another *file-local* function whose sig is already in `base_ctx` resolves. Powers
/// `check_file`'s fixpoint pass, letting a function defined in the file being checked (which
/// isn't loaded, so [`sig_of`] can't see it) still flow its return to same-file callers. The
/// return-only counterpart of [`infer_return_only`] for the form path; `None` if any arm's
/// tail can't be typed (defer — never an under-approximation). `self_name` skips a
/// self-recursive branch (see `infer::branch_union`).
pub(super) fn infer_return_from_form(
    heap: &Heap,
    fn_form: Value,
    self_name: Option<Symbol>,
    base_ctx: &Ctx,
) -> Option<Ty> {
    let items = super::walk::list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    // (params, tail) per arm — a multi-arity fn's clauses, or the single arm.
    let mut arms: Vec<(Vec<Symbol>, Value)> = Vec::new();
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        let clauses = match items.get(1..) {
            // skip a leading docstring
            Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
            Some(rest) => rest,
            None => return None,
        };
        for &clause in clauses {
            let citems = super::walk::list_items(heap, clause)?;
            if citems.len() < 2 {
                return None; // a clause with no body — can't type
            }
            let plist = *citems.first()?;
            let tail = *citems.last()?;
            arms.push((super::walk::fn_params(heap, plist), tail));
        }
    } else {
        let plist = *items.get(1)?;
        let body_start = match (items.get(2), items.get(3)) {
            (Some(Value::Str(_)), Some(_)) => 3,
            _ => 2,
        };
        let tail = *items.get(body_start..).and_then(|b| b.last())?;
        arms.push((super::walk::fn_params(heap, plist), tail));
    }
    if arms.is_empty() {
        return None;
    }
    let mut ret: Option<Ty> = None;
    for (binders, tail) in &arms {
        let mut ctx = match self_name {
            Some(n) => base_ctx.with_inferring_self(n),
            None => base_ctx.clone(),
        };
        for &p in binders {
            ctx = ctx.bind(p, Some(Ty::ANY));
        }
        let t = expr_ty(heap, *tail, &ctx)?;
        ret = Some(match ret {
            Some(a) => a.union(t),
            None => t,
        });
    }
    ret
}

/// A single-arity file function's inferred **parameter demands**, read from its
/// `(fn (params) body…)` form (ADR-190) — each param's unconditional type demand across the
/// body (see [`collect_param_demands`]). `None` for a multi-arity / malformed / no-param fn
/// (no single demand to pin). **Sound for caller-flagging:** `collect_param_demands`
/// under-constrains (a superset of the true valid-argument type), so an argument disjoint from
/// a demand is disjoint from the truth too — it genuinely errors at runtime, never a false
/// positive. The companion of [`infer_return_from_form`] (which yields the return).
pub(super) fn infer_params_from_form(heap: &Heap, fn_form: Value) -> Option<Vec<Ty>> {
    let items = super::walk::list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        return None; // params vary per clause — no single demand to store
    }
    let plist = *items.get(1)?;
    let params = super::walk::fn_params(heap, plist);
    if params.is_empty() {
        return None;
    }
    let body_start = match (items.get(2), items.get(3)) {
        (Some(Value::Str(_)), Some(_)) => 3, // skip a leading docstring
        _ => 2,
    };
    let body: Vec<Value> = items.get(body_start..)?.to_vec();
    if body.is_empty() {
        return None;
    }
    Some(collect_param_demands(heap, &body, &params))
}

/// Collect each parameter's **unconditional** type demand across the whole body:
/// the type a known-sig callee requires of a parameter passed *directly* in a
/// position guaranteed to execute on every call — a call argument, a `do` form, a
/// `let`-binding RHS or body, an `if`/`when`/`cond`/`case`/`match` *test*, an
/// `and`/`or` *first* operand. Positions gated by a branch or guard (branch arms,
/// `and`/`or` tails, `try` bodies, nested `fn` bodies, quoted forms) are skipped —
/// so a guarded use like `(if (string? x) (str x) (+ x 1))` never constrains `x`
/// (sound: no false positive). Multiple demands on one param intersect; a param
/// shadowed by an inner `let`/`fn` binder is excluded within that scope; params
/// with no unconditional demand stay `ANY`.
fn collect_param_demands(heap: &Heap, body: &[Value], params: &[Symbol]) -> Vec<Ty> {
    let mut tys = vec![Ty::ANY; params.len()];
    let shadowed: HashSet<Symbol> = HashSet::new();
    // Every top-level body form runs unconditionally (sequenced, like a `do`).
    for &form in body {
        collect_demands(heap, form, params, &shadowed, &mut tys);
    }
    // A conflicting set of unconditional demands intersects to NEVER (the function
    // could never be called successfully). Rather than flag every caller, drop such
    // a param back to `ANY` — conservative, and avoids a surprising all-callers warning.
    for t in tys.iter_mut() {
        if t.is_never() {
            *t = Ty::ANY;
        }
    }
    tys
}

/// Walk one **unconditionally-executed** `form`, recording param demands into `tys`.
/// `shadowed` names enclosing `let` binders that hide a same-named parameter.
fn collect_demands(
    heap: &Heap,
    form: Value,
    params: &[Symbol],
    shadowed: &HashSet<Symbol>,
    tys: &mut [Ty],
) {
    let Some(items) = list_items(heap, form) else {
        return; // an atom (a bare param reference demands nothing on its own)
    };
    let Some(&head) = items.first() else {
        return; // the empty list
    };
    let Value::Sym(h) = head else {
        // A computed callee `((f) x …)` — the args still evaluate unconditionally.
        for &arg in &items[1..] {
            collect_demands(heap, arg, params, shadowed, tys);
        }
        return;
    };
    // Non-executed / definer forms: nothing here runs against the parameters now.
    if value::symbol_is(h, kw::QUOTE)
        || value::symbol_is(h, kw::QUASIQUOTE)
        || value::symbol_is(h, kw::FN)
        || value::symbol_is(h, kw::TRY)
        || value::symbol_is(h, kw::DEF)
        || value::symbol_is(h, kw::DEFN)
        || value::symbol_is(h, kw::DEFMACRO)
        || value::symbol_is(h, kw::DEFDYN)
    {
        return;
    }
    // Branch/guard forms: only the TEST (or scrutinee / first cond-test / case key)
    // runs unconditionally; the arms don't.
    if value::symbol_is(h, kw::IF)
        || value::symbol_is(h, kw::WHEN)
        || value::symbol_is(h, kw::UNLESS)
        || value::symbol_is(h, kw::COND)
        || value::symbol_is(h, kw::CASE)
        || value::symbol_is(h, kw::MATCH)
    {
        if let Some(&test) = items.get(1) {
            collect_demands(heap, test, params, shadowed, tys);
        }
        return;
    }
    // `and`/`or`: only the FIRST operand is unconditional (the rest short-circuit).
    if value::symbol_is(h, kw::AND) || value::symbol_is(h, kw::OR) {
        if let Some(&first) = items.get(1) {
            collect_demands(heap, first, params, shadowed, tys);
        }
        return;
    }
    // `let`/`letrec`: each binding RHS runs (sequentially), then the body. A binder
    // shadows a same-named parameter for the remainder of the form.
    if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
        let Some(&binds_form) = items.get(1) else {
            return;
        };
        let Some(binds) = list_items(heap, binds_form) else {
            return;
        };
        if binds.len() % 2 != 0 {
            return;
        }
        let mut scope = shadowed.clone();
        let mut i = 0;
        while i < binds.len() {
            // A non-Sym binder is a destructuring pattern — we can't track exactly
            // which names it shadows, so bail on the whole form (sound: collect
            // nothing rather than risk a shadowed param leaking a demand).
            let Value::Sym(name) = binds[i] else {
                return;
            };
            collect_demands(heap, binds[i + 1], params, &scope, tys);
            scope.insert(name); // shadows a same-named param from here on
            i += 2;
        }
        for &b in &items[2..] {
            collect_demands(heap, b, params, &scope, tys);
        }
        return;
    }
    // `do`: every form runs unconditionally.
    if value::symbol_is(h, kw::DO) {
        for &f in &items[1..] {
            collect_demands(heap, f, params, shadowed, tys);
        }
        return;
    }
    // Otherwise: a normal call. All arguments evaluate unconditionally before it.
    // A parameter passed *directly* to a known-sig callee takes the demanded type;
    // every argument is itself an unconditional position (nested calls contribute).
    let callee_sig = primitive_sig(heap, h).or_else(|| curated_sig(h));
    // Ability-op occurrence typing (ADR-190): a call to a *sealed* ability op demands its
    // FIRST argument be a member of that ability (a non-member `no-impl`s at runtime), so a
    // `(defn f (s) (area s))` derives `s : Shape` with no annotation. `None` unless provably
    // sound — see `protocol::sealed_op_domain`.
    let op_domain = super::protocol::sealed_op_domain(h);
    for (i, &arg) in items[1..].iter().enumerate() {
        if let Value::Sym(a) = arg {
            if !shadowed.contains(&a) {
                if let Some(pos) = params.iter().position(|&p| p == a) {
                    if let Some(expected) = callee_sig.as_ref().and_then(|s| s.param(i)) {
                        tys[pos] = tys[pos].clone().intersect(expected);
                    }
                    if i == 0 {
                        if let Some(dom) = op_domain.clone() {
                            tys[pos] = tys[pos].clone().intersect(dom);
                        }
                    }
                }
            }
        }
        collect_demands(heap, arg, params, shadowed, tys);
    }
}

/// Tier 1 of [`infer_sig`]: the precise, parameter-inferring case — a body that is
/// exactly one call to a primitive/curated callee (optionally through one
/// let-alias `(let (y x) (callee … y …))`). Returns `None` (so `infer_sig` falls
/// to the sound return-only tier) for anything else: a non-call body, a
/// user/unknown callee, a macro head, or direct self-recursion.
fn infer_from_single_call(
    heap: &Heap,
    body: Value,
    params: &[Symbol],
    self_name: Option<Symbol>,
) -> Option<Sig> {
    // Optionally unwrap a single let-alias: `(let (y x) call)` where `x` is a
    // closure param. The alias `y` is resolved back to `x` in the arg loop.
    let (call_form, alias_map) = unwrap_let_alias(heap, body, params);
    let items = list_items(heap, call_form)?;
    let Value::Sym(callee) = items.first().copied()? else {
        return None;
    };
    // No direct self-recursion, and only a callee we can describe *without*
    // inference (`primitive`/`curated`) — so this precise tier never recurses.
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
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    annot::parse_type(heap, type_value)?.as_arrow().cloned()
}

/// The **overload** counterpart of [`declared_heap_sig`] (ADR-116) — the
/// cross-module/intra-module path for an `(and (int -> int) (bool -> bool))`
/// declaration. Reads the same heap-recorded raw type-expression
/// [`declared_heap_sig`] does, but extracts `.overload_sigs()` instead of
/// `.as_arrow()`: a genuine 2+-arm overload has `arrow: None`, so
/// `declared_heap_sig` alone silently discards it (this is the fix — the
/// heap store itself needed no change, it already holds the opaque raw
/// form). `None` for a plain single-arrow sig or no declaration at all.
pub(super) fn declared_heap_overload(heap: &Heap, sym: Symbol) -> Option<Vec<Sig>> {
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    annot::parse_type(heap, type_value)?
        .overload_sigs()
        .cloned()
}

/// The **value-type** counterpart of [`declared_heap_sig`] — a non-arrow
/// `(sig name T)` declaration read from the heap-wide store instead of the
/// file-local `Ctx::declared_value_ty` (`walk.rs`'s `parse_value_sig_decl`
/// only scans the *current file's* un-expanded forms, so it misses a
/// same-module reference that got qualified to `mod/name` during expansion,
/// and any genuinely cross-module reference). `%register-sig` already records
/// *every* `(sig …)` — arrow or not — under the qualified name, so this reads
/// the same store [`declared_heap_sig`] does; the difference is which shape
/// of `Ty` it keeps (a plain value type, not an arrow — mirrors how
/// [`parse_value_sig_decl`](super::annot::parse_value_sig_decl) is
/// `parse_sig_decl`'s non-arrow counterpart). `None` for an arrow declaration
/// (that's `declared_heap_sig`'s) or no declaration at all.
pub(super) fn declared_heap_value_ty(heap: &Heap, sym: Symbol) -> Option<Ty> {
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    let ty = annot::parse_type(heap, type_value)?;
    if ty.as_arrow().is_some() {
        return None;
    }
    Some(ty)
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
    match super::deps::obs_global(heap, sym)? {
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
    super::deps::obs_global(heap, sym).is_some()
}
