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
    // Maps are seqable in the stdlib (`seq`/`fold` coerce them via `%map-pairs`),
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
    // `count` accepts a string, map, bytes, or sequence (the prelude
    // `count` dispatches string?/map?/else-fold, and bytes counts its octets) —
    // but not a number/keyword/etc. Rope and Table joined in ADR-253: both have an
    // O(1) size and both used to raise. A signature narrower than the function is a
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
    // Variadic arithmetic. A numeric arg — or a `Num` RECORD, which the kernel's
    // `%add`/`%sub`/`%mul`/`%div` fallback dispatches to the `num/*` multimethods (ADR-172
    // §7, ADR-179) — so `(+ money money)` is legal. These curated entries are the widest
    // reading, `number | map`; they are SHADOWED by `operator_sig` (ADR-299), which reads
    // the domain off the registry — `number` plus exactly the records with methods — and
    // is what every lookup consults first. The RESULT of a pure-numeric call is typed
    // precisely by `numeric_call_ty` (int/float) either way.
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
    put("math/mod", Sig::new(vec![int, int], int));
    // Common helpers the checker can't infer (branchy / nested-param bodies),
    // hand-vetted against std/prelude.blsp — same soundness basis as the rest of
    // this table. Conservative on the domain (widest type the body accepts) so a
    // tighter type never false-positives:
    //   even?/odd? — body reduces via `rem`/`=`; require a number → bool.
    //   abs        — `(if (< n 0) (- n) n)`: numeric in and out.
    //   not/math/zero?  — accept any value (truthiness / dispatch), but pin the `bool`
    //                result, so a non-bool sink like `(+ 1 (not x))` is catchable.
    //   count      — a string, map, or sequence → int.
    //
    // **Keyed QUALIFIED for the three ADR-227 moved out of the prelude.** An entry here
    // does more than supply a type: a name in this table is one the checker treats as
    // *known*, which suppressed the unbound lint. So after `even?`/`odd?`/`abs` moved into
    // `std/math.blsp`, `nest check` stayed silent on a bare `(even? 4)` — no import, unbound
    // at runtime — and reported a *type* warning on `(even? "x")` instead of the truth. Its
    // sibling moves that were never curated (`sum`, `frequencies`) correctly said "unbound
    // symbol", which is what pinned the cause. `nest check` is the gate that exits nonzero on
    // any warning, so this let a program that dies on an unbound symbol pass CI.
    //
    // Keyed as `math/…`, all three cases come out right: a bare use with no import draws the
    // unbound lint, a qualified `math/abs` gets the vetted signature (it previously got
    // nothing — lookup is by exact symbol), and a `(:use math)` bare use is known via the
    // module's exports and simply carries no curated sig, exactly like every other module
    // function. If these are ever wanted for the `:use`d bare spelling too, the fix is a
    // bare→owning-module index, NOT re-adding the bare key — that would restore the masking.
    put("math/even?", Sig::new(vec![num], bool_ty));
    put("math/odd?", Sig::new(vec![num], bool_ty));
    put("math/abs", Sig::new(vec![num], num));
    put("not", Sig::new(vec![any], bool_ty));
    put("math/zero?", Sig::new(vec![any], bool_ty));
    put("math/nan?", Sig::new(vec![any], bool_ty));
    put("math/infinite?", Sig::new(vec![any], bool_ty));
    // `min`/`max`/`rem`/`quot`/`mod`/`->fixed`/`floor` moved to `math` on 2026-08-27, and
    // their kernel halves took the `%` prefix (`%max`, `%rem`, …) so the PRELUDE can do
    // arithmetic without loading a module. The registered Sig therefore sits on the `%`
    // name, which user code never writes — these entries put it back on the name that IS
    // written, so `(math/min "a" 2)` is still flagged.
    // `math/max`/`math/min` are NOT here: they take the record-aware signature further
    // down (they route through `compare-to`, ADR-179), and a duplicate `put` would just
    // let source order decide which one the checker used.
    put("math/rem", Sig::new(vec![int, int], int));
    put("math/quot", Sig::new(vec![int, int], int));
    put("math/floor", Sig::new(vec![num], int));
    put("math/->fixed", Sig::new(vec![num, int], str_ty));
    put("math/numerator", Sig::new(vec![num], int));
    put("math/denominator", Sig::new(vec![num], int));
    put("math/rational", Sig::new(vec![num, num], num));
    put("reflect/read-string", Sig::new(vec![str_ty], any));
    put("reflect/read-all", Sig::new(vec![str_ty], any));
    put("reflect/read-first", Sig::new(vec![str_ty], any));
    put("count", Sig::new(vec![countable], int));
    // NO `length` entry: there is no `length` function, and there never was. It was added
    // 2026-05-31 alongside `count` as if it were an alias ("each vetted against
    // std/prelude.blsp" — this one was not), and because a curated entry marks its name
    // *known* and so suppresses the unbound lint, `nest check` accepted `(length x)` in
    // silence for months while the runtime raised `unbound symbol: length`. The string case
    // is `string-length`, which has its own entry; sequences use `count`.
    // Output fns: io/puts / io/write are Brood closures with rest params,
    // so infer_sig bails — pin their nil result so `(+ 1 (io/puts x))` is caught.
    for n in ["io/puts", "io/write"] {
        put(n, Sig::variadic(any, nil_ty));
    }
    // min/max: at least one number-or-`Ord`-record arg (fixed) plus a variadic rest of the
    // same → same domain (they route through `compare-to` for records, ADR-179).
    // Variadic via rest; infer_sig bails on rest-param closures, so curate.
    // Module-qualified since the math wave: bare `min`/`max` do not exist. Curating them
    // under the old names made the checker vouch for names the runtime had moved, so
    // `(max 1 2)` in ordinary code checked clean and raised at run time — which is how
    // hive shipped a broken `clamp-limit`.
    for n in ["math/min", "math/max"] {
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
    // `member?` used to be curated here. It no longer exists — it became `includes?`,
    // with the arguments the other way round — and the entry is NOT remapped, because a
    // signature under a name nothing defines is worse than none: the checker treats a
    // curated name as known, so it stopped reporting `member?` as unbound at all.
    put(
        "contains?",
        Sig::new(vec![Ty::of_tags(&[Tag::Map, Tag::Set]), any], bool_ty),
    );
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
    //   join           — complex if/apply body; always returns a string.
    //   capitalize     — if-branches, both arms produce strings.
    //   string-split   — accumulator recursion; returns a list of strings
    //                    (unrefined list — list<string> would warn on (first …) = nil).
    put("string/join", Sig::new(vec![any, seq], str_ty));
    put("string/capitalize", Sig::new(vec![str_ty], str_ty));
    // `->string` renders ANY value as text (ADR-258's rename of `name`): always a string.
    put("->string", Sig::new(vec![Ty::ANY], str_ty));
    // Equality: `=`/`not=` are multi-arm closures; infer_sig bails on multi-arm.
    // Pin the bool result so `(+ 1 (= x y))` is caught.
    for n in ["=", "not="] {
        put(n, Sig::variadic(any, bool_ty));
    }
    // String conversions: branchy bodies or `apply` — infer_sig bails.
    //   string/->symbol — if-guard over (string? s).
    put("string/->symbol", Sig::new(vec![str_ty], sym_ty));
    // String predicates: nested calls or let+branch bodies.
    //   starts-with?/ends-with? — let + and + branch.
    //   blank?                  — let + cond recursion.
    for n in ["string/starts-with?", "string/ends-with?"] {
        put(n, Sig::new(vec![str_ty, str_ty], bool_ty));
    }
    //   includes? — polymorphic membership `(>= (index-of coll x) 0)` over
    //   list/vector/string (string/substring) and map (values); both args stay `any`.
    put("includes?", Sig::new(vec![any, any], bool_ty));
    put("string/blank?", Sig::new(vec![str_ty], bool_ty));
    // String transforms: all call recursive helpers or use `apply`; infer_sig bails.
    //   trim/triml/trimr   — call tail-recursive aux helpers.
    //   replace            — if-branch over join/string-split.
    //   string-repeat      — (apply str (repeat n s)).
    //   pad-left/pad-right — let + if.
    //   char-at            — (string/substring s i (inc i)): nested call.
    for n in ["string/trim", "string/triml", "string/trimr"] {
        put(n, Sig::new(vec![str_ty], str_ty));
    }
    put(
        "string/replace",
        Sig::new(vec![str_ty, str_ty, str_ty], str_ty),
    );
    put("string/repeat", Sig::new(vec![str_ty, int], str_ty));
    for n in ["string/pad-left", "string/pad-right"] {
        put(n, Sig::new(vec![str_ty, int], str_ty));
    }
    put("string/char-at", Sig::new(vec![str_ty, int], str_ty));
    // String/list conversions: recursive helpers or `apply`.
    //   string->list        — (string/split s "").
    //   list->string        — (apply str cs).
    //   codepoints->string  — (apply str (map string/int->char cs)).
    // (string/->codepoints is a primitive now — its sig rides on the NativeFn.)
    put("string/->list", Sig::new(vec![str_ty], Ty::LIST));
    put("string/list->", Sig::new(vec![seq], str_ty));
    put("string/codepoints->", Sig::new(vec![seq], str_ty));
    // format: variadic with a required string template arg and a string result.
    put("string/format", Sig::with_rest(vec![str_ty], any, str_ty));
    // Search → int: all have branchy/recursive/optional-param bodies.
    //   index-of      — multi-clause cond over collection type; &optional from.
    //   index-where   — tail-recursive helper; 1-ary predicate.
    //   string/last-index-of — &optional before param; infer_sig bails.
    put("index-of", Sig::new(vec![any, any], int));
    // `seq/` since ADR-227 — keyed qualified for the same reason as `math/abs` above: a
    // bare key here would suppress the unbound lint on a name that no longer exists bare.
    put("seq/index-where", Sig::new(vec![cb1, seq], int));
    put("string/last-index-of", Sig::new(vec![str_ty, str_ty], int));
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
            let info = super::protocol::build_multi_info(heap, &[]);
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

    /// The same memo for **multi-arm** inference ([`infer_overload_of`]) — a separate
    /// table because the two answer different questions about the same name (one arm's
    /// signature vs every arm's), and a multi-arm closure has no single `Sig` to cache.
    static OVERLOAD_MEMO: RefCell<HashMap<Symbol, Option<Vec<Sig>>>> =
        RefCell::new(HashMap::new());
}

/// Reset the per-pass inference memo. `check_file` calls this at the start of each file so
/// one file's inferred signatures never leak into the next — and, in the long-lived LSP,
/// so an edit re-infers rather than serving a stale cached sig.
pub(super) fn clear_sig_memo() {
    SIG_MEMO.with(|m| m.borrow_mut().clear());
    OVERLOAD_MEMO.with(|m| m.borrow_mut().clear());
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
/// (defn g ((x) :when (string? x) (string/length x))
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
    let Value::Fn(cid) = super::deps::obs_global(heap, sym)? else {
        return None;
    };
    let closure = heap.closure(cid);
    if closure.arms.len() < 2 {
        return None;
    }
    let self_name = closure.name;
    let mut sigs = Vec::with_capacity(closure.arms.len());
    for arm in closure.arms.iter() {
        // A rest/optional arm's parameters don't map 1:1 to argument positions, so it
        // can't be checked positionally — and one unreadable arm makes the *set*
        // unusable (the check needs every arm to reject before it warns).
        if !arm.optionals.is_empty() || arm.rest.is_some() {
            return None;
        }
        let params: Vec<Symbol> = arm.params.clone();
        let param_tys = param_domains(heap, &arm.body, &params, &Ctx::default());
        let mut ctx = match self_name {
            Some(name) => Ctx::default().with_inferring_self(name),
            None => Ctx::default(),
        };
        for &p in &params {
            ctx = ctx.bind(p, Some(Ty::ANY));
        }
        let ret = arm
            .body
            .last()
            .and_then(|&tail| expr_ty(heap, tail, &ctx))
            .unwrap_or(Ty::ANY);
        sigs.push(Sig::new(param_tys, ret));
    }
    Some(sigs)
}

/// The same, read from a **form** rather than a loaded closure — the same-file
/// counterpart (`check_file` Pass 2.8), where the file being checked is not loaded.
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
/// to a single variadic `fn` over `match*` (ADR-226), so by the time the expanded
/// tree exists there are no clauses left to read — only a rest-list being
/// destructured. The un-expanded form still says exactly what each clause takes.
pub(super) fn infer_overload_from_clauses(
    heap: &Heap,
    clauses: &[Value],
    ctx: &Ctx,
) -> Option<Vec<Sig>> {
    let mut sigs = Vec::with_capacity(clauses.len());
    for &clause in clauses {
        let parts = super::walk::list_items(heap, clause)?;
        let plist = *parts.first()?;
        // Same restriction as the loaded path: a variadic/optional arm has no
        // positional signature, and one unreadable arm makes the set unusable.
        let raw = match plist {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => super::walk::list_items(heap, plist).unwrap_or_default(),
        };
        if raw.iter().any(|&it| {
            matches!(it, Value::Sym(s)
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST))
        }) {
            return None;
        }
        let params = super::walk::fn_params(heap, plist);
        // A `:when` guard on the clause head (ADR-226) — `(params :when guard body…)`.
        // The clause runs only for arguments its guard admits, so that is exactly the
        // clause's domain, intersected with whatever its body demands. This is where a
        // guarded multi-clause `defn` gets its argument types from, with no annotation:
        // each clause contributes its guard, and a call no clause admits raises
        // `match*`'s no-match at run time.
        let guarded = matches!(parts.get(1), Some(&Value::Keyword(k))
            if value::symbol_name_ref(k) == "when");
        let (guard, body): (Option<Value>, Vec<Value>) = if guarded {
            (parts.get(2).copied(), parts.get(3..)?.to_vec())
        } else {
            (None, parts[1..].to_vec())
        };
        if body.is_empty() {
            return None;
        }
        let mut param_tys = param_domains(heap, &body, &params, ctx);
        if let Some(g) = guard {
            let scope = DomainScope {
                shadowed: HashSet::new(),
                aliases: HashMap::new(),
            };
            let (then_slice, _) = guard_slices(heap, g, &params, &scope, ctx);
            param_tys = meet(param_tys, then_slice);
        }
        sigs.push(Sig::new(param_tys, Ty::ANY));
    }
    (sigs.len() >= 2).then_some(sigs)
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

    // Tier 1.5: each parameter's **domain** across the whole body — the union over
    // the body's possible executions, each branch credited only within the guard
    // that selects it (see [`param_domains`]). Generalises Tier 1 beyond a single
    // top-level call, and reaches the guarded uses the old unconditional-demand rule
    // had to ignore. Params nothing constrains stay `ANY`, recovering the return-only
    // behaviour exactly.
    let param_tys = param_domains(heap, &arm.body, &params, &Ctx::default());

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
    match expr_ty(heap, tail, &ctx) {
        Some(ret) => Some(Sig::new(param_tys, ret)),
        // The return couldn't be inferred (e.g. the body calls an ability op whose facts aren't
        // on this bare ctx). Still surface the parameter demands with an `ANY` return when a
        // param is actually constrained (ADR-190), so a *cross-file* caller's arguments are
        // checked — sound, since the demands under-constrain. A wholly-unconstrained param set
        // stays deferred (`None`), preserving the prior return-only behaviour exactly.
        None if param_tys.iter().any(|t| *t != Ty::ANY) => Some(Sig::new(param_tys, Ty::ANY)),
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

/// A single-arity file function's inferred **parameter domains**, read from its
/// `(fn (params) body…)` form (ADR-190) — the set of values each parameter can be called
/// with (see [`param_domains`]). `None` for a multi-arity / malformed / no-param fn (no
/// single parameter list to describe). **Sound for caller-flagging:** the domain
/// over-approximates (a superset of the true valid-argument type), so an argument disjoint
/// from it is disjoint from the truth too — it genuinely errors at runtime, never a false
/// positive. The companion of [`infer_return_from_form`] (which yields the return).
pub(super) fn infer_params_from_form(heap: &Heap, fn_form: Value, ctx: &Ctx) -> Option<Vec<Ty>> {
    let items = super::walk::list_items(heap, fn_form)?;
    if !matches!(items.first(), Some(&Value::Sym(s)) if super::walk::is_fn_head(s)) {
        return None;
    }
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        return None; // params vary per clause — no single demand to store
    }
    let plist = *items.get(1)?;
    // Skip a variadic (`&`) / `&optional` fn: its parameters don't map 1:1 to argument
    // positions (a rest binder collects the args into a *list*, not each arg), so a
    // per-position demand can't be soundly checked at a call site — it would flag a valid
    // `(vf 1 2 3)`. Mirrors `infer_sig`, which already yields a params-less sig for a complex
    // closure on the loaded path.
    let raw = match plist {
        Value::Vector(id) => heap.vector(id).to_vec(),
        _ => super::walk::list_items(heap, plist).unwrap_or_default(),
    };
    if raw.iter().any(|&it| {
        matches!(it, Value::Sym(s)
            if value::symbol_is(s, kw::AMP)
                || value::symbol_is(s, kw::AMP_OPTIONAL)
                || value::symbol_is(s, kw::AMP_REST))
    }) {
        return None;
    }
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
    Some(param_domains(heap, &body, &params, ctx))
}

// ---- parameter DOMAINS: the union over a body's possible executions ----

/// One parameter's slot in a domain vector, indexed like `params`.
type Domain = Vec<Ty>;

/// The scope facts the domain walk carries: which names an inner binder shadows,
/// and which `let`-binders **alias** a parameter. The alias table is load-bearing:
/// the `match` lowering binds the scrutinee to a fresh `m__28` and tests *that*, so
/// without it no clause guard would ever reach the parameter it came from.
#[derive(Clone)]
struct DomainScope {
    shadowed: HashSet<Symbol>,
    aliases: HashMap<Symbol, usize>,
}

/// Each parameter's **domain** (ADR-261): the set of values for which this body can run to
/// completion, over-approximated. The successor to the unconditional-demand rule,
/// and the piece that makes an inferred parameter type describe a function's actual
/// shape instead of only its straight-line uses.
///
/// The old rule credited only *unconditional* demands — a use inside any branch
/// constrained nothing, because crediting it outright would flag a caller whose
/// value never reaches that branch. That left the ordinary shape of Brood code
/// invisible:
///
/// ```lisp
/// (defn f (x) (if (string? x) (string/length x) (+ x 1)))
/// (f :kw)   ; neither branch admits a keyword — and nothing said so
/// ```
///
/// The fix is not to credit a guarded use unconditionally but to credit it *within
/// its guard*, and to union the alternatives:
///
/// > `D(if test then else)` = `D(test) ∩ ( (G ∩ D(then)) ∪ (¬G ∩ D(else)) )`
///
/// where `G` is the parameter slice the test proves ([`guards::guard_assertion`],
/// whose `ty` is a **necessary** condition for the test being true — exactly what
/// this needs) and `¬G` its complement, or `any` for a `then_only` guard that proves
/// nothing when false. With no guard on this parameter both slices are `any`, so the
/// rule degrades to `D(then) ∪ D(else)` — "whichever branch runs, one of them
/// demands this" — which is itself sound, and is what catches the example above.
///
/// **Soundness.** Every combinator widens or is exact: an unrecognised form is
/// `any`, sequenced forms intersect (both really do run), alternatives union, and
/// only a guard slice narrows — where the guard makes it a fact. So the result is a
/// *superset* of the true domain, and an argument disjoint from it is disjoint from
/// the truth: it genuinely errors. Never a false positive.
fn param_domains(heap: &Heap, body: &[Value], params: &[Symbol], ctx: &Ctx) -> Domain {
    let scope = DomainScope {
        shadowed: HashSet::new(),
        aliases: HashMap::new(),
    };
    // The body's forms are sequenced: all of them run.
    let mut acc = any_domain(params.len());
    for &form in body {
        acc = meet(acc, domain_of(heap, form, params, &scope, ctx));
    }
    // A parameter whose demands conflict to `never` could never be called
    // successfully at all. Rather than flag every caller, hand back `any` — the
    // same conservative retreat the unconditional rule made.
    for t in acc.iter_mut() {
        if t.is_never() {
            *t = Ty::ANY;
        }
    }
    acc
}

fn any_domain(n: usize) -> Domain {
    vec![Ty::ANY; n]
}

/// Both run — a value must satisfy each.
fn meet(a: Domain, b: Domain) -> Domain {
    a.into_iter().zip(b).map(|(x, y)| x.intersect(y)).collect()
}

/// One or the other runs — a value need only satisfy the one it reaches.
fn join(a: Domain, b: Domain) -> Domain {
    a.into_iter().zip(b).map(|(x, y)| x.union(y)).collect()
}

/// The domain of one form, per parameter.
///
/// Grows the stack in heap-backed segments as it recurses, like the rest of the
/// checker's walkers: a deeply-nested-but-legal body (the kernel's own deep-value
/// tests build them) must be *typed*, not blow the host's native stack.
fn domain_of(
    heap: &Heap,
    form: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        domain_of_inner(heap, form, params, scope, ctx)
    })
}

fn domain_of_inner(
    heap: &Heap,
    form: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    let Some(items) = list_items(heap, form) else {
        return any_domain(n); // an atom demands nothing on its own
    };
    let Some(&head) = items.first() else {
        return any_domain(n);
    };
    let Value::Sym(h) = head else {
        // A computed callee `((f) x …)` — the operands still all evaluate.
        return items[1..].iter().fold(any_domain(n), |acc, &arg| {
            meet(acc, domain_of(heap, arg, params, scope, ctx))
        });
    };
    // Nothing here runs against the parameters now: quoted data, a closure body
    // (deferred), a definer, or a `try` — whose failure is caught, so what its body
    // demands is not a requirement on the caller.
    if value::symbol_is(h, kw::QUOTE)
        || value::symbol_is(h, kw::QUASIQUOTE)
        || value::symbol_is(h, kw::FN)
        || value::symbol_is(h, kw::TRY)
        || value::symbol_is(h, kw::TRY_PRIM)
        || value::symbol_is(h, kw::DEF)
        || value::symbol_is(h, kw::DEFN)
        || value::symbol_is(h, kw::DEFMACRO)
        || value::symbol_is(h, kw::DEFDYN)
    {
        return any_domain(n);
    }
    // The `match` compiler's failure branch: `(throw [:match-error …])`. Reaching it
    // is not an execution any caller can want — every value that lands there raises.
    // So its domain is `never`, which is what makes a `match`'s clause patterns (and
    // a `defn` clause's `:when` guard, which lowers through the same engine) add up
    // to the function's domain instead of vanishing.
    if value::symbol_is(h, "throw") && items.len() == 2 && is_match_failure(heap, items[1]) {
        return vec![Ty::NEVER; n];
    }
    // `if` — the rule this whole function exists for.
    if value::symbol_is(h, kw::IF) {
        let Some(&test) = items.get(1) else {
            return any_domain(n);
        };
        let d_test = domain_of(heap, test, params, scope, ctx);
        let (then_slice, else_slice) = guard_slices(heap, test, params, scope, ctx);
        let d_then = items
            .get(2)
            .map(|&t| domain_of(heap, t, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
        let d_else = items
            .get(3)
            .map(|&e| domain_of(heap, e, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
        return meet(
            d_test,
            join(meet(then_slice, d_then), meet(else_slice, d_else)),
        );
    }
    // `cond` / `when` / `unless` — `if`s in disguise, and *not* always expanded away
    // before this runs: a prelude closure keeps its body as written, so the checker
    // reads `(cond …)` verbatim there. (Reading it as an ordinary call is what made
    // `type-matches?` demand a `seqable` first argument: every clause body, including
    // `(first t)`, looked unconditional.)
    if value::symbol_is(h, kw::COND) {
        return cond_domain(heap, &items[1..], params, scope, ctx);
    }
    if value::symbol_is(h, kw::WHEN) || value::symbol_is(h, kw::UNLESS) {
        let Some(&test) = items.get(1) else {
            return any_domain(n);
        };
        let d_test = domain_of(heap, test, params, scope, ctx);
        let (then_slice, else_slice) = guard_slices(heap, test, params, scope, ctx);
        let d_body = items[2..].iter().fold(any_domain(n), |acc, &f| {
            meet(acc, domain_of(heap, f, params, scope, ctx))
        });
        // `unless` runs its body on the *false* side; either way the other outcome
        // runs nothing and demands nothing.
        let (taken, skipped) = if value::symbol_is(h, kw::WHEN) {
            (then_slice, else_slice)
        } else {
            (else_slice, then_slice)
        };
        return meet(d_test, join(meet(taken, d_body), skipped));
    }
    // `case` / `match`: only the scrutinee is guaranteed to run. Their clause bodies
    // are guarded by patterns this walk doesn't read (the *expanded* form is where a
    // pattern becomes a guard chain it can read — and does).
    if value::symbol_is(h, kw::CASE) || value::symbol_is(h, kw::MATCH) {
        return items
            .get(1)
            .map(|&scrutinee| domain_of(heap, scrutinee, params, scope, ctx))
            .unwrap_or_else(|| any_domain(n));
    }
    // `and` / `or` are `if`s in disguise — `(and a b)` runs `b` only when `a` holds,
    // `(or a b)` only when it doesn't. Running them through the same rule keeps a
    // narrowing conjunct (`(and (int? x) (> x 0))`) working on the un-expanded path,
    // where they survive as themselves.
    if value::symbol_is(h, kw::AND) || value::symbol_is(h, kw::OR) {
        return short_circuit_domain(
            heap,
            &items[1..],
            value::symbol_is(h, kw::AND),
            params,
            scope,
            ctx,
        );
    }
    // `let` / `letrec`: every binding RHS runs, then the body. A binder shadows a
    // same-named parameter; a binder whose RHS *is* a parameter aliases it.
    if value::symbol_is(h, kw::LET) || value::symbol_is(h, kw::LETREC) {
        let Some(&binds_form) = items.get(1) else {
            return any_domain(n);
        };
        let Some(binds) = list_items(heap, binds_form) else {
            return any_domain(n);
        };
        if binds.len() % 2 != 0 {
            return any_domain(n);
        }
        let mut inner = scope.clone();
        let mut acc = any_domain(n);
        let mut i = 0;
        while i < binds.len() {
            // A destructuring binder hides names we can't enumerate — bail to `any`
            // rather than let a shadowed parameter leak a demand.
            let Value::Sym(name) = binds[i] else {
                return any_domain(n);
            };
            acc = meet(acc, domain_of(heap, binds[i + 1], params, &inner, ctx));
            match param_index(binds[i + 1], params, &inner) {
                Some(idx) => inner.aliases.insert(name, idx),
                None => inner.aliases.remove(&name),
            };
            inner.shadowed.insert(name);
            i += 2;
        }
        for &b in &items[2..] {
            acc = meet(acc, domain_of(heap, b, params, &inner, ctx));
        }
        return acc;
    }
    // `do`: every form runs.
    if value::symbol_is(h, kw::DO) {
        return items[1..].iter().fold(any_domain(n), |acc, &f| {
            meet(acc, domain_of(heap, f, params, scope, ctx))
        });
    }
    // An unexpanded **macro** call: its operands are syntax, not necessarily code —
    // a template may drop them, defer them, or bind them. Nothing here is known to
    // run, so nothing is demanded. (The generic rule below assumes every operand
    // evaluates, which is true of a call and false of a macro.)
    if super::walk::resolves_to_macro(heap, ctx, h) {
        return any_domain(n);
    }
    // An ordinary call: every argument evaluates, and a parameter passed *directly*
    // to a callee whose signature is known takes the type that position demands.
    // Table lookups only (primitive / curated / declared) — never `infer_sig`, so
    // this can't recurse into the inference that called it.
    // A **file-local** `(sig …)` first: the file is checked before it loads, so its
    // declarations are on `ctx` and NOT in the heap — `declared_heap_sig` cannot see
    // them. Without this a signature constrained its callers in every OTHER module and
    // not in its own, which is where most calls to it are. `std/bytes.blsp` declares
    // `(sig at (bytes int -> int))` and then calls `(bytes/at bs off)` three lines
    // later; `off` still inferred as the arithmetic domain rather than `int`.
    //
    // A lexical local shadows the global, so its type is not the declared one; and this
    // stays a table lookup like the three below, never `infer_sig`, so it cannot recurse
    // into the inference that called it.
    let callee_sig = (!scope.shadowed.contains(&h))
        .then(|| ctx.declared_sig(h))
        .flatten()
        .or_else(|| primitive_sig(heap, h))
        .or_else(|| curated_sig(h))
        .or_else(|| declared_heap_sig(heap, h));
    // Ability-op occurrence typing (ADR-190): a call to a *sealed* ability op demands
    // its first argument be a member of that ability.
    let op_domain = super::protocol::sealed_op_domain(h);
    let mut acc = any_domain(n);
    // **A parameter in call-HEAD position is callable.** `(g x)` only runs if `g` is,
    // so an accepted call proves it — the same argument every other demand rests on.
    // This is what types a *callback* parameter, which is what most higher-order
    // functions take and the position an argument-order slip reverses: without it
    // `(defn each (f xs) (f (first xs)))` types `f` as `any`, and passing the sequence
    // first is accepted in silence.
    //
    // Callable is `fn | native | keyword`, not just `fn`: a keyword is a function of a
    // map in Brood (`(:a {:a 1})` → 1), while maps, vectors and strings are not
    // callable (verified, not assumed — each raises).
    if let Some(pos) = param_index(head, params, scope) {
        let callable = Ty::of(Tag::Fn)
            .union(Ty::of(Tag::Native))
            .union(Ty::of(Tag::Keyword));
        acc[pos] = acc[pos].clone().intersect(callable);
    }
    for (i, &arg) in items[1..].iter().enumerate() {
        if let Some(pos) = param_index(arg, params, scope) {
            if let Some(expected) = callee_sig.as_ref().and_then(|s| s.param(i)) {
                acc[pos] = acc[pos].clone().intersect(expected);
            }
            if i == 0 {
                if let Some(dom) = op_domain.clone() {
                    acc[pos] = acc[pos].clone().intersect(dom);
                }
            }
        }
        acc = meet(acc, domain_of(heap, arg, params, scope, ctx));
    }
    acc
}

/// `(and a b c…)` / `(or a b c…)` — each operand after the first runs only when the
/// ones before it went the right way. Recurses over the tail rather than rebuilding
/// a form, so `c`'s demand is guarded by `b`'s outcome as well as `a`'s (folding the
/// whole tail together instead would *narrow* the domain — the unsound direction).
fn short_circuit_domain(
    heap: &Heap,
    operands: &[Value],
    is_and: bool,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    match operands {
        [] => any_domain(n),
        [only] => domain_of(heap, *only, params, scope, ctx),
        [first, rest @ ..] => {
            let d_first = domain_of(heap, *first, params, scope, ctx);
            let d_rest = short_circuit_domain(heap, rest, is_and, params, scope, ctx);
            let (then_slice, else_slice) = guard_slices(heap, *first, params, scope, ctx);
            // `and` continues when the first operand holds, `or` when it doesn't; the
            // other way, the tail never runs and demands nothing (its slice alone).
            let (taken, skipped) = if is_and {
                (then_slice, else_slice)
            } else {
                (else_slice, then_slice)
            };
            meet(d_first, join(meet(taken, d_rest), skipped))
        }
    }
}

/// `(cond t1 b1 t2 b2 … else bn)` — the `if` rule, applied down the clause list.
/// A clause's body is credited only within the slice its own test proves, and the
/// tail (no clause matched, so nothing ran) demands nothing.
fn cond_domain(
    heap: &Heap,
    clauses: &[Value],
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> Domain {
    let n = params.len();
    match clauses {
        [] | [_] => any_domain(n), // exhausted, or a malformed dangling test
        [test, body, rest @ ..] => {
            let d_test = domain_of(heap, *test, params, scope, ctx);
            let (then_slice, else_slice) = guard_slices(heap, *test, params, scope, ctx);
            let d_body = domain_of(heap, *body, params, scope, ctx);
            let d_rest = cond_domain(heap, rest, params, scope, ctx);
            meet(
                d_test,
                join(meet(then_slice, d_body), meet(else_slice, d_rest)),
            )
        }
    }
}

/// The parameter a form refers to — directly, or through a `let` alias — unless an
/// inner binder shadows it.
fn param_index(form: Value, params: &[Symbol], scope: &DomainScope) -> Option<usize> {
    let Value::Sym(s) = form else { return None };
    if scope.shadowed.contains(&s) {
        return scope.aliases.get(&s).copied();
    }
    params.iter().position(|&p| p == s)
}

/// The two parameter slices a test splits its branches by: what a parameter must be
/// for the test to be true, and what it must be for the test to be false. `any` in
/// both slots when the test proves nothing about that parameter — which degrades the
/// `if` rule to a plain union of the branches, the sound default.
///
/// A whole-test guard gives both slices. A *conjunction* gives only then-slices:
/// every conjunct must hold for the test to be true, but its being false says only
/// that *some* conjunct failed — never that a particular one did.
fn guard_slices(
    heap: &Heap,
    test: Value,
    params: &[Symbol],
    scope: &DomainScope,
    ctx: &Ctx,
) -> (Domain, Domain) {
    let n = params.len();
    let mut then_slice = any_domain(n);
    let mut else_slice = any_domain(n);
    if let Some(guard) = super::guards::guard_assertion(heap, test, ctx) {
        if let Some(idx) = param_index(Value::Sym(guard.sym), params, scope) {
            then_slice[idx] = guard.ty.clone();
            if !guard.then_only {
                else_slice[idx] = guard.ty.negate();
            }
        }
        return (then_slice, else_slice);
    }
    for guard in super::guards::and_conjunct_guards(heap, test, ctx) {
        if let Some(idx) = param_index(Value::Sym(guard.sym), params, scope) {
            then_slice[idx] = then_slice[idx].clone().intersect(guard.ty);
        }
    }
    (then_slice, else_slice)
}

/// Is this the vector the `match` compiler throws when no clause matched —
/// `[:match-error 'context target 'patterns]`?
fn is_match_failure(heap: &Heap, arg: Value) -> bool {
    let Value::Vector(vid) = arg else {
        return false;
    };
    let elems = heap.vector(vid).to_vec();
    matches!(elems.first(), Some(&Value::Keyword(tag))
        if value::symbol_name_ref(tag) == "match-error")
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
/// The [`SigWithVars`] a loaded module declared for `sym`, when its declaration carries
/// type variables — the live-image counterpart of `Ctx::declared_sig_with_vars`.
pub(super) fn declared_heap_sig_with_vars(
    heap: &Heap,
    sym: Symbol,
) -> Option<super::ctx::SigWithVars> {
    let type_value = super::deps::obs_declared_sig_value(heap, sym)?;
    annot::parse_arrow_type_with_vars(heap, type_value)
}

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
    // The operator sugar's registry-derived domain (ADR-299) wins over the widest reading
    // the native declares, whichever registry that reading arrives through.
    operator_sig(heap, sym)
        .or_else(|| declared_heap_sig(heap, sym))
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
