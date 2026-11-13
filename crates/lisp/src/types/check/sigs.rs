//! How the checker finds out *what shape a name has* — [`sig_of`] states the order of
//! the sources (operator, declared, primitive, image, curated, inferred). The ones that
//! need explaining:
//!
//! - **Primitive** — every [`NativeFn`](crate::core::value::NativeFn) carries its `Sig`
//!   and `Arity` in the global env (contract point #6, enforced at construction).
//!   [`primitive_sig`] / [`arity_of`] just read it.
//! - **Curated** — a small hand-vetted table for the closures nothing else types
//!   (`map`, `reduce`, `io/puts`, …). See [`curated_sig`].
//! - **Body inference** ([`infer_sig`]), two sound tiers. (a) *Precise* — a body
//!   that is one direct call to a known sig pins each parameter to the callee's
//!   expectation (sound: a straight-line use is unconditional). (b) *Return-only*
//!   — for any other single-arm body, the return type is `expr_ty` of the body
//!   tail (parameters left `ANY`), so a multi-step/branchy function's *result* is
//!   typed. Sound (`expr_ty` over-approximates and unions branches) and — by not
//!   constraining parameters — free of the guarded-use false positive that full
//!   parameter inference would create. Complete parameter inference stays out
//!   (needs occurrence-typing; ADR-011).
//!
//! `arity_of` is independent: it works for any callable (primitive or
//! closure) without needing a sig.
//!
//! Three passes live in children: [`specialize`] (re-typing a body under one call's
//! argument types), [`param_domains`] (what each parameter must be across a body's
//! executions) and [`caller_derived`] (ADR-341, parameter types from a file's call sites).

mod caller_derived;
mod param_domains;
mod specialize;

pub(super) use caller_derived::*;
use param_domains::*;
pub(super) use specialize::*;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use crate::core::heap::{Heap, SymbolMap};
use crate::core::keywords as kw;
use crate::core::value::{self, Arity, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

// The checker modules the children name as `super::walk` and so on.
use super::{deps, guards, infer, protocol, walk};

use super::annot;
use super::ctx::Ctx;
use super::infer::expr_ty;
use super::walk::list_items;

/// Curated stdlib sigs, keyed by interned `Symbol` (a `SymbolMap` probe, no spelling
/// allocated per call). The last authored source [`sig_of`] consults — see its doc for
/// the order — so it holds only names no other source types.
///
/// **Why these are not `(sig …)` lines in the prelude.** The original reason — the prelude
/// could not carry a signature at all — no longer holds (it carries several). What does:
/// a prelude `sig` is a runtime CONTRACT wherever contracts are armed (the `nest run` /
/// `nest test` default, ADR-381), wrapping the name in a checking shim at boot. The names
/// here are the hottest calls in the language (`map`, `fold`, `count`, `get`, `=`, `not`),
/// so moving one is a per-call cost to be measured on the benchmark machine first, and a
/// name the macro expander calls can blow its stack budget once wrapped (KI-81, why
/// `reverse`/`append` are exempt). A curated entry is a checker fact with no runtime cost.
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
    // Maps are seqable in the stdlib (`seq`/`fold` coerce them via `%map-pairs`),
    // so the higher-order combinators accept maps too — without this the
    // checker would warn on `(map f some-map)` even though it runs fine. `bytes`
    // is likewise seqable — `count`/`first`/`rest`/`map`/`every?` all iterate its
    // octets at runtime — so it belongs in the domain too. That set is `Ty::SEQABLE`,
    // the type `seqable` names in a `sig`; spelled once, there.
    #[allow(non_upper_case_globals)]
    const seq: Ty = Ty::SEQABLE;
    #[allow(non_upper_case_globals)]
    const bool_ty: Ty = Ty::of(Tag::Bool);
    // `count` accepts a string, map, bytes, or sequence (the prelude
    // `count` dispatches string?/map?/else-fold, and bytes counts its octets) —
    // but not a number/keyword/etc. Rope and Table too (ADR-253). A signature narrower than the function is a
    // false positive on correct code, so this list has to move with the dispatch.
    #[allow(non_upper_case_globals)]
    const countable: Ty = Ty::COUNTABLE;
    #[allow(non_upper_case_globals)]
    const str_ty: Ty = Ty::of(Tag::Str);
    #[allow(non_upper_case_globals)]
    const sym_ty: Ty = Ty::of(Tag::Sym);
    let mut m: SymbolMap<Sig> = SymbolMap::default();
    let mut put = |name: &str, sig: Sig| {
        m.insert(value::intern(name), sig);
    };
    // Only what NOTHING ELSE types belongs here: an operator (`operator_sig`), a primitive
    // (its `NativeFn` sig) or a name `std/` declares (`(sig …)`) is never curated. An entry
    // also marks its name as KNOWN, which suppresses the unbound lint — so every key is
    // the name a program writes (`math/zero?`, never a moved bare `zero?`), and a name
    // nothing defines must never be added (`tests/curated_sigs_exist.rs`).
    //
    // `not`/`math/zero?` and the float predicates accept any value but pin the `bool`
    // result, so a non-bool sink like `(+ 1 (not x))` is catchable.
    put("not", Sig::new(vec![any], bool_ty));
    put("math/zero?", Sig::new(vec![any], bool_ty));
    put("math/nan?", Sig::new(vec![any], bool_ty));
    put("math/infinite?", Sig::new(vec![any], bool_ty));
    put("math/rational", Sig::new(vec![num, num], num));
    put("reflect/read-string", Sig::new(vec![str_ty], any));
    put("reflect/read-all", Sig::new(vec![str_ty], any));
    put("reflect/read-first", Sig::new(vec![str_ty], any));
    put("count", Sig::new(vec![countable], int));
    // Output fns: rest-param closures `infer_sig` bails on — pin their nil result so
    // `(+ 1 (io/puts x))` is caught.
    for n in ["io/puts", "io/write"] {
        put(n, Sig::variadic(any, nil_ty));
    }
    // higher-order: the callback is of a *known arity* — what the combinator calls
    // it with. The arrow's parameter count drives the callback-arity check
    // (ADR-078): `(map xs f)` calls `(f x)` → 1-ary; `(fold xs init f)` calls
    // `(f acc x)` → 2-ary. The arrow's tags are still `fn | native`, so the
    // existing "non-function argument" check is unchanged; the arrow only *adds*
    // the arity refinement.
    //
    // These are data-FIRST (ADR-308): the collection is argument one, matching
    // `Enum.map(enum, fun)` and `Enum.reduce/2`+`/3`. `reduce`'s `& more` was
    // only an optional-init slot, and data-first moves the varying slot to the
    // end, so both of its arities survive the change unchanged in meaning.
    let cb1 = Ty::arrow(Sig::new(vec![any], any));
    let cb2 = Ty::arrow(Sig::new(vec![any, any], any));
    // The result is a LIST, exactly: both build with `cons` over whatever they walk —
    // a vector, a set, a map, a seq-view — and answer `nil` for nothing. `seqable` was
    // honest and loose, and the looseness cost every consumer with a narrower domain:
    // `(sort (map …))` read as `seqable` into `nil | pair | vector | set` under
    // `--strict`, at every one of the six such sites in bedit.
    put("map", Sig::new(vec![seq, cb1.clone()], Ty::LIST));
    put("reduce", Sig::new(vec![seq, any, cb2.clone()], any));
    put("fold", Sig::new(vec![seq, any, cb2], any));
    // The rest of the data-first sequence walkers (ADR-308) — curated for the same
    // reason, and because their ABSENCE was load-bearing: during bedit's 0.20
    // migration, seven pre-reorder call sites (`(take n coll)`, `(drop n coll)`,
    // `(mapcat f coll)`, `(fold f init coll)`, …) sailed through `nest check`
    // silently and failed only at runtime. A number or callback in the collection
    // slot is exactly what these domains exist to reject. Results stay `any`
    // (drop can return its input unchanged, so even "list" would be wrong);
    // precise results for the inferable ones come from `infer.rs`'s arms, which
    // run independently of these domains.
    for n in ["each", "seq/take-while", "seq/drop-while"] {
        put(n, Sig::new(vec![seq, cb1.clone()], any));
    }
    // `mapcat`, `seq/filter`, `seq/reject` and `seq/keep` build a fresh list like `map`
    // does (`append` / `cons`; `nil` for nothing) — so `list`, for the reason given there.
    // `seq/` is qualified, like `seq/index-where` above (a bare key would suppress
    // the unbound lint on names that no longer exist bare, ADR-227) — which is why
    // `filter` sits here rather than beside `map` since ADR-330 moved it out of the
    // prelude: bare `filter` is unbound now, and a bare key would hide that.
    for n in ["mapcat", "seq/filter", "seq/reject", "seq/keep"] {
        put(n, Sig::new(vec![seq, cb1.clone()], Ty::LIST));
    }
    // The count is `number`, not `int`: the merely-wider residue (a provably-int
    // value typed `number` — observer's `start`, string.blsp's `(inc lim)`) is the
    // class the checker deliberately defers (ADR-011), and the STRICT gate showed
    // `int` here flags exactly those legitimate sites. The wrong-ORDER class this
    // signature exists for is caught by the seqable slot either way.
    put("take", Sig::new(vec![seq, num.clone()], any));
    put("drop", Sig::new(vec![seq, num], any));
    // Predicates: `or`-expanded bodies that infer_sig can't walk, with the widest-safe
    // domain so a tighter call never warns falsely.
    for n in ["number?", "list?"] {
        put(n, Sig::new(vec![any], bool_ty));
    }
    put(
        "contains?",
        Sig::new(
            vec![Ty::of_tags(&[Tag::Map, Tag::Record, Tag::Set]), any],
            bool_ty,
        ),
    );
    //   get — the polymorphic accessor, and it had NO signature at all: it is
    //   multi-arity (3 arms) and `infer_sig` bails on multi-arm closures, so its
    //   domain was unconstrained and `(get 5 :k)` went unwarned while `(count 5)` and
    //   `(first 5)` were caught. Same `countable` domain as `count` — every kind `get`
    //   can index or key — with the result left `any` (the value at a key is anything)
    //   and the optional `default` slot variadic. ADR-167.
    put("get", Sig::with_rest(vec![countable, any], any, any));
    // any?/every?: both take a sequence and a 1-ary callback (data-first, ADR-308),
    // return bool. Curated because the body is a cond-recursive closure; infer_sig bails.
    for n in ["any?", "every?"] {
        put(n, Sig::new(vec![seq, cb1.clone()], bool_ty));
    }
    // `->string` renders ANY value as text: always a string.
    put("->string", Sig::new(vec![Ty::ANY], str_ty));
    // Equality: `=`/`not=` are multi-arm closures; infer_sig bails on multi-arm.
    // Pin the bool result so `(+ 1 (= x y))` is caught.
    for n in ["=", "not="] {
        put(n, Sig::variadic(any, bool_ty));
    }
    // String conversions: branchy bodies or `apply` — infer_sig bails.
    //   string/->symbol — if-guard over (string? s).
    put("string/->symbol", Sig::new(vec![str_ty], sym_ty));
    //   includes? — polymorphic membership over list/vector/string and map (values).
    put("includes?", Sig::new(vec![any, any], bool_ty));
    put("string/list->", Sig::new(vec![seq], str_ty));
    put("seq/index-where", Sig::new(vec![seq, cb1], int));
    m
});

thread_local! {
    /// The operator-sugar domains (`protocol::operator_domains`) for the check in progress —
    /// set at a file check's root from that file's forms + the registry, or derived from the
    /// registry alone on first use when no file check installed one (a bare fragment).
    static OPERATOR_DOMAINS: RefCell<Option<HashMap<Symbol, Ty>>> = const { RefCell::new(None) };
}

/// Install the operator domains for the check in progress (ADR-299).
pub(super) fn set_operator_domains(domains: HashMap<Symbol, Ty>) {
    OPERATOR_DOMAINS.with(|d| *d.borrow_mut() = Some(domains));
}

/// The domain installed for operator `op` — filled from the registry on first use when no
/// file check has installed one (a bare fragment, a `Display` of a type outside a check).
fn operator_domain(heap: Option<&Heap>, op: &str) -> Option<Ty> {
    OPERATOR_DOMAINS.with(|d| {
        let mut slot = d.borrow_mut();
        if slot.is_none() {
            let heap = heap?;
            let info = super::protocol::build_multi_info(heap, &[], None);
            *slot = Some(super::protocol::operator_domains(&info));
        }
        slot.as_ref()
            .and_then(|m| m.get(&value::intern(op)).cloned())
    })
}

/// A **named cover** (ADR-299): `numeric` is `number` plus every record `num/*` has a
/// method for, `ordered` is `number` plus every record `compare-to` has a method for — the
/// domains of `+` and `<`, under the names a `sig` can write and a suggestion can print
/// without going stale when another record gains a method. `None` for any other name; with
/// no domains installed and no heap to read them from, the plain `number` each reduces to.
pub(crate) fn named_cover(heap: Option<&Heap>, name: &str) -> Option<Ty> {
    let op = match name {
        "numeric" => "+",
        "ordered" => "<",
        _ => return None,
    };
    Some(operator_domain(heap, op).unwrap_or(Ty::NUMBER))
}

/// The cover name `ty` IS, when it is one of the two and wider than plain `number`, with
/// the number of records in it — so a renderer can decide between the name and the list.
pub(crate) fn cover_name_of(ty: &Ty) -> Option<(&'static str, usize)> {
    for (name, op) in [("numeric", "+"), ("ordered", "<")] {
        let Some(domain) = operator_domain(None, op) else {
            continue;
        };
        if domain != Ty::NUMBER && *ty == domain {
            let records = domain
                .project_record_ids()
                .map(|ids| ids.len())
                .unwrap_or(0);
            return Some((name, records));
        }
    }
    None
}

/// The signature of an arithmetic/comparison operator, its domain read off the multimethod
/// registry (ADR-299) — `number` plus exactly the records `num/*` / `compare-to` have
/// methods for — rather than the `number | map` the native declares. `None` for any
/// other symbol.
fn operator_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let name = value::symbol_name_ref(sym);
    let is_arithmetic = matches!(name, "+" | "-" | "*" | "/");
    let is_comparison = matches!(name, "<" | "<=" | ">" | ">=");
    // `%max`/`%min` compare through `compare-to` like `<` and hand back an operand.
    let is_extremum = matches!(name, "%max" | "%min" | "math/max" | "math/min");
    if !is_arithmetic && !is_comparison && !is_extremum {
        return None;
    }
    let domain = operator_domain(Some(heap), name)?;
    Some(if is_comparison {
        Sig::variadic(domain, Ty::of(Tag::Bool))
    } else {
        Sig::variadic(domain.clone(), domain)
    })
}

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
    if let Some(sig) = operator_sig(heap, sym) {
        return Some(sig);
    }
    match super::deps::obs_global(heap, sym)? {
        Value::Native(id) => Some(heap.native(id).sig.clone()),
        _ => None,
    }
}

/// **The one place a data-first combinator's argument positions are stated (ADR-308).**
///
/// `map`/`keep`/`fold`/`reduce` all take the collection FIRST and the callback LAST — the
/// convention `curated_combinators_put_the_callback_last` asserts over the curated table.
/// Reading them positionally in each consumer is what made the ADR-308 reorder expensive:
/// `infer.rs` (result inference), `walk.rs` (callback synthesis) and `specialize_call`
/// each restated `items[1]`/`items[2]`, and when they went stale the checker degraded to
/// `any` *silently*. They all call this instead, so the convention lives once.
///
/// Deriving from arity rather than from the signature's parameter count is deliberate:
/// `reduce` has two arities — `(reduce coll f)` and `(reduce coll acc f)` — and the
/// callback is last in both, while a fixed index would be right for only one.
///
/// `items` is the whole call form, head included. `None` when it is too short to have
/// both a collection and a callback.
pub(super) fn combinator_args(items: &[Value]) -> Option<(Value, Value)> {
    if items.len() < 3 {
        return None;
    }
    Some((items[1], *items.last()?))
}

/// core higher-order fns. Hand-vetted, so sound — this is what makes `(+ 1 "x")`
/// catchable even though `+` is `(reduce %add 0 xs)`.
pub(super) fn curated_sig(sym: Symbol) -> Option<Sig> {
    CURATED_SIGS.get(&sym).cloned()
}

/// Does the curated table carry a signature for `sym`? [`curated_sig`] without cloning the
/// `Sig` it would hand back, for the callers that only ask whether one exists.
pub(super) fn has_curated_sig(sym: Symbol) -> bool {
    CURATED_SIGS.contains_key(&sym)
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

    /// The same memo for **multi-arm** inference ([`infer_overload_of`]) — a separate
    /// table because the two answer different questions about the same name (one arm's
    /// signature vs every arm's), and a multi-arm closure has no single `Sig` to cache.
    static OVERLOAD_MEMO: RefCell<HashMap<Symbol, Option<Vec<Sig>>>> =
        RefCell::new(HashMap::new());
}

/// Arm re-typings [`specialized_ret`] has spent since the last [`clear_sig_memo`] — i.e.
/// on the file currently being checked. The memo makes a repeated question free, so this
/// tracks the number of DISTINCT questions the file asked; a multiple of that is the
/// KI-138 signature (2760 re-typings for 52 distinct questions), which
/// `tests/check_specialization_cost.rs` bounds.
pub(super) fn fuel_spent() -> u32 {
    SPECIAL_FUEL.with(|f| MAX_SPECIAL_FUEL - f.get())
}

/// Did this file spend its whole specialization budget? Every question asked after that
/// answered "unknown" (B6 reports it).
pub(super) fn fuel_exhausted() -> bool {
    SPECIAL_FUEL.with(|f| f.get() == 0)
}

/// Drop the completed specializations alone — once per round of the joint caller-derived
/// fixpoint. A specialization re-types a body under the file's returns and derived
/// parameters as they stand, and the memo is keyed by `(name, argument types)` only, so
/// an answer kept across rounds would make the fixpoint depend on which question was
/// asked first. The fuel is not reset — it bounds the file, not the round.
pub(super) fn clear_specializations() {
    SPECIAL_MEMO.with(|m| m.borrow_mut().clear());
}

/// Reset the per-pass inference memo. `check_file` calls this at the start of each file so
/// one file's inferred signatures never leak into the next — and, in the long-lived LSP,
/// so an edit re-infers rather than serving a stale cached sig.
pub(super) fn clear_sig_memo() {
    SIG_MEMO.with(|m| m.borrow_mut().clear());
    OVERLOAD_MEMO.with(|m| m.borrow_mut().clear());
    SPECIAL_MEMO.with(|m| m.borrow_mut().clear());
    NO_ARMS.with(|m| m.borrow_mut().clear());
    SPECIAL_FUEL.with(|f| f.set(MAX_SPECIAL_FUEL));
    // Only DELTAS of this are ever read, so the reset is not needed for correctness — it
    // keeps a long-lived LSP process from ever reaching the saturating ceiling, where a
    // delta would read zero and a refused `None` would look stable.
    REFUSALS.with(|c| c.set(0));
    super::infer::reset_expr_ty_visits();
    // The operator domains are a file's too (`set_operator_domains`); a fragment checked
    // after a file must derive its own from the registry, not read that file's.
    OPERATOR_DOMAINS.with(|m| *m.borrow_mut() = None);
}

thread_local! {
    /// Names whose body is currently being re-typed under call-site argument types
    /// ([`specialized_ret`]) — the re-entry guard for that path, separate from
    /// [`INFERRING`] so the two do not compete for one depth budget. Same discipline:
    /// a refusal never constructs a guard (KI-87).
    static SPECIALIZING: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
    /// Specialization questions [`SpecializeGuard`] has refused — a name already in flight,
    /// or the depth cap. A refusal is a fact about WHERE the question was asked, not about
    /// the question, so a `None` reached while one happened underneath is not a stable
    /// answer and must not be memoized (KI-138's fix is bounded by this counter).
    static REFUSALS: Cell<u32> = const { Cell::new(0) };
    /// Arm re-typings [`specialized_ret`] may still do in this file check — the bound on
    /// its total work. The memo makes a REPEATED question free, but a question refused
    /// by the guard (a name in flight, the depth cap) has no answer to memoize, and the
    /// walker asks it again at every enclosing level: the `(:use io)` header of a
    /// two-line file drove 933k specializations of `require-one` and timed
    /// out. Past the budget the answer is `None` — the flat signature stands, sound —
    /// and the file check finishes. Reset per file by [`clear_sig_memo`].
    static SPECIAL_FUEL: Cell<u32> = const { Cell::new(MAX_SPECIAL_FUEL) };
    /// How many call-site argument typings for specialization are nested right now —
    /// `(f (g (h x)))` types `f`'s operand `(g …)` to specialize `f`, which would type
    /// `(h x)` to specialize `g`, and so on down. Unbounded, `expr_ty` of a call form
    /// walks the call's whole subtree, and the file walker asks `expr_ty` at every
    /// enclosing level: `tests/maps_test.blsp` went from 0.15 s to 7.7 s (15 M `expr_ty`
    /// calls) the day this landed. Past [`MAX_SPEC_ARG_NEST`] a nested call keeps its
    /// flat signature, so one call form costs O(operands), not O(subtree). Reset to 0
    /// while an arm body is re-typed — that work is bounded by the depth cap and fuel.
    static SPEC_ARG_NEST: Cell<u8> = const { Cell::new(0) };

    /// Completed specializations for this check pass, keyed by `(name, argument types)`.
    /// Per-thread, cleared per file by [`clear_sig_memo`]; a deliberate `None` is stored
    /// too, so an unhelpful shape is not re-typed at every call site.
    /// Image-bound names [`fixed_arms_of`] found nothing to re-type for — every builtin
    /// and primitive, every variadic loaded definition. A fact about the image, so it is
    /// recorded by NAME (under exactly the condition `specialized_ret` memoizes its `None`
    /// by argument tuple) and consulted by [`specialize_call`] BEFORE it types the call's
    /// operands: on bedit that operand walk was the profile's hottest frame, and for these
    /// names it bought nothing. Same-file names never enter — their arms arrive during
    /// same-file inference. Reset per file with the memo.
    static NO_ARMS: RefCell<HashSet<Symbol>> = RefCell::new(HashSet::new());
    static SPECIAL_MEMO: RefCell<HashMap<(Symbol, Vec<Option<Ty>>), Option<Ty>>> =
        RefCell::new(HashMap::new());
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
        let entered = INFERRING.with(|s| {
            let mut set = s.borrow_mut();
            if set.len() >= MAX_INFER_DEPTH {
                return false;
            }
            set.insert(sym)
        });
        // Construct the guard ONLY on the success path. `bool::then_some(InferGuard(sym))`
        // builds its argument eagerly, so on the refusal path a guard was built and dropped
        // at once — and its `Drop` removed `sym` from the set while the *outer* inference
        // of `sym` was still in flight. That un-guarded every cycle the guard refused: the
        // next reference to `sym` re-entered it, and a mutually recursive pair
        // (`require-one` ⇄ `%require-await`) re-inferred each other without bound, each
        // level growing the stack, until the checker exhausted memory (54 GB on a `nest
        // run`). Nothing may drop a guard that was never entered.
        entered.then(|| InferGuard(sym))
    }
}
impl Drop for InferGuard {
    fn drop(&mut self) {
        INFERRING.with(|s| {
            s.borrow_mut().remove(&self.0);
        });
    }
}

/// Inferred signature for a **user closure** named `sym`, sound in both halves:
///
/// - **Parameters** are their *domains* (ADR-261, [`param_domains`]): the union over the
///   body's possible executions of what each execution demands, a guarded use credited
///   only within its guard. This over-approximates the valid arguments, so a caller
///   rejected on it would have failed anyway — and a parameter nothing constrains stays
///   `ANY` (a param used as a number only inside `(if (number? x) …)` is NOT typed number).
/// - **The return** is [`expr_ty`] of the body's tail with each parameter bound to its
///   demand — what every call that reaches the tail satisfied. Sound because `expr_ty` is a
///   proven over-approximation (soundness oracle) and already unions branch results.
///
/// A multi-arity closure or one taking `&optional` params has no single parameter
/// signature and gets return-only inference ([`infer_return_only`]); a `& rest` binder is
/// fine (its demand becomes a per-argument one). Recursion — direct or mutual — is broken
/// by [`InferGuard`], so a cyclic call graph just declines to infer.
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

/// Every arm of a **multi-arm** closure, as its own signature — the inferred
/// counterpart of a declared overload (`(sig f (and (int -> int) (bool -> bool)))`).
///
/// A multi-arm closure has no single signature, so [`infer_sig`] falls back to a
/// params-less return-only sig for one and a call's arguments go unchecked. But each
/// *arm* does have one, and the runtime picks an arm by arity and (for a `:when`
/// guard, which lowers into the arm's body) by guard — so a call whose arguments no
/// arm of the matching arity accepts errors as surely as a single-arm mismatch:
///
/// ```lisp
/// (defn g ((x) :when (string? x) (%string-length x))
///         ((x) :when (int? x)    (+ x 1)))
/// (g :kw)   ; no clause takes a keyword
/// ```
///
/// `None` for a single-arm closure (that is [`infer_sig`]'s job) or a non-closure.
/// The call-site check requires *every* arity-relevant arm to reject before it warns
/// (see `walk::overload_arg_mismatch`), so an arm whose domain stays `any` — the
/// common case — keeps the whole call silent.
pub(super) fn infer_overload_of(heap: &Heap, sym: Symbol) -> Option<Vec<Sig>> {
    if let Some(cached) = OVERLOAD_MEMO.with(|m| m.borrow().get(&sym).cloned()) {
        return cached;
    }
    let _guard = InferGuard::enter(sym)?;
    let result = infer_overload_inner(heap, sym);
    OVERLOAD_MEMO.with(|m| m.borrow_mut().insert(sym, result.clone()));
    result
}

fn infer_overload_inner(heap: &Heap, sym: Symbol) -> Option<Vec<Sig>> {
    let Value::Fn(cid) = super::deps::obs_function(heap, sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    if closure.arms.len() < 2 {
        return None;
    }
    let self_name = closure.name;
    let mut arms: Vec<(Vec<Symbol>, Vec<Value>)> = Vec::with_capacity(closure.arms.len());
    for arm in closure.arms.iter() {
        // A rest/optional arm's parameters don't map 1:1 to argument positions, so it
        // can't be checked positionally — and one unreadable arm makes the *set*
        // unusable (the check needs every arm to reject before it warns).
        if !arm.optionals.is_empty() || arm.rest.is_some() {
            return None;
        }
        arms.push((arm.params.clone(), arm.body.clone()));
    }
    let base = match self_name {
        Some(name) => Ctx::default().with_inferring_self(name),
        None => Ctx::default(),
    };
    // Each arm's return under a scope in which a self-call returns `R`: an arm that
    // cannot be typed reads `any`, as before.
    let arm_rets = |scope: &Ctx| -> Vec<Ty> {
        arms.iter()
            .map(|(params, body)| {
                let mut ctx = scope.clone();
                for &p in params {
                    ctx = ctx.bind(p, Some(Ty::ANY));
                }
                body.last()
                    .and_then(|&tail| super::infer::with_fresh_depth(|| expr_ty(heap, tail, &ctx)))
                    .unwrap_or(Ty::ANY)
            })
            .collect()
    };
    // A self-call may land in any arm, so the ascent is over the UNION of the arms'
    // returns; each arm is then read once more under the settled `R`.
    let tails: Vec<Value> = arms
        .iter()
        .filter_map(|(_, body)| body.last().copied())
        .collect();
    let scope = match self_name.filter(|&n| tails.iter().any(|&t| form_mentions(heap, t, n))) {
        Some(name) => {
            let settled = self_ascent(heap, self_name, &tails, &base, |scope| {
                arm_rets(scope).into_iter().reduce(Ty::union)
            })
            .unwrap_or(Ty::ANY);
            base.bind(name, Some(Ty::arrow(Sig::variadic(Ty::ANY, settled))))
        }
        None => base.clone(),
    };
    let rets = arm_rets(&scope);
    let sigs = arms
        .iter()
        .zip(rets)
        .map(|((params, body), ret)| {
            Sig::new(param_domains(heap, body, params, &Ctx::default()), ret)
        })
        .collect();
    Some(sigs)
}

/// The same, read from a **form** rather than a loaded closure — the same-file
/// counterpart (`check_file` same-file inference), where the file being checked is not loaded.
pub(super) fn infer_overload_from_form(heap: &Heap, fn_form: Value, ctx: &Ctx) -> Option<Vec<Sig>> {
    let items = super::walk::list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    if !crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        return None;
    }
    let forms = &items[1..];
    let forms = match forms.first() {
        Some(Value::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    infer_overload_from_clauses(heap, forms, ctx)
}

/// The clause list of a multi-clause `fn`/`defn`, as one signature per clause.
///
/// Taken as a *slice of clauses* rather than a form because the surface `defn` is
/// where this fact is legible: a `:when`-guarded multi-clause definition **lowers**
/// to a single variadic `fn` over `%match` (ADR-226), so by the time the expanded
/// tree exists there are no clauses left to read — only a rest-list being
/// destructured. The un-expanded form still says exactly what each clause takes.
pub(super) fn infer_overload_from_clauses(
    heap: &Heap,
    clauses: &[Value],
    ctx: &Ctx,
) -> Option<Vec<Sig>> {
    let mut sigs = Vec::with_capacity(clauses.len());
    for &clause in clauses {
        // One unreadable arm makes the set unusable — a missing arm would make a call
        // "no arm admits" that the runtime accepts.
        let arm = clause_arm(heap, clause)?;
        // A variadic/optional arity head has no positional signature.
        if arm.heads.iter().any(|&h| {
            matches!(h, Value::Sym(s)
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST))
        }) {
            return None;
        }
        // Per position: a plain-symbol head is a parameter the body's demands can
        // narrow (`param_domains`), a pattern head is what the pattern itself admits
        // (`pattern_domain`). A position that is a pattern is given a fresh, unreferenced
        // name for the domain walk, so it reads `any` there and the pattern's domain is
        // what remains after the meet.
        let names: Vec<Symbol> = arm
            .heads
            .iter()
            .map(|&h| match h {
                Value::Sym(s) => s,
                _ => match value::gensym("pat") {
                    Value::Sym(g) => g,
                    _ => value::intern("%pattern-position"),
                },
            })
            .collect();
        let mut param_tys = param_domains(heap, &arm.body, &names, ctx);
        let pattern_tys: Vec<Ty> = arm.heads.iter().map(|&h| pattern_domain(heap, h)).collect();
        param_tys = meet(param_tys, pattern_tys);
        // A `:when` guard on the clause head (ADR-226) — `(params :when guard body…)`.
        // The clause runs only for arguments its guard admits, so that is exactly the
        // clause's domain, intersected with whatever its body demands. This is where a
        // guarded multi-clause `defn` gets its argument types from, with no annotation:
        // each clause contributes its guard, and a call no clause admits raises
        // `%match`'s no-match at run time.
        if let Some(g) = arm.guard {
            let scope = DomainScope {
                shadowed: HashSet::new(),
                aliases: HashMap::new(),
                thunks: HashMap::new(),
            };
            let (then_slice, _) = guard_slices(heap, g, &names, &scope, ctx);
            param_tys = meet(param_tys, then_slice);
        }
        // The arm's return: its tail typed with every binder in scope — a symbol head at
        // its domain, a pattern head's binders at what the pattern's domain implies for
        // them. `any` when the tail cannot be typed, as before (the params still count).
        let mut sub = ctx.clone();
        for (&h, ty) in arm.heads.iter().zip(&param_tys) {
            sub = bind_head(heap, sub, h, Some(ty.clone()));
        }
        let ret = arm
            .body
            .last()
            .and_then(|&tail| expr_ty(heap, tail, &sub))
            .unwrap_or(Ty::ANY);
        sigs.push(Sig::new(param_tys, ret));
    }
    (sigs.len() >= 2).then_some(sigs)
}

/// One clause of a multi-clause `fn`/`defn`, read from the UN-EXPANDED form:
/// `(heads [:when guard] body…)`, where each head is a plain parameter symbol or a
/// pattern (a literal, a list/vector pattern, `_`).
pub(super) struct ClauseArm {
    pub(super) heads: Vec<Value>,
    pub(super) guard: Option<Value>,
    pub(super) body: Vec<Value>,
}

/// Read one clause (see [`ClauseArm`]); `None` for a clause with no head list or no body.
pub(super) fn clause_arm(heap: &Heap, clause: Value) -> Option<ClauseArm> {
    let parts = list_items(heap, clause)?;
    let plist = *parts.first()?;
    let heads = match plist {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil => Vec::new(),
        _ => list_items(heap, plist)?,
    };
    let guarded = matches!(parts.get(1), Some(&Value::Keyword(k))
        if value::symbol_name_ref(k) == "when");
    let (guard, body): (Option<Value>, Vec<Value>) = if guarded {
        (parts.get(2).copied(), parts.get(3..)?.to_vec())
    } else {
        (None, parts.get(1..)?.to_vec())
    };
    if body.is_empty() {
        return None;
    }
    Some(ClauseArm { heads, guard, body })
}

/// What one clause-head PATTERN admits — the runtime test the `match` lowering emits for
/// it (`std/prelude/match.blsp`), read as a type: a binder or `_` admits anything; a
/// literal, its own type; a non-empty list pattern tests `pair?` (so never nil); the
/// empty list pattern tests `nil?`; a vector pattern tests `vector?`. An over-approximation
/// wherever it is not exact (an element pattern inside a list is not read), which is the
/// sound direction for a domain: a call outside it genuinely fails to match.
pub(super) fn pattern_domain(heap: &Heap, pat: Value) -> Ty {
    match pat {
        Value::Sym(_) => Ty::ANY,
        Value::Nil => Ty::of(Tag::Nil),
        Value::Pair(_) => match list_items(heap, pat) {
            // `(quote x)` / `(record …)` / `(or …)` heads carry match-language forms whose
            // domains are not read here — anything, soundly.
            Some(items)
                if matches!(items.first(), Some(Value::Sym(s))
                    if matches!(value::symbol_name_ref(*s), "quote" | "record" | "or" | "and" | "not" | "%pin" | "unquote")) =>
            {
                Ty::ANY
            }
            Some(_) => Ty::of(Tag::Pair),
            None => Ty::ANY,
        },
        Value::Vector(_) => Ty::of(Tag::Vector),
        other => Ty::of_value(other),
    }
}

/// Bring one clause head's binders into scope under the type its position holds: a symbol
/// head is bound to it directly; a pattern head's binders get what the pattern's shape
/// implies for them from that type (`walk::pattern_bindings` — element type for the
/// positional binders, a list of it for a `& rest`), or `None` where the type says nothing.
pub(super) fn bind_head(heap: &Heap, ctx: Ctx, head: Value, ty: Option<Ty>) -> Ctx {
    match head {
        Value::Sym(s) if value::symbol_name_ref(s) == "_" => ctx,
        Value::Sym(s) => ctx.bind(s, ty),
        Value::Pair(_) | Value::Vector(_) | Value::Nil => {
            let mut out = ctx;
            for (name, bound) in super::walk::pattern_bindings(heap, head, ty.as_ref()) {
                out = out.bind(name, bound);
            }
            out
        }
        _ => ctx,
    }
}

/// Bound on [`self_ascent`]: a scalar result settles in two or three rounds, a recursive
/// list builder in four (⊥, the base case, the first `cons`, the folded `μ`); the rest is
/// slack before the ascent is cut and the return read as unknown.
const MAX_SELF_ASCENT_ROUNDS: usize = 8;

/// The return of a body that calls ITSELF, typed in ONE question — a LOADED closure
/// ([`infer_sig`], [`infer_overload_of`]), where same-file inference's file-level fixpoint never runs.
///
/// `type_round` types the body (its tail, or every arm's) under the scope it is handed,
/// in which the function's own name is bound to the arrow `(any… -> R)`. The ascent starts
/// at `R = ⊥` and each round takes `R ∪ F(R)`, widened by the ADR-349/350 discipline Pass
/// 2.8 uses (a moving interval to its infinity, a nesting folded into a `μ`, a depth cut
/// past the early rounds); it stops when a round adds nothing — a post-fixpoint, so every
/// return at every recursion depth lies in `R` by induction on the depth — and reads the
/// return as `any` when the bound runs out, never the intermediate it was cut at.
///
/// Before this the single pass typed the self-call as ⊥ ("Kleene from ⊥", true only of
/// an ITERATED fixpoint) and kept the first round's answer: `(defn h (n) (if (= n 0) nil
/// (cons n (h (- n 1)))))` in a required module inferred `-> nil`, so `(nth (h 3) 2)`
/// warned "got nil", and `(when (nil? x) (self 0))` read as a diverging guard.
/// A body that does not mention its own name is typed once, exactly as before.
fn self_ascent(
    heap: &Heap,
    self_name: Option<Symbol>,
    bodies: &[Value],
    ctx: &Ctx,
    mut type_round: impl FnMut(&Ctx) -> Option<Ty>,
) -> Option<Ty> {
    // A parameter spelled like the function shadows it: the mention is the parameter.
    let Some(name) = self_name
        .filter(|&n| !ctx.is_lexical_local(n) && bodies.iter().any(|&b| form_mentions(heap, b, n)))
    else {
        return type_round(ctx);
    };
    let mut ret = Ty::NEVER;
    let mut older: Option<Ty> = None;
    for round in 0..MAX_SELF_ASCENT_ROUNDS {
        let scope = ctx
            .clone()
            .bind(name, Some(Ty::arrow(Sig::variadic(Ty::ANY, ret.clone()))));
        let mut next = type_round(&scope)?.union(ret.clone());
        next = next.widen_intervals_against(&ret);
        for prev in std::iter::once(&ret).chain(older.iter()) {
            if let Some(folded) = Ty::fold_recursive(prev, &next) {
                next = folded;
                break;
            }
        }
        if round + 1 >= MAX_SELF_ASCENT_ROUNDS / 2 {
            next = next.widened_below(WIDEN_DEPTH);
        }
        if next == ret {
            return Some(ret);
        }
        older = Some(std::mem::replace(&mut ret, next));
    }
    Some(Ty::ANY)
}

fn infer_sig_inner(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let Value::Fn(cid) = super::deps::obs_function(heap, sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    let self_name = closure.name;
    // A **complex** closure — multi-arity, or with optionals / a rest param — has no single
    // *parameter* signature to pin, but its RETURN is still the union of each arm's tail,
    // which flows to callers. Infer that (a params-less sig), since arity is checked
    // independently (`arity_of`) and a union of arm returns is a sound over-approximation —
    // a supertype can only *under*-flag a caller, never false-positive.
    // A rest binder is fine (its demand becomes a per-argument one, as in
    // `infer_params_from_form`); optionals are not — a position that may be unfilled has no
    // checkable demand.
    let simple = closure.arms.len() == 1 && closure.arms[0].optionals.is_empty();
    if !simple {
        return infer_return_only(heap, cid, self_name);
    }
    let arm = &closure.arms[0];
    if arm.body.is_empty() {
        return None;
    }
    // Copy out before we ask `sig_of` / `expr_ty` (which borrow the heap again).
    let params: Vec<Symbol> = arm.params.clone();
    let rest_binder: Option<Symbol> = arm.rest;

    // Every parameter's **domain** across the whole body — the union over the body's
    // possible executions, each branch credited only within the guard that selects it
    // (see [`param_domains`]). Params nothing constrains stay `ANY`, recovering the
    // return-only behaviour exactly.
    //
    // No shortcut for a body that is one call: the domain walk subsumes it (a direct
    // parameter argument takes the callee's demand at that position, a nested one is
    // walked), and a shortcut reading only direct arguments would make the same function
    // constrain its callers differently here than in its own file.
    let mut names = params.clone();
    names.extend(rest_binder);
    let mut param_tys = param_domains(heap, &arm.body, &names, &Ctx::default());
    // A typed parameter (ADR-420) bounds the demand too, as in `infer_return_from_form`:
    // `(defn f (s:string) … (count s) …)` demands only a countable, and without the meet a
    // cross-module caller read `f`'s open result as widened past the `string` it returns.
    if let Some(declared) = declared_heap_sig(heap, sym) {
        for (ty, decl) in param_tys.iter_mut().zip(&declared.params) {
            *ty = ty.clone().intersect(decl.clone());
        }
    }
    let rest = match rest_binder {
        Some(_) => rest_element_demand(&param_tys.pop()?),
        None => None,
    };
    let demands = ParamDemands {
        params: param_tys,
        optional: Vec::new(),
        rest,
    };

    // The return: read the body tail's type under the parameter demands.
    let tail = *arm.body.last()?;
    // Mark the ctx as inferring `self_name`, so a self-recursive call in a branch result is
    // skipped in the return union (see `infer::branch_union`) — this is what lets a
    // tail-recursive function's return infer from its base cases instead of deferring.
    let mut ctx = match self_name {
        Some(name) => Ctx::default().with_inferring_self(name),
        None => Ctx::default(),
    };
    // Each parameter is bound to its DEMAND for the return inference, not to `any`: the
    // demand is what every call that reaches the body's tail satisfied, so the tail's
    // type under it is the return type of every successful call — `(fold + x xs)` with
    // `x` demanded numeric is numeric, not `any`. A call outside the demand raises before
    // it returns, so nothing is claimed about it.
    for (index, &p) in names.iter().enumerate() {
        let bound = if index < demands.params.len() {
            demands.params[index].clone()
        } else {
            Ty::ANY // the rest binder holds a list; its demand was on the elements
        };
        ctx = ctx.bind(p, Some(bound));
    }
    let informative = demands.params.iter().any(|t| *t != Ty::ANY) || demands.rest.is_some();
    let typed = self_ascent(heap, self_name, &[tail], &ctx, |scope| {
        super::infer::with_fresh_depth(|| expr_ty(heap, tail, scope))
    });
    match typed {
        Some(ret) => Some(demands.into_sig(ret)),
        // The return couldn't be inferred (e.g. the body calls an ability op whose facts aren't
        // on this bare ctx). Still surface the parameter demands with an `ANY` return when a
        // param is actually constrained (ADR-190), so a *cross-file* caller's arguments are
        // checked — sound, since the demands under-constrain. A wholly-unconstrained param set
        // stays deferred (`None`), preserving the prior return-only behaviour exactly.
        None if informative => Some(demands.into_sig(Ty::ANY)),
        None => None,
    }
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
    // Every arm is typed in the same round, under the same `R` for the self-call: a
    // recursive call may land in any arm of its arity.
    let base = match self_name {
        Some(name) => Ctx::default().with_inferring_self(name),
        None => Ctx::default(),
    };
    let tails: Vec<Value> = arms.iter().map(|(_, tail)| *tail).collect();
    let ret = self_ascent(heap, self_name, &tails, &base, |scope| {
        let mut ret: Option<Ty> = None;
        for (binders, tail) in &arms {
            let mut ctx = scope.clone();
            for &p in binders {
                ctx = ctx.bind(p, Some(Ty::ANY));
            }
            let t = super::infer::with_fresh_depth(|| expr_ty(heap, *tail, &ctx))?;
            ret = Some(match ret {
                Some(a) => a.union(t),
                None => t,
            });
        }
        ret
    })?;
    Some(Sig::new(vec![], ret))
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
    // A single-clause fn's parameters are bound to their DEMANDS (see `infer_sig_inner`'s
    // Tier 2 for why that is the return type of every successful call); a multi-clause
    // fn's arms have no single demand vector and stay at `any`. The rest binder holds a
    // list — its demand was on the elements — so it is `any` here too.
    let demands = (arms.len() == 1)
        .then(|| infer_params_from_form(heap, fn_form, base_ctx))
        .flatten()
        .map(|d| d.params)
        .unwrap_or_default();
    let mut ret: Option<Ty> = None;
    for (binders, tail) in &arms {
        let mut ctx = match self_name {
            Some(n) => base_ctx.with_inferring_self(n),
            None => base_ctx.clone(),
        };
        // A module-private function's parameters are also bounded by what its callers
        // hand it (the caller-derived parameter pass, ADR-341): the MEET of the demand and the caller-derived type
        // over-approximates every successful call's argument, so the tail under it is still
        // the return of every successful call — and `(+ i 1)` under `i : int` is `int`, not
        // the `number` the demand alone says.
        let derived = self_name.and_then(|n| base_ctx.derived_params(n));
        // …and by its own declaration: `(defn- f (w:string) (count w))` demands only a
        // countable, and a helper passed as a value has no caller-derived types, so without
        // this its `w` read as `countable` and its return widened past the `string` it was
        // declared over. A call outside the declaration is flagged at the call.
        let declared = self_name
            .filter(|_| arms.len() == 1)
            .and_then(|n| base_ctx.declared_sig(n));
        for (index, &p) in binders.iter().enumerate() {
            let mut bound = demands.get(index).cloned().unwrap_or(Ty::ANY);
            if let Some(from_callers) = derived.and_then(|d| d.get(index)).and_then(Option::as_ref)
            {
                bound = bound.intersect(from_callers.clone());
            }
            if let Some(declared) = declared.as_ref().and_then(|s| s.params.get(index)) {
                bound = bound.intersect(declared.clone());
            }
            ctx = ctx.bind(p, Some(bound));
        }
        if let Some(n) = self_name {
            ctx = ctx.with_derived_count_aliases(n, binders);
        }
        let t = expr_ty(heap, *tail, &ctx)?;
        ret = Some(match ret {
            Some(a) => a.union(t),
            None => t,
        });
    }
    ret
}

/// A single-arity file function's inferred **parameter domains**, read from its
/// `(fn (params) body…)` form (ADR-190) — the set of values each parameter can be called
/// with (see [`param_domains`]). `None` for a multi-arity / malformed / no-param fn (no
/// single parameter list to describe). **Sound for caller-flagging:** the domain
/// over-approximates (a superset of the true valid-argument type), so an argument disjoint
/// from it is disjoint from the truth too — it genuinely errors at runtime, never a false
/// positive. The companion of [`infer_return_from_form`] (which yields the return).
/// What a function's body demands of its parameters, position by position — plus, for a
/// `& rest` binder, what it demands of EACH rest argument. A rest binder collects the
/// tail of the call into a list, so a demand on the binder is a demand on that list; its
/// element type is the per-argument one ([`Sig::rest`]), which is what a call site can
/// check. `(defn foo (x y & more) (+ (fold + x more) y))` reads
/// `(numeric numeric & numeric -> …)`.
pub(super) struct ParamDemands {
    pub params: Vec<Ty>,
    /// The `&optional` positions' demands — each already widened by its DEFAULT's type,
    /// since an omitted argument binds the default and the body runs over it all the same.
    pub optional: Vec<Ty>,
    pub rest: Option<Ty>,
}

impl ParamDemands {
    /// The `Sig` these demands make with return `ret`.
    pub fn into_sig(self, ret: Ty) -> Sig {
        let mut sig = match self.rest {
            Some(rest) => Sig::with_rest(self.params, rest, ret),
            None => Sig::new(self.params, ret),
        };
        sig.optional = self.optional;
        sig
    }
}

/// The per-argument demand a demand on a rest BINDER implies: the element type of the
/// list the binder collects. `None` when the body says nothing about the elements.
fn rest_element_demand(binder_demand: &Ty) -> Option<Ty> {
    if binder_demand.is_any() {
        return None;
    }
    binder_demand.elem_ty().filter(|e| !e.is_any())
}

/// A seqable whose every element is an `elem` — what a collection argument must be for a
/// callback that demands `elem` of each element. The plain `seqable` when `elem` is `any`;
/// otherwise the three element-carrying kinds plus the empty list, plus `bytes` when an
/// int element would do (bytes yield ints) and `map` when a 2-tuple would (a map yields
/// its entries). A demand must over-approximate, so every kind that could satisfy the
/// callback stays in.
fn seqable_of(elem: Ty) -> Ty {
    if elem.is_any() {
        return Ty::SEQABLE;
    }
    let mut t = Ty::list_of(elem.clone())
        .union(Ty::vector_of(elem.clone()))
        .union(Ty::set_of(elem.clone()))
        .union(Ty::of(Tag::Nil));
    if !elem.is_disjoint(&Ty::of(Tag::Int)) {
        t = t.union(Ty::of(Tag::Bytes));
    }
    if !elem.is_disjoint(&Ty::of(Tag::Vector)) {
        t = t.union(Ty::of(Tag::Map));
    }
    t
}

pub(super) fn infer_params_from_form(
    heap: &Heap,
    fn_form: Value,
    ctx: &Ctx,
) -> Option<ParamDemands> {
    let items = super::walk::list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        return None; // params vary per clause — no single demand to store
    }
    let plist = *items.get(1)?;
    // A `& rest` fn binds its fixed parameters positionally exactly as a plain fn does,
    // and the rest binder's demand becomes a per-argument one through
    // `rest_element_demand`. An `&optional` position constrains only an argument that IS
    // passed — an omitted one binds its default — so its demand is the body's domain
    // widened by the default's type (`nil` with none): the body runs over that value too,
    // so it is in the domain by construction.
    let raw = match plist {
        Value::Vector(id) => heap.vector(id).to_vec(),
        _ => super::walk::list_items(heap, plist).unwrap_or_default(),
    };
    let optional_at = raw
        .iter()
        .position(|&it| matches!(it, Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL)));
    let rest_marker_at = raw.iter().position(|&it| {
        matches!(it, Value::Sym(s)
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST))
    });
    // The optional binders' defaults, in order: `(name default)`, or a bare `name`.
    let optional_defaults: Vec<Option<Value>> = match optional_at {
        Some(at) => raw[at + 1..rest_marker_at.unwrap_or(raw.len())]
            .iter()
            .map(|&item| match item {
                Value::Sym(_) => Some(None),
                Value::Pair(_) | Value::Vector(_) => {
                    let inner = match item {
                        Value::Vector(id) => heap.vector(id).to_vec(),
                        _ => super::walk::list_items(heap, item).unwrap_or_default(),
                    };
                    match inner.as_slice() {
                        [Value::Sym(_)] => Some(None),
                        [Value::Sym(_), default] => Some(Some(*default)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect::<Option<_>>()?,
        None => Vec::new(),
    };
    // The count of positions before the rest binder: required ones and optional ones.
    let rest_at = rest_marker_at.map(|at| match optional_at {
        Some(_) => at - 1,
        None => at,
    });
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
    let mut demands = param_domains(heap, &body, &params, ctx);
    // `fn_params` lists the rest binder last; a `&` with no binder after it leaves the
    // count at the fixed parameters and nothing to split off.
    let rest = match rest_at {
        Some(fixed) if demands.len() == fixed + 1 => rest_element_demand(&demands.pop()?),
        _ => None,
    };
    let required = demands.len().checked_sub(optional_defaults.len())?;
    let optional = demands
        .split_off(required)
        .into_iter()
        .zip(&optional_defaults)
        .map(|(domain, default)| {
            if domain.is_any() {
                return domain;
            }
            let default_ty = match default {
                Some(form) => expr_ty(heap, *form, &Ctx::default()).unwrap_or(Ty::ANY),
                None => Ty::of(Tag::Nil),
            };
            domain.union(default_ty)
        })
        .collect();
    Some(ParamDemands {
        params: demands,
        optional,
        rest,
    })
}

/// The [`SigWithVars`] a loaded module declared for `sym`, when its declaration carries
/// type variables — the live-image counterpart of `Ctx::declared_sig_with_vars`.
pub(super) fn declared_heap_sig_with_vars(
    heap: &Heap,
    sym: Symbol,
) -> Option<super::ctx::SigWithVars> {
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    annot::in_declaring_scope(sym, || annot::parse_arrow_type_with_vars(heap, type_value))
}

/// A **user-declared** signature for `sym` — the `(sig name (A -> B))` the author wrote,
/// recorded on the heap (keyed by the module-qualified global) by `%register-sig` when
/// the `(sig …)` form evaluated at load time. Only an arrow type-expr yields a caller
/// sig (a value `(sig x int)` is [`declared_heap_value_ty`]'s). The file-local
/// `ctx.declared_sig` still wins ahead of this for the file being checked; this is the
/// store that makes a declaration authoritative for a qualified intra-module call or a
/// cross-module caller.
pub(super) fn declared_heap_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    annot::in_declaring_scope(sym, || annot::parse_type(heap, type_value))?
        .as_arrow()
        .cloned()
}

/// A loaded declaration whose result is open — `(T… -> _)`, a typed parameter list with no
/// `-> TYPE` (ADR-420) — takes its result from inference, so typing a parameter never costs a
/// cross-module caller the return it read before.
fn with_open_ret_inferred(heap: &Heap, sym: Symbol, mut sig: Sig) -> Sig {
    let open = super::deps::obs_declared_sig_value(heap, sym)
        .is_some_and(|form| annot::arrow_ret_is_open(heap, form));
    if open && let Some(inferred) = infer_sig(heap, sym) {
        sig.ret = inferred.ret;
    }
    sig
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
    annot::in_declaring_scope(sym, || annot::parse_type(heap, type_value))?
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
    let ty = annot::in_declaring_scope(sym, || annot::parse_type(heap, type_value))?;
    if ty.as_arrow().is_some() {
        return None;
    }
    Some(ty)
}

/// The signature for `sym`. The sources, in the order they are consulted — the one place
/// the precedence is stated:
///
/// 1. **operator** ([`operator_sig`]) — `+ - * / < <= > >=` and the extrema, their domain
///    read off the multimethod registry (ADR-299) rather than the native's widest reading;
/// 2. **declared** ([`declared_heap_sig`]) — a loaded module's `(sig …)`, authoritative
///    over everything below (a body inferring `number` the author declared `int`);
/// 3. **primitive** ([`primitive_sig`]) — a native's own `Sig`;
/// 4. **image** ([`image_heap_sig`]) — a std declaration read without loading its module,
///    so a check that never loads `math` reads what one that did would (ADR-370). After
///    the primitives because a native is never in the index and is asked about most;
/// 5. **curated** ([`curated_sig`]) — only names no source above types;
/// 6. **inferred** ([`infer_sig`]) — one step deep from a loaded body.
///
/// The non-inferring sources are exposed separately so [`infer_sig`] can consult a
/// callee's sig without starting another inference. (The checked file's own `(sig …)`
/// and inferred signatures live on `Ctx` and are read by the walk ahead of all of these.)
pub(super) fn sig_of(heap: &Heap, sym: Symbol) -> Option<Sig> {
    operator_sig(heap, sym)
        .or_else(|| declared_heap_sig(heap, sym).map(|sig| with_open_ret_inferred(heap, sym, sig)))
        .or_else(|| primitive_sig(heap, sym))
        .or_else(|| image_heap_sig(heap, sym))
        .or_else(|| curated_sig(sym))
        .or_else(|| infer_sig(heap, sym))
}

/// The signature index's declaration for `sym` (ADR-370): a std function's `(sig …)`,
/// when the checker can read it as-is (`check::image_carried_sig`) — declarations only,
/// never an inferred signature. The image footer caches the index; a process booted
/// without an image scans the one module the question is about on demand, so the answer
/// is the same either way. Consulted before inferring a body, so a std callee's type is
/// known without its module in the heap; `None` for an undeclared function or under
/// `BROOD_NO_IMAGE_SIGS=1`.
pub(super) fn image_heap_sig(heap: &Heap, sym: Symbol) -> Option<Sig> {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};
    // Parsed once per process per name, and keyed by the name alone: the index is a fact
    // about the std sources embedded in THIS binary (its stdlib id is a build constant),
    // `BROOD_NO_IMAGE_SIGS` is read once per process, and the parse runs with every
    // per-file table empty — so a name's answer cannot change within a process. A name
    // with no declaration caches its `None` too.
    static CACHE: OnceLock<RwLock<HashMap<Symbol, Option<Sig>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(hit) = cache.read().unwrap_or_else(|e| e.into_inner()).get(&sym) {
        return hit.clone();
    }
    let parsed = with_image_sig_form(heap, sym, |scratch, form| {
        annot::parse_type(scratch, form)?.as_arrow().cloned()
    });
    cache
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(sym, parsed.clone());
    parsed
}

/// The image's signature for `sym` when its declaration carries type variables — the
/// footer's counterpart of [`declared_heap_sig_with_vars`], resolved per call from the
/// arguments exactly as the loaded declaration is. Before this reader existed a
/// variable-bearing declaration could not ride at all: its flat reading has an `any`
/// return (`?A` parses to `any`), which is "no declaration" to the call-site typing.
pub(super) fn image_heap_sig_with_vars(
    heap: &Heap,
    sym: Symbol,
) -> Option<super::ctx::SigWithVars> {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};
    // Keyed by the name alone for the reason `image_heap_sig`'s cache is.
    static CACHE: OnceLock<RwLock<HashMap<Symbol, Option<super::ctx::SigWithVars>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(hit) = cache.read().unwrap_or_else(|e| e.into_inner()).get(&sym) {
        return hit.clone();
    }
    let parsed = with_image_sig_form(heap, sym, |scratch, form| {
        annot::parse_arrow_type_with_vars(scratch, form)
    });
    cache
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(sym, parsed.clone());
    parsed
}

/// Read the footer's signature text for `sym` into a form and hand it to `read`. The text
/// is read into a form on a SCRATCH heap of this thread — `sig_of` has only a shared
/// borrow of the caller's — sharing the prelude and runtime regions, so the type parser
/// resolves record and alias names exactly as it would on the caller's heap. The scratch
/// forms are never collected; there are at most as many as the footer has names. `read`
/// runs with every per-file table empty — the reading the writer proved self-contained
/// (`check::image_carried_sig`), so the checked file's own aliases cannot bend it.
fn with_image_sig_form<T>(
    heap: &Heap,
    sym: Symbol,
    read: impl FnOnce(&mut Heap, Value) -> Option<T>,
) -> Option<T> {
    let text = crate::eval::derive::image_sig_text(heap, sym)?;
    thread_local! {
        static SCRATCH: std::cell::RefCell<Option<Heap>> = const { std::cell::RefCell::new(None) };
    }
    SCRATCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        let scratch = slot.get_or_insert_with(|| {
            let mut h = Heap::with_regions(heap.prelude_arc(), heap.runtime_arc());
            h.set_global(crate::core::value::EnvId::GLOBAL);
            h
        });
        let form = crate::syntax::reader::read_one(scratch, &text).ok()?;
        annot::without_tables(|| read(scratch, form))
    })
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
    let Some(global) = super::deps::obs_function(heap, sym) else {
        // Not in the heap — an imaged module's function the check did not load (ADR-370):
        // the arity the image recorded from the live closure at build time (the signature
        // TEXT is not it: an inferred sig carries no rest parameter).
        let (min, max) = crate::eval::derive::image_sig_arity(heap, sym)?;
        return Some(Arity { min, max });
    };
    match global {
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

/// The callee's parameter NAMES, rendered `coll` / `acc x` / `coll x & more`, or `None`
/// when the name resolves to neither a primitive nor a closure. Mirrors [`arity_of`]'s
/// lookup so the checker's arity warning can name what is missing, exactly as the runtime
/// error does.
pub(super) fn param_names_of(heap: &Heap, sym: Symbol) -> Option<String> {
    match super::deps::obs_function(heap, sym)? {
        Value::Native(id) => {
            let p = heap.native(id).params;
            (!p.is_empty()).then(|| p.join(" "))
        }
        Value::Fn(cid) => {
            // The arms differ; name the FIRST, which is the one a reader sees at the top
            // of the definition. A multi-arm callee's real contract is its arity list,
            // which the message already carries.
            let c = heap.closure(cid);
            let text = c.arms.first()?.source_param_text();
            (!text.is_empty()).then_some(text)
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

#[cfg(test)]
mod convention_tests {
    use super::*;

    /// **The data-first convention, machine-checked (ADR-308).**
    ///
    /// A combinator's argument positions belong in exactly ONE place — its curated
    /// signature. The checker's structural reasoning restates them: `infer.rs` reads the
    /// collection and callback out of a call to type the result, `walk.rs` synthesises a
    /// callback signature from the collection's element type, and `specialize_call` below
    /// pushes callback demand onto parameters. During ADR-308 those hard-coded indices
    /// went stale against the signatures, and the failure was **silent** — inference
    /// degraded to `any` rather than erroring, so exactly one test in 5300 noticed.
    ///
    /// This asserts the invariant all of those sites assume, so the next reorder fails
    /// here, by name, instead of quietly losing type precision:
    ///
    ///   in every curated signature that takes a callback, the callback is the LAST
    ///   fixed parameter — the collection comes first.
    #[test]
    fn curated_combinators_put_the_callback_last() {
        let mut offenders: Vec<String> = Vec::new();
        for (sym, sig) in CURATED_SIGS.iter() {
            if sig.params.len() < 2 {
                continue;
            }
            let arrows: Vec<usize> = sig
                .params
                .iter()
                .enumerate()
                .filter(|(_, t)| t.as_arrow().is_some())
                .map(|(i, _)| i)
                .collect();
            let Some(&first_arrow) = arrows.first() else {
                continue;
            };
            let last = sig.params.len() - 1;
            if first_arrow != last {
                offenders.push(format!(
                    "{}: callback at param {first_arrow} of {}, expected last ({last})",
                    crate::core::value::symbol_name(*sym),
                    sig.params.len()
                ));
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "curated signatures must be data-first — the callback is the last parameter \
             (ADR-308). Offenders:\n  {}\n\nIf you are deliberately reordering, update \
             `infer.rs` (result inference), `walk.rs` (callback synthesis) and \
             `specialize_call` together — they read these positions.",
            offenders.join("\n  ")
        );
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    /// The contract every cycle break rests on: a refused `enter` must leave the in-flight
    /// entry alone. The regression: `bool::then_some(InferGuard(sym))` built the guard on the
    /// refusal path too, and dropping it removed the *outer* inference's mark — so the very
    /// next reference re-entered the cycle, without bound (54 GB on a `nest run`).
    #[test]
    fn a_refused_enter_does_not_release_the_in_flight_mark() {
        let sym = crate::core::value::intern("guard-test/in-flight");
        let outer = InferGuard::enter(sym).expect("first entry is admitted");
        assert!(
            InferGuard::enter(sym).is_none(),
            "re-entry is a cycle: refused"
        );
        // Refusing must not have un-marked `sym` — a THIRD attempt is refused too.
        assert!(
            InferGuard::enter(sym).is_none(),
            "the refusal released the in-flight mark (the eager-drop bug)"
        );
        assert!(INFERRING.with(|s| s.borrow().contains(&sym)));
        drop(outer);
        assert!(!INFERRING.with(|s| s.borrow().contains(&sym)));
        assert!(
            InferGuard::enter(sym).is_some(),
            "released once the real guard drops"
        );
    }

    /// The depth cap refuses without touching any mark either.
    #[test]
    fn the_depth_cap_refuses_without_releasing_anything() {
        let guards: Vec<InferGuard> = (0..MAX_INFER_DEPTH)
            .map(|i| {
                InferGuard::enter(crate::core::value::intern(&format!("guard-test/depth-{i}")))
                    .expect("under the cap")
            })
            .collect();
        let extra = crate::core::value::intern("guard-test/over-the-cap");
        assert!(InferGuard::enter(extra).is_none(), "at the cap: refused");
        assert_eq!(
            INFERRING.with(|s| s.borrow().len()),
            MAX_INFER_DEPTH,
            "cap refusal left every mark"
        );
        drop(guards);
        assert_eq!(INFERRING.with(|s| s.borrow().len()), 0);
    }
}

/// The call-site signature of a **`fn` literal bound to a `let` name**: each parameter's
/// domain over the literal's body ([`param_domains`], the ADR-261 rule a `defn` gets) and
/// the literal's own result (`literal_ty`, the arrow `check_let` already typed). `(let (g (fn (b) (+ 1 b))) (g "x"))` is then a warning, where the
/// literal's own arrow type — `(any -> number)`, see `infer::lambda_arrow` — only typed the
/// result. The domain must NOT go into that arrow: an arrow's parameters are contravariant,
/// and a `(number -> number)` handed to an inferred `(any -> any)` callback slot would read
/// as a mismatch when the slot's `any` means "unknown". So this is a per-NAME fact consulted
/// where the name heads a call, exactly as a same-file `defn`'s inferred sig is, and it is
/// dropped with the binding (`Ctx::bind` shadows it). Single-clause, plain-symbol
/// parameters only; `None` otherwise. An all-`any` signature is still returned: it carries the
/// literal's ARITY, which the arrow cannot when the result is unknown (`lambda_arrow`
/// declines then), so `(g 1 2)` on a one-parameter `g` is reported either way.
pub(super) fn let_bound_lambda_sig(
    heap: &Heap,
    form: Value,
    literal_ty: Option<&Ty>,
    ctx: &Ctx,
) -> Option<Sig> {
    let items = super::walk::fn_form_items(heap, form)?;
    let parts = &items[1..];
    if parts.len() < 2 {
        return None;
    }
    let params: Vec<Symbol> = list_items(heap, parts[0])?
        .iter()
        .map(|p| match p {
            Value::Sym(s) if !value::symbol_name_ref(*s).starts_with('&') => Some(*s),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let domains = param_domains(heap, &parts[1..], &params, ctx);
    let ret = literal_ty
        .and_then(|t| t.as_arrow().map(|a| a.ret.clone()))
        .unwrap_or(Ty::ANY);
    Some(Sig::new(domains, ret))
}

/// The arrow type of a **bare global function reference** handed as an argument to a call
/// being specialized — `inc` in `(my-map xs inc)` — or `None` for anything else.
///
/// A function-valued global has NO type as a value (`infer::global_value_ty` declines it:
/// a redefinable global is `dynamic()`, and an arrow in value position would be checked
/// contravariantly against whatever slot it lands in). It does have a signature, and
/// inside the specialized body the parameter it binds heads calls — `(f (first xs))` —
/// where the arrow's RESULT is exactly what a direct `(inc …)` would be typed as: an
/// over-approximation of what the function returns from any call that succeeds, dynamic
/// under reload the same way. Only `expr_ty` runs over a specialized body, so the arrow
/// is read for its result and never checked; that is what keeps this sound where typing
/// the reference itself would not be. Same-file names read the file's tables (the file
/// is not loaded); a lexical local shadowing the name is not the global.
fn global_fn_arrow(heap: &Heap, arg: Value, ctx: &Ctx) -> Option<Ty> {
    let Value::Sym(s) = arg else {
        return None;
    };
    if ctx.is_lexical_local(s) {
        return None;
    }
    let sig = ctx
        .declared_sig(s)
        .or_else(|| ctx.inferred_fn_sig(s))
        .or_else(|| (!ctx.is_file_global(s)).then(|| sig_of(heap, s)).flatten())?;
    Some(Ty::arrow(sig))
}

/// Every curated name — for the construction gate that holds the curated skip sound
/// (`check/tests/image_sigs.rs::a_curated_name_reads_the_same_loaded_or_not`) and for
/// `tests/curated_sigs_exist.rs`, which once found the keys by string-splitting this file.
pub(super) fn curated_names() -> Vec<Symbol> {
    CURATED_SIGS.keys().copied().collect()
}
