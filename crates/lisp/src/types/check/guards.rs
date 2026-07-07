//! Predicates over forms that the walker dispatches on:
//!
//! - [`is_syntactic_keyword`] — which heads are *not* callables, so an
//!   "unbound symbol" warning doesn't fire on them. (The "don't descend
//!   into this body" predicate that used to live here is now folded into
//!   `walk::SPECIAL_HEAD` so the dispatch is one `SymbolMap` probe.)
//! - [`guard_assertion`] / [`literal_eq_guard`] — pull a `(sym, type)` pair
//!   out of an `if`-test when it's a recognised guard, so the walk can
//!   narrow the variable in each branch.
//! - [`expr_ty`] — the static type of a form `in ctx`, the single
//!   "do I know what this expression returns?" probe the misuse-check
//!   reads off.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::Ty;

use super::ctx::{resolve_overload_ret, Ctx, PathKey};
use super::sigs::{declared_heap_overload, sig_of};
use super::walk::{is_fn_head, list_items};

/// Names that have *syntactic* meaning but aren't bound values — never flag
/// these as unbound. Mirrors `eval::SPECIAL_NAMES` plus the macros that the
/// reader / un-expanded forms may carry (the CLI's `--check` doesn't
/// macroexpand). `catch` is the carrier-form for `try`'s catcher, not a
/// callable; `&` / `&optional` are parameter-list markers.
pub(super) fn is_syntactic_keyword(name: &str) -> bool {
    matches!(
        name,
        kw::QUOTE
            | kw::QUASIQUOTE
            | kw::UNQUOTE
            | kw::UNQUOTE_SPLICING
            | kw::IF
            | kw::DO
            | kw::DEF
            | kw::FN
            | kw::LAMBDA
            | kw::LET
            | kw::LETREC
            | kw::DEFMACRO
            | kw::DEFN
            | kw::DEFDYN
            | kw::DEFMODULE
            | kw::MODULE_DOC
            | kw::WHEN
            | kw::UNLESS
            | kw::COND
            | kw::AND
            | kw::OR
            | kw::THREAD_FIRST
            | kw::THREAD_LAST
            | kw::MATCH
            | kw::CASE
            | kw::TRY
            | kw::CATCH
            | kw::THROW
            | kw::BINDING
            | kw::FOR
            | kw::SPAWN
            | kw::AMP
            | kw::AMP_OPTIONAL
            | kw::AMP_REST
    )
}

// `skips_body` used to live here; it's now folded into the
// `SpecialHead::SkipBody` arm of `walk::SPECIAL_HEAD` (one `SymbolMap` probe
// shared with the special-form dispatch, no per-call string allocation).
// Names that route through that arm: `quote`, `quasiquote`, `try`,
// `error-of`, `assert-error`, `%try`. `%try` matters post-expansion: the
// macroexpand pass rewrites `(try …)` to `(%try (fn () body) (fn (e) handler))`
// before `check_file` walks the tree, and without `%try` in that arm the walk
// would descend into the "I expect this to fail" body and flag every
// `(error-of (cons 1))` in the test suite.

/// A recognised type guard over a single variable: when `test` is truthy, `sym`
/// provably has type `ty`. `then_only` marks a guard whose *negation is unsound*
/// — a falsy `test` does **not** establish `¬ty`, so the else-branch must not be
/// narrowed (the `and` short-circuit is the case: a falsy `and` may have failed
/// on a *later* conjunct, so the first conjunct can still hold). An ordinary type
/// predicate is biconditional (`then_only = false`): the else-branch narrows to
/// `¬ty` soundly.
pub(super) struct Guard {
    pub(super) sym: Symbol,
    pub(super) ty: Ty,
    pub(super) then_only: bool,
}

/// A type guard over a **compound access path** — a keyword-`get` and/or fixed
/// integer-index chain (`(get r :age)`, `(nth t 0)`, `(first (get r :xs))`) —
/// rather than a bare variable. `(if (int? (get r :age)) …)` yields
/// `PathGuard { base: r, keys: [Field :age], ty: int }`. `then_only` carries the
/// same meaning as [`Guard`]'s (an ordinary type predicate is biconditional, so
/// the else-branch narrows to `¬ty`).
pub(super) struct PathGuard {
    pub(super) base: Symbol,
    pub(super) keys: Vec<PathKey>,
    pub(super) ty: Ty,
    pub(super) then_only: bool,
}

/// Peel a (possibly nested) access chain down to its base symbol and the ordered
/// [`PathKey`]s, base-outward: `(get r :age)` → `(r, [Field :age])`,
/// `(nth (get cfg :items) 0)` → `(cfg, [Field :items, Index 0])`. Recognises
/// `get` with a keyword key and the fixed-index accessors `nth` (literal
/// non-negative index), `first`/`second`/`third` (0/1/2). A bare symbol yields
/// `(s, [])` (the recursion base — an empty "path" is just the variable, so
/// callers that require a real path check for that). `None` for anything else —
/// a computed (non-literal) key/index, `last` (arity-dependent), or a non-access
/// form — none of which is a statically pinnable path.
pub(super) fn path_of(heap: &Heap, expr: Value) -> Option<(Symbol, Vec<PathKey>)> {
    if let Value::Sym(s) = expr {
        return Some((s, Vec::new()));
    }
    let items = list_items(heap, expr)?;
    let Some(&Value::Sym(head)) = items.first() else {
        return None;
    };
    let (inner, key) = if value::symbol_is(head, "get") && items.len() == 3 {
        let Value::Keyword(k) = items[2] else {
            return None;
        };
        (items[1], PathKey::Field(k))
    } else if value::symbol_is(head, "nth") && items.len() == 3 {
        let Value::Int(i) = items[2] else {
            return None;
        };
        (items[1], PathKey::Index(usize::try_from(i).ok()?))
    } else if value::symbol_is(head, "first") && items.len() == 2 {
        (items[1], PathKey::Index(0))
    } else if value::symbol_is(head, "second") && items.len() == 2 {
        (items[1], PathKey::Index(1))
    } else if value::symbol_is(head, "third") && items.len() == 2 {
        (items[1], PathKey::Index(2))
    } else {
        return None;
    };
    let (base, mut keys) = path_of(heap, inner)?;
    keys.push(key);
    Some((base, keys))
}

/// If `test` is a type predicate applied to an access path — or its `(not …)` —
/// return the [`PathGuard`] it asserts. Handles arbitrary nesting of field/index
/// steps via [`path_of`]; a computed key/index or a non-path form is left alone
/// (no narrowing, no false positive), and a bare variable (empty path) is
/// deferred to the plain [`guard_assertion`]. Mirrors that function's structure.
pub(super) fn path_guard_assertion(heap: &Heap, test: Value) -> Option<PathGuard> {
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // `(not <inner>)` — invert a biconditional inner path guard.
    if items.len() == 2 && head_name == kw::NOT {
        let inner = path_guard_assertion(heap, items[1])?;
        if inner.then_only {
            return None;
        }
        return Some(PathGuard {
            ty: inner.ty.negate(),
            ..inner
        });
    }
    // `(pred? <get-path>)` — a type predicate over a (possibly nested) field path.
    if items.len() != 2 {
        return None;
    }
    let ty = Ty::tested_by(&head_name)?;
    let (base, keys) = path_of(heap, items[1])?;
    if keys.is_empty() {
        return None; // a bare variable — `guard_assertion` handles that
    }
    Some(PathGuard {
        base,
        keys,
        ty,
        then_only: false,
    })
}

/// If `test` is a recognisable type guard over a single variable, return the
/// [`Guard`] it implies. A leading `(not …)` flips the assertion via
/// [`Ty::negate`]. A bare `Sym` is looked up in `ctx`'s guard-alias table (a
/// `let`-stored guard result — `(let (cond (int? x)) (if cond …))`). `None` for
/// any test that isn't a pure single-variable guard.
pub(super) fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<Guard> {
    if let Value::Sym(s) = test {
        // A let-stored guard alias — recorded only for biconditional guards
        // (see `check_let`), so it narrows the else-branch too.
        let (sym, ty) = ctx.guard(s)?;
        return Some(Guard {
            sym,
            ty,
            then_only: false,
        });
    }
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        return None;
    };
    let head_name = value::symbol_name(head);
    // (not <inner>) — invert the inner assertion. Only invertible when `inner`
    // is itself biconditional; a `then_only` inner can't be soundly negated
    // (we'd be reasoning from `inner` being false), so we decline.
    if items.len() == 2 && head_name == kw::NOT {
        let inner = guard_assertion(heap, items[1], ctx)?;
        if inner.then_only {
            return None;
        }
        return Some(Guard {
            sym: inner.sym,
            ty: inner.ty.negate(),
            then_only: false,
        });
    }
    // `(%eq sym literal)` / `(%eq literal sym)` — equality against a literal
    // asserts the variable has the literal's runtime tag. The `match` pattern
    // compiler emits this for literal patterns (e.g. `(match x (5 …))`
    // lowers through `(let (m x) (if (%eq m 5) …))` — and the let-alias
    // machinery threads the narrowing back to `x`). Variadic `=` reaches us
    // pre-expanded as `%eq` calls when arities are 2, so we only need to
    // recognise the primitive shape.
    if items.len() == 3 && head_name == kw::EQ_PRIM {
        if let Some((sym, ty)) =
            literal_eq_guard(items[1], items[2]).or_else(|| literal_eq_guard(items[2], items[1]))
        {
            // **`then_only`:** `(%eq m lit)` being true proves `m` has `lit`'s
            // tag, but being *false* proves nothing about the tag — `m ≠ "x"`
            // can still be another string. So the else-branch must NOT narrow to
            // `¬ty` (that flagged a valid `(string-length m)` after `(= m "x")`).
            // (`nil` is the one tag where `≠ nil` *would* imply `¬nil`, but we
            // don't special-case it — dropping that narrowing only loses
            // precision, never soundness.)
            return Some(Guard {
                sym,
                ty,
                then_only: true,
            });
        }
        return None;
    }
    // The `and` short-circuit expansion `(let (g E) (if g _ g))` — a truthy
    // `and` implies its first conjunct `E` holds, so an `(if (and (pred? x) …) …)`
    // narrows `x` in the *then* branch. Matched post-`macroexpand_all` (when the
    // `(and …)` surface is already this shape); the `or` expansion
    // `(if g g _)` is deliberately *not* matched (a truthy `or` implies nothing
    // about its first operand). This is what lets the `match` compiler's
    // `(if (and (vector? m) (= (vector-length m) 2)) …)` narrow `m` to a vector,
    // so the guarded `vector-ref m i` isn't flagged against a list/other scrutinee.
    // **`then_only`:** a falsy `and` may have failed on a later conjunct, so the
    // else-branch must NOT be narrowed to `¬E` (that was a real false positive —
    // an else-branch `(vector-ref m i)` on a value that *is* a longer vector).
    if head_name == kw::LET && items.len() == 3 {
        if let Some(g) = and_first_conjunct_guard(heap, items[1], items[2], ctx) {
            return Some(g);
        }
    }
    if items.len() != 2 {
        return None;
    }
    let ty = Ty::tested_by(&head_name)?;
    match items[1] {
        Value::Sym(s) => Some(Guard {
            sym: s,
            ty,
            then_only: false,
        }),
        _ => None,
    }
}

/// Recognise the `and`-expansion `(let (g E) (if g _ g))` and return the guard
/// its first conjunct `E` asserts, marked `then_only` (the negation is unsound —
/// see [`Guard`]). The binding must be exactly one name `g`, and the body must be
/// `(if g <then> g)` — test and *else* both `g` (the `and` shape; `or` is
/// `(if g g <else>)` and must not match).
fn and_first_conjunct_guard(heap: &Heap, binding: Value, body: Value, ctx: &Ctx) -> Option<Guard> {
    let bs = list_items(heap, binding)?;
    if bs.len() != 2 {
        return None; // a multi-binding `let` isn't the `and` shape
    }
    let Value::Sym(g) = bs[0] else { return None };
    let cond = bs[1];
    let body_items = list_items(heap, body)?;
    // `(if test then else)` — 4 items; test == g and else == g.
    let is_if = matches!(body_items.first(), Some(&Value::Sym(s)) if value::symbol_is(s, kw::IF));
    if body_items.len() != 4 || !is_if {
        return None;
    }
    let is_g = |v: Value| matches!(v, Value::Sym(s) if s == g);
    if !is_g(body_items[1]) || !is_g(body_items[3]) {
        return None;
    }
    let inner = guard_assertion(heap, cond, ctx)?;
    Some(Guard {
        then_only: true, // a falsy `and` doesn't establish `¬E`
        ..inner
    })
}

/// If `a` is a symbol and `b` is a self-evaluating literal, return the guard
/// `(a, type-of(b))`. Used by `guard_assertion`'s `%eq` arm to recognise both
/// `(%eq sym lit)` and `(%eq lit sym)`. Returns `None` when `b` is itself a
/// variable — equality between two unknowns asserts nothing.
fn literal_eq_guard(a: Value, b: Value) -> Option<(Symbol, Ty)> {
    let Value::Sym(s) = a else { return None };
    // A literal is anything that's not a symbol / pair / vector / map.
    // Strings, ints, floats, keywords, booleans, nil all self-evaluate and
    // have a definite tag; pairs/vectors/maps are constructions whose pieces
    // could be unknown.
    match b {
        Value::Sym(_) | Value::Pair(_) | Value::Vector(_) | Value::Map(_) => None,
        other => Some((s, Ty::of_value(other))),
    }
}

/// Match-exhaustiveness check over literal-enum scrutinees (ADR-118).
///
/// `match` compiles `(match expr clause…)` to a `let`+`if`+`%eq` chain whose
/// innermost failure is `(throw [:match-error 'context target 'patterns])`
/// (`match-no-match`, `std/prelude.blsp`) — and that exact shape is only
/// present in the compiled tree when the match has **no catch-all clause**
/// (an irrefutable wildcard/bind clause compiles to its body directly, no
/// further `if`, so the throw never gets generated). So finding this shape
/// at all already means "this match isn't covered by a catch-all"; the
/// `patterns` slot is the full list of every clause's raw pattern, quoted
/// literal data sitting right there — no clause-boundary reconstruction
/// needed.
///
/// `target`'s ctx type here is its *original* declared type, unnarrowed: the
/// else-branch of a `(%eq target lit)` test is `then_only` (`guard_assertion`),
/// so `check_if` never narrows it going down the chain. If that type is a
/// **pure** literal-enum (every member is a keyword-literal or an
/// int-literal, nothing else mixed in), and the tried patterns don't cover
/// every member, this returns a message naming what's missing.
///
/// Conservative by construction: a non-literal pattern among those tried
/// (a destructuring pattern, a guarded bind) bails to `None` rather than
/// half-reasoning about coverage; a scrutinee whose type isn't a pure
/// literal-enum bails too. Never a false positive, may miss a real gap.
pub(super) fn match_exhaustiveness_gap(heap: &Heap, throw_arg: Value, ctx: &Ctx) -> Option<String> {
    let Value::Vector(vid) = throw_arg else {
        return None;
    };
    let elems = heap.vector(vid).to_vec();
    if elems.len() != 4 {
        return None;
    }
    let Value::Keyword(tag) = elems[0] else {
        return None;
    };
    if value::symbol_name_ref(tag) != "match-error" {
        return None;
    }
    let Value::Sym(target) = elems[2] else {
        return None;
    };
    let target_ty = expr_ty(heap, Value::Sym(target), ctx)?;

    // Unwrap `(quote patterns-list)` to the raw pattern list.
    let quote_items = list_items(heap, elems[3])?;
    if quote_items.len() != 2 {
        return None;
    }
    let Value::Sym(q) = quote_items[0] else {
        return None;
    };
    if !value::symbol_is(q, "quote") {
        return None;
    }
    let patterns = list_items(heap, quote_items[1])?;

    // **Purity check, generalized (ADR-121):** every tag `target_ty` admits
    // must be one of the five enumerable kinds — `coverable` carries no
    // refinements itself, so `is_subtype`'s per-bit refinement checks never
    // fire; this reduces to a plain tag-subset check ("is every tag in
    // `target_ty` one of these five"). Any other tag (a vector, a map, an
    // unrefined open tag among these five, …) bails — can't enumerate an
    // open set.
    let coverable = Ty::of(Tag::Keyword)
        .union(Ty::of(Tag::Int))
        .union(Ty::of(Tag::Bool))
        .union(Ty::of(Tag::Str))
        .union(Ty::of(Tag::Nil));
    if !target_ty.is_subtype(&coverable) {
        return None;
    }

    // Render every declared member to a canonical label, one tag at a time.
    // An unrefined occurrence of any of these tags (the literal-set accessor
    // is `None` while the tag is still present) bails the whole check — an
    // open int/keyword/bool/string mixed into an otherwise-enumerable type
    // isn't itself enumerable.
    let mut declared: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if target_ty.contains_tag(Tag::Nil) {
        declared.insert("nil".to_string());
    }
    if target_ty.contains_tag(Tag::Keyword) {
        for &s in target_ty.as_lit()? {
            declared.insert(format!(":{}", value::symbol_name_ref(s)));
        }
    }
    if target_ty.contains_tag(Tag::Int) {
        for &n in target_ty.as_lit_int()? {
            declared.insert(n.to_string());
        }
    }
    if target_ty.contains_tag(Tag::Bool) {
        for &b in target_ty.as_lit_bool()? {
            declared.insert(b.to_string());
        }
    }
    if target_ty.contains_tag(Tag::Str) {
        for s in target_ty.as_lit_str()? {
            declared.insert(format!("{s:?}"));
        }
    }

    // Render every tried pattern the same way; any non-literal pattern
    // (destructuring, a guarded bind, a pin) bails — no coverage reasoning
    // attempted for those (sound: misses a real gap rather than guessing).
    let mut tested: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for &p in &patterns {
        tested.insert(render_literal_pattern(heap, p)?);
    }

    let mut missing: Vec<&String> = declared.difference(&tested).collect();
    if missing.is_empty() {
        return None;
    }
    missing.sort();
    let joined = missing
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("match: not exhaustive — missing {joined}"))
}

/// Render a raw literal pattern `Value` to the same canonical label
/// [`match_exhaustiveness_gap`] uses for a declared type's enumerated
/// members — `:name` / bare digits / `true`/`false` / a quoted string /
/// `nil`. `None` for anything else (a destructuring pattern, a guarded bind,
/// a pin) — the caller declines to reason about coverage in that case.
pub(super) fn render_literal_pattern(heap: &Heap, v: Value) -> Option<String> {
    match v {
        Value::Keyword(s) => Some(format!(":{}", value::symbol_name_ref(s))),
        Value::Int(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Str(id) => Some(format!("{:?}", heap.string(id))),
        Value::Nil => Some("nil".to_string()),
        // Not one of `Ty`'s enumerable literal kinds (no `lit_float`), but a
        // legitimate literal pattern nonetheless — only [`find_redundant_clause`]
        // (ADR-122, no `Ty` involved) ever needs to render one.
        Value::Float(f) => Some(f.to_string()),
        _ => None,
    }
}

/// Match-redundancy detection (ADR-122) — a different, independent problem
/// from exhaustiveness: purely structural on the compiled `if`/`%eq` chain,
/// no scrutinee `Ty` involved at all. Fires on *any* same-symbol `%eq`-literal
/// `if`-chain, whether it came from `match`/`cond` or was hand-written.
///
/// If `test` is itself `(%eq sym lit)`, return `(sym, lit)` — the raw literal
/// `Value`, not a `Ty` (redundancy needs exact value equality, not a tag).
/// Mirrors [`literal_eq_guard`]'s recognition of `(%eq sym lit)` /
/// `(%eq lit sym)`, independently (that function only returns the guard's
/// `Ty`, having already discarded the concrete value).
pub(super) fn literal_eq_test_raw(heap: &Heap, test: Value) -> Option<(Symbol, Value)> {
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        return None;
    };
    if items.len() != 3 || !value::symbol_is(head, kw::EQ_PRIM) {
        return None;
    }
    literal_eq_raw(items[1], items[2]).or_else(|| literal_eq_raw(items[2], items[1]))
}

/// Like [`literal_eq_guard`], but returns the raw literal `Value` instead of
/// converting it to a `Ty` — `literal_eq_test_raw`'s single-pair-order half.
fn literal_eq_raw(a: Value, b: Value) -> Option<(Symbol, Value)> {
    let Value::Sym(s) = a else { return None };
    match b {
        Value::Sym(_) | Value::Pair(_) | Value::Vector(_) | Value::Map(_) => None,
        other => Some((s, other)),
    }
}

/// Exact syntactic equality between two literal patterns — used to detect a
/// duplicate clause, not to build a type's value set (unlike the `BTreeSet`-
/// based literal types, `Value::Float` is included here: comparing two
/// literal tokens for "did the source write the same thing twice" has none of
/// `Ord`/`Hash`'s NaN trouble).
fn literal_values_equal(heap: &Heap, a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => heap.string(x) == heap.string(y),
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Nil, Value::Nil) => true,
        _ => false,
    }
}

/// Scan forward through a same-symbol `%eq`-literal `if`-chain starting at
/// `form` (an `else`-branch continuation), looking for another test of `sym`
/// against `lit` — which would make that clause unreachable (an earlier
/// occurrence in the chain always wins). Returns the duplicate `if` form, if
/// found. Stops silently as soon as `form` isn't itself another same-symbol
/// `%eq`-guarded `if` (a catch-all body, a `match-no-match` throw, or a
/// divergent hand-written `if`) — nothing more to reason about.
pub(super) fn find_redundant_clause(
    heap: &Heap,
    form: Value,
    sym: Symbol,
    lit: Value,
) -> Option<Value> {
    let items = list_items(heap, form)?;
    if items.len() != 4 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, kw::IF) {
        return None;
    }
    let (test_sym, test_lit) = literal_eq_test_raw(heap, items[1])?;
    if test_sym != sym {
        return None;
    }
    if literal_values_equal(heap, test_lit, lit) {
        return Some(form);
    }
    find_redundant_clause(heap, items[3], sym, lit)
}

/// The static type of an expression form *in `ctx`*, or `None` when it can't
/// be pinned. `None` is "unknown" and is never flagged. Self-evaluating
/// literals get their exact tag; a `quote`d datum gets the datum's tag; a call
/// with a known signature gets its result type; a variable returns whatever
/// `ctx` knows about it (typically `None` for a free / global reference).
pub(super) fn expr_ty(heap: &Heap, form: Value, ctx: &Ctx) -> Option<Ty> {
    match form {
        // A bare symbol is a variable reference — looked up in the local ctx
        // (let-bound RHS / if-guard narrowing). A miss falls back to a `(sig x T)`
        // value-type declaration on a *global*: a redefinable global with a declared
        // type contributes `T` to the disjointness check, so `(string-length g)` for
        // `(sig g int)` is caught. Sound — the disjointness check only warns on a
        // provable mismatch with the declared (contract) type and defers on overlap,
        // exactly `dynamic(T)`'s behaviour (contract #4). A lexical local shadows it.
        Value::Sym(s) => ctx.get(s).or_else(|| {
            if ctx.is_lexical_local(s) {
                None
            } else {
                // Declared value type first (authoritative), then the Gap A
                // inferred current-image type for an undeclared global. Both feed
                // the `∩`-only `is_disjoint` arg check, so this is reload-safe.
                ctx.declared_value_ty(s)
                    .or_else(|| ctx.inferred_value_ty(s))
            }
        }),
        // A vector literal `[a b c]` — its elements are evaluated in place, so
        // (ADR-128) its exact per-position types are known, not just their
        // union: infer a tuple shape rather than widening to a uniform
        // `vector<E>`. Sound and strictly more precise (a `tuple` is already a
        // subtype of the corresponding uniform `vector<E>` — `Ty::is_subtype`
        // derives that fallback — so every check that passed under the old
        // widened inference still passes). Any unknown element → the whole
        // literal falls back to unrefined `vector` (same all-or-nothing
        // strictness `element_union` already had).
        Value::Vector(id) => {
            let items = heap.vector(id).to_vec();
            let elems: Option<Vec<Ty>> = items.iter().map(|&it| expr_ty(heap, it, ctx)).collect();
            Some(match elems {
                Some(e) => Ty::tuple_of(e),
                None => Ty::of(Tag::Vector),
            })
        }
        // A map literal `{:k v …}` — every keyword-literal key is definitely
        // present (it's data, evaluated once), so infer a record shape: each
        // resolvable `:key value` pair becomes a *required* field (Step 5+,
        // ADR-115). A non-keyword key, or a value whose type is unknown, is
        // simply omitted from the shape — records are open, so silently
        // under-declaring a field is sound (widens what the type claims),
        // never a false positive. `{}` / an all-unknown map infers the empty
        // record (equivalent to flat `map` for every check that matters here).
        Value::Map(id) => {
            let mut fields = std::collections::BTreeMap::new();
            for (k, v) in heap.map_entries(id) {
                if let Value::Keyword(name) = k {
                    if let Some(vty) = expr_ty(heap, v, ctx) {
                        fields.insert(name, (vty, true));
                    }
                }
            }
            Some(Ty::record_of(fields))
        }
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // A guard-narrowed path wins over any structural result type: if this
            // whole form is a recognised access path (`(get …)`/`(nth …)`/
            // `(first …)`…) that an enclosing `(if (pred? <path>) …)` narrowed,
            // that's the most specific type for the branch (occurrence typing,
            // sound under immutability). Subsumes the per-accessor rules below.
            if let Some((base, keys)) = path_of(heap, form) {
                if !keys.is_empty() {
                    if let Some(t) = ctx.path_ty(base, &keys) {
                        return Some(t);
                    }
                }
            }
            match items.first().copied() {
                Some(Value::Sym(s)) => {
                    if value::symbol_is(s, kw::QUOTE) {
                        return items.get(1).map(|&d| Ty::of_value(d));
                    }
                    // Control-flow forms whose value is one of their sub-forms —
                    // typed by unioning the possible result positions, but *only*
                    // when the head isn't a lexical local shadowing the special form.
                    // CRITICAL for false-positive-freedom: if any contributing
                    // sub-form types to `None` (unknown), `control_flow_ty` returns
                    // `None`, so the whole form defers. A special-form head can't be
                    // a callable global, so we never confuse this with a call.
                    if !ctx.is_lexical_local(s) {
                        if let Some(t) = control_flow_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                    }
                    // A user `(sig …)` declaration is authoritative for the
                    // result type — consult it unless a *lexical* local (fn/let)
                    // shadows the name. (A file-global with a declared sig is the
                    // target, so guard on `is_lexical_local`, not `is_local`.)
                    if !ctx.is_lexical_local(s) {
                        // Type-variable sig: resolve the return type from arg types
                        // (e.g. `(sig identity (?A -> ?A))` → result = arg type).
                        if let Some(sv) = ctx.declared_sig_with_vars(s) {
                            let arg_tys: Vec<Option<Ty>> =
                                items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                            return Some(sv.resolve_ret(&arg_tys));
                        }
                        // An overloaded sig (ADR-116) — `(and (int -> int)
                        // (bool -> bool))` — resolves per matching arm instead
                        // of a single flat `ret`.
                        if let Some(sigs) = ctx.declared_overload(s) {
                            let arg_tys: Vec<Option<Ty>> =
                                items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                            return Some(resolve_overload_ret(sigs, &arg_tys));
                        }
                        if let Some(sg) = ctx.declared_sig(s) {
                            return Some(sg.ret);
                        }
                    }
                    // Sequence-aware refinements (`list`/`vector` constructors,
                    // `first`/`last`/`nth` extractors) and the integer-closed
                    // numeric rule — both when the head isn't a local shadow; else
                    // the callee's flat result type.
                    if !ctx.is_local(s) {
                        if let Some(t) = numeric_call_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                        if let Some(t) = seq_aware_call_ty(heap, s, &items, ctx) {
                            return Some(t);
                        }
                    }
                    // The heap-recorded counterpart of the `ctx.declared_overload`
                    // check above (ADR-116) — makes an overload declared in
                    // *another* module visible here too, the same way
                    // `sig_of`/`declared_heap_sig` already does for a plain
                    // single-arrow sig.
                    if let Some(sigs) = declared_heap_overload(heap, s) {
                        let arg_tys: Vec<Option<Ty>> =
                            items[1..].iter().map(|&a| expr_ty(heap, a, ctx)).collect();
                        return Some(resolve_overload_ret(&sigs, &arg_tys));
                    }
                    sig_of(heap, s).map(|sig| sig.ret)
                }
                _ => None,
            }
        }
        // Int / Float / Str / Keyword / Bool / Nil: self-evaluating.
        other => Some(Ty::of_value(other)),
    }
}

/// The union of the element forms' types, or `None` if empty or any element is
/// unknown (so the element type can't be pinned — stay unrefined, never wrong).
fn element_union(heap: &Heap, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for &it in items {
        let t = expr_ty(heap, it, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    acc
}

/// The static type of a **control-flow form** — one whose runtime value is one of
/// its sub-forms (`if`/`when`/`do`/`let`/`cond`/`case`/`match`/`and`/`or`). The
/// result is the union of the types at every position the form can yield from; the
/// `let` family threads each binding's RHS type into the scope used to type the body
/// (so `(let (x 5) (+ x 1))` sees `x : int`).
///
/// **False-positive discipline:** if *any* contributing sub-form types to `None`
/// (unknown), the whole form is `None` (defer) — `branch_union` enforces this by
/// short-circuiting on the first unknown. This graceful degradation is what keeps
/// the precise return-check warning only when the body type is fully pinned. Returns
/// `None` for a head that isn't one of these forms (the caller then falls through to
/// the call/sig path).
fn control_flow_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    // `(if test then else)` → ty(then) | ty(else). A two-arm `(if test then)`
    // (no else) can also yield `nil`.
    if value::symbol_is(head, kw::IF) {
        return match items.len() {
            4 => branch_union(heap, &[items[2], items[3]], ctx),
            3 => Some(expr_ty(heap, items[2], ctx)?.union(Ty::of(Tag::Nil))),
            _ => None,
        };
    }
    // `(do … last)` → ty(last). Empty `(do)` is `nil`.
    if value::symbol_is(head, kw::DO) {
        return match items.last() {
            Some(&last) if items.len() > 1 => expr_ty(heap, last, ctx),
            _ => Some(Ty::of(Tag::Nil)),
        };
    }
    // `(when test body…)` / `(unless test body…)` → ty(last body form) | nil
    // (the test failing yields `nil`). Surface form; usually already lowered to
    // `(if test (do …) nil)` by macroexpansion, but handled here for fragments.
    if value::symbol_is(head, kw::WHEN) || value::symbol_is(head, kw::UNLESS) {
        let last = *items.last()?;
        if items.len() < 3 {
            return Some(Ty::of(Tag::Nil));
        }
        return Some(expr_ty(heap, last, ctx)?.union(Ty::of(Tag::Nil)));
    }
    // `(let (bindings) body…)` / `(letrec …)` → ty(last body form),
    // typed in a scope with each binding's RHS type threaded in (sequentially).
    if value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LETREC) {
        let binds = let_bindings(heap, *items.get(1)?)?;
        if binds.len() % 2 != 0 {
            return None;
        }
        let mut scope = ctx.clone();
        let mut i = 0;
        while i < binds.len() {
            let Value::Sym(name) = binds[i] else {
                // A destructuring binding — we can't pin the names; the body may
                // depend on them, so defer the whole form.
                return None;
            };
            let rhs_ty = expr_ty(heap, binds[i + 1], &scope);
            scope = scope.bind(name, rhs_ty);
            i += 2;
        }
        let last = *items.last()?;
        if items.len() < 3 {
            return None;
        }
        return expr_ty(heap, last, &scope);
    }
    // `(cond test1 res1 test2 res2 … :else resN)` — union of the *result*
    // positions (every odd index from 2 onward). Surface form (post-expansion this
    // is nested `if`s, handled above); kept for fragments.
    if value::symbol_is(head, kw::COND) {
        let mut results = Vec::new();
        let mut i = 2;
        while i < items.len() {
            results.push(items[i]);
            i += 2;
        }
        if results.is_empty() {
            return None;
        }
        return branch_union(heap, &results, ctx);
    }
    // `(case key v1 res1 v2 res2 … [default])` — `key` at index 1, then `val res`
    // pairs from index 2; a lone trailing item is the default. Collect the result
    // of each pair (index 3, 5, …) plus any trailing default.
    if value::symbol_is(head, kw::CASE) && items.len() >= 4 {
        let clauses = &items[2..];
        let mut results = Vec::new();
        let mut i = 0;
        while i < clauses.len() {
            if i + 1 < clauses.len() {
                results.push(clauses[i + 1]); // the result of this `val res` pair
                i += 2;
            } else {
                results.push(clauses[i]); // a lone trailing default
                i += 1;
            }
        }
        if results.is_empty() {
            return None;
        }
        return branch_union(heap, &results, ctx);
    }
    // `(match scrutinee pat1 body1 pat2 body2 …)` — union of the arm *bodies*
    // (every result position; we ignore pattern-narrowing, which is sound — just
    // less precise). Bodies are at even offsets from index 3.
    if value::symbol_is(head, kw::MATCH) {
        let mut bodies = Vec::new();
        let mut i = 3;
        while i < items.len() {
            bodies.push(items[i]);
            i += 2;
        }
        if bodies.is_empty() {
            return None;
        }
        return branch_union(heap, &bodies, ctx);
    }
    // `(and a b … last)` → union of operand types (a falsy operand short-circuits
    // to itself, so any operand can be the value). `(or a b … last)` likewise.
    // Empty `(and)` → true; empty `(or)` → nil. Surface forms (post-expansion both
    // are `let`+`if`, handled above); kept for fragments.
    if value::symbol_is(head, kw::AND) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Bool));
        }
        return branch_union(heap, &items[1..], ctx);
    }
    if value::symbol_is(head, kw::OR) {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Nil));
        }
        return branch_union(heap, &items[1..], ctx);
    }
    None
}

/// The union of the types of several branch result forms, or `None` if *any* of
/// them is unknown — the graceful-degradation rule that keeps the precise
/// return-check from warning on a form whose value isn't fully pinned.
fn branch_union(heap: &Heap, forms: &[Value], ctx: &Ctx) -> Option<Ty> {
    let mut acc: Option<Ty> = None;
    for &f in forms {
        let t = expr_ty(heap, f, ctx)?;
        acc = Some(match acc {
            Some(a) => a.union(t),
            None => t,
        });
    }
    acc
}

/// Parse a `let`-family bindings form into a flat `[name val name val …]` vec —
/// accepts both `(…)` lists and `[…]` vectors (the two shapes the reader emits),
/// mirroring `walk::bindings`. `None` for any other shape.
fn let_bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
    }
}

/// Result type for the arithmetic ops the curated `(number… -> number)` sigs type
/// too widely — two sound sharpenings that both stay a *subtype* of `number`, so
/// they can only make a result more precise, never wrong:
///
/// - **Integer-closed** ("int op int → int"): an integer operation on integers
///   yields an integer (an i64 or a bignum — both fold to `Tag::Int`, see
///   `value::tag`), so `+ - * quot rem mod abs` with every operand `⊆ int` is
///   exactly `int`. This is what lets `(defn f (x) (* x x))` declared `(int -> int)`
///   not warn. `/` is EXCLUDED here: integer division can yield a float
///   (`(/ 6 2)` → `3`, `(/ 5 2)` → `2.5`).
/// - **Float-contagion** ("anything op float → float"): IEEE/tower contagion means
///   `+ - * /` with any operand *provably* `⊆ float` yields a `float`. Since `float`
///   is disjoint from `int`, this is what catches an `(int -> int)`-declared body
///   doing float arithmetic (`(+ x 1.5)` → `float`), plus the always-float unary
///   math `sqrt sin cos tan` — mismatches the flat `number` sig would silently miss.
///
/// Returns `None` (defer to the curated `number` sig) whenever a rule can't fire
/// with certainty — a non-numeric or unknown operand, a mixed int/`number` set that
/// proves neither all-int nor any-float, or zero operands (a bare `(+)`). Deferring
/// is always sound: the wider `number` never narrows below what the value can be.
fn numeric_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    let int = Ty::of(Tag::Int);
    let float = Ty::of(Tag::Float);
    let num = Ty::NUMBER;

    // Always-float unary math: `sqrt`/`sin`/`cos`/`tan` return a float even for a
    // perfect square / whole-number argument (`(sqrt 4)` → `2.0`). Only fires on a
    // known numeric argument (a non-numeric one is a separate arg-type error the
    // curated sig already flags).
    let is_always_float = value::symbol_is(head, "sqrt")
        || value::symbol_is(head, "sin")
        || value::symbol_is(head, "cos")
        || value::symbol_is(head, "tan");
    if is_always_float {
        let arg = *items.get(1)?;
        let t = expr_ty(heap, arg, ctx)?;
        return t.is_subtype(&num).then_some(float);
    }

    let is_contagious = value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "/");
    let is_int_closed = value::symbol_is(head, "+")
        || value::symbol_is(head, "-")
        || value::symbol_is(head, "*")
        || value::symbol_is(head, "quot")
        || value::symbol_is(head, "rem")
        || value::symbol_is(head, "mod")
        || value::symbol_is(head, "abs");
    if !is_contagious && !is_int_closed {
        return None;
    }
    // Every operand must be a known numeric type; one non-numeric / unknown defers.
    // (Zero operands — e.g. a bare `(+)` — also defers, leaving the curated sig.)
    let args = items.get(1..)?;
    if args.is_empty() {
        return None;
    }
    let mut all_int = true;
    let mut any_float = false;
    for &arg in args {
        let t = expr_ty(heap, arg, ctx)?;
        if !t.is_subtype(&num) {
            return None;
        }
        all_int &= t.is_subtype(&int);
        any_float |= t.is_subtype(&float);
    }
    if is_contagious && any_float {
        return Some(float);
    }
    if is_int_closed && all_int {
        return Some(int);
    }
    None
}

/// Element-aware result type for the sequence builtins — `None` falls through to
/// the callee's flat signature. `(list …)`/`(vector …)` build a refined
/// sequence; `(first xs)`/`(last xs)`/`(nth xs i)` extract the element type
/// (widened with `nil` for the empty / out-of-range case, so the result is a
/// sound superset). Only refines when the element type is actually known.
fn seq_aware_call_ty(heap: &Heap, head: Symbol, items: &[Value], ctx: &Ctx) -> Option<Ty> {
    if value::symbol_is(head, "list") {
        return element_union(heap, &items[1..], ctx).map(Ty::list_of);
    }
    if value::symbol_is(head, "vector") {
        return element_union(heap, &items[1..], ctx).map(Ty::vector_of);
    }
    if value::symbol_is(head, "first")
        || value::symbol_is(head, "last")
        || value::symbol_is(head, "nth")
        || value::symbol_is(head, "second")
        || value::symbol_is(head, "third")
    {
        let arg = *items.get(1)?;
        let coll_ty = expr_ty(heap, arg, ctx)?;
        // A statically-known index into a tuple-typed collection resolves to
        // that *exact* position's type (ADR-128), not just the coarse union
        // every other element access falls back to — `first` = 0, `second` =
        // 1, `third` = 2, `last` = the final position, `nth` reads its own
        // literal-int index argument (a non-literal index can't be resolved
        // this precisely, so it falls through to the union case below).
        if let Some(elems) = coll_ty.tuple_elems() {
            let idx = if value::symbol_is(head, "first") {
                Some(0)
            } else if value::symbol_is(head, "second") {
                Some(1)
            } else if value::symbol_is(head, "third") {
                Some(2)
            } else if value::symbol_is(head, "last") {
                elems.len().checked_sub(1)
            } else if value::symbol_is(head, "nth") {
                match items.get(2) {
                    Some(Value::Int(n)) if *n >= 0 => Some(*n as usize),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(i) = idx {
                // In range → exactly that position's type, no `nil` — a
                // tuple's arity is fixed and known, so an in-range access on a
                // well-typed value is never absent. A provably out-of-range
                // literal index → exactly `nil` (matches the runtime, which
                // returns nil rather than erroring).
                return Some(match elems.get(i) {
                    Some(t) => t.clone(),
                    None => Ty::of(Tag::Nil),
                });
            }
        }
        let elem = coll_ty.elem_ty()?;
        // first/second/third/last/nth yield `nil` on an empty / out-of-range seq.
        return Some(elem.union(Ty::of(Tag::Nil)));
    }
    // `(filter pred coll)` keeps `coll`'s element type — the result is the items
    // that pass, so `nil | list<A>` for `A = elem(coll)` (ADR-078 parametric
    // results). `None` element → fall through to the flat curated `list`.
    if value::symbol_is(head, "filter") {
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // Element-preserving reshapers whose sequence is the *first* argument — the
    // same elements, fewer / reordered: `reverse`, `rest` (drop the head),
    // `but-last`, `distinct` / `dedupe` (drop duplicates). `nil | list<A>`.
    if value::symbol_is(head, "reverse")
        || value::symbol_is(head, "rest")
        || value::symbol_is(head, "but-last")
        || value::symbol_is(head, "distinct")
        || value::symbol_is(head, "dedupe")
    {
        let coll = *items.get(1)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(sort coll)` / `(sort less? coll)` and `(sort-by key-fn coll)` — the
    // sequence is always the last argument; element type is preserved unchanged.
    if value::symbol_is(head, "sort") || value::symbol_is(head, "sort-by") {
        let coll = *items.last()?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // Element-preserving slices/filters whose sequence is the *second* argument —
    // `take` / `drop` / `take-while` / `drop-while`, `take-last` / `drop-last`, and
    // `remove` (the `filter` complement). Element type is preserved unchanged.
    if value::symbol_is(head, "take")
        || value::symbol_is(head, "drop")
        || value::symbol_is(head, "take-while")
        || value::symbol_is(head, "drop-while")
        || value::symbol_is(head, "take-last")
        || value::symbol_is(head, "drop-last")
        || value::symbol_is(head, "remove")
    {
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        return list_result(a);
    }
    // `(cons x xs)` — prepend `x` onto `xs`; the result element type is
    // `type(x) | elem(xs)`. Both must be known; if either is unknown the element
    // type is unknown (the tail may hold values of any type). The result is always
    // a `pair` (not nil), so we return `list<E>` without the `nil` variant.
    if value::symbol_is(head, "cons") && items.len() == 3 {
        let hd_ty = expr_ty(heap, items[1], ctx);
        let tail_elem = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
        return match (hd_ty, tail_elem) {
            (Some(h), Some(e)) => Some(Ty::list_of(h.union(e))),
            _ => Some(Ty::of(Tag::Pair)), // one side unknown → unrefined pair
        };
    }
    // `(append xs ys …)` / `(concat xs ys …)` — variadic list concatenation.
    // Result element type is the union of every argument's element type; any
    // argument with an unknown element type → fall through to the flat result.
    if value::symbol_is(head, "append") || value::symbol_is(head, "concat") {
        if items.len() == 1 {
            return Some(Ty::of(Tag::Nil)); // (append) = nil
        }
        let mut acc: Option<Ty> = None;
        for &arg in &items[1..] {
            let elem = expr_ty(heap, arg, ctx).and_then(|t| t.elem_ty())?;
            acc = Some(match acc {
                Some(a) => a.union(elem),
                None => elem,
            });
        }
        return list_result(acc);
    }
    // Map K/V refinement rules — derive result types when the first argument is a
    // `map<K, V>`.  Sound by the usual "widening is conservative" rule: these rules
    // only fire when K/V are known; unknown → fall through to the curated flat result.
    //
    // `(get m k [default])` → `V | nil` (nil = key absent or default not given).
    // On a record shape (Step 5+, ADR-115) with a *literal keyword* key, the
    // exact declared field type wins — more specific than the flat `map_kv`
    // fallback below. An undeclared/dynamic key (or a non-keyword key on a
    // record) falls through: records are open, so an unknown key's type is
    // genuinely unknown, not an error.
    if value::symbol_is(head, "get") && items.len() >= 3 {
        let map_arg = *items.get(1)?;
        let map_ty = expr_ty(heap, map_arg, ctx);
        if let Value::Keyword(key) = items[2] {
            if let Some((fty, _required)) = map_ty
                .as_ref()
                .and_then(Ty::record_fields)
                .and_then(|f| f.get(&key))
            {
                return Some(fty.clone().union(Ty::of(Tag::Nil)));
            }
        }
        if let Some((_, v)) = map_ty.as_ref().and_then(Ty::map_kv) {
            return Some(v.clone().union(Ty::of(Tag::Nil)));
        }
    }
    // `(keys m)` → `nil | list<K>`.
    if value::symbol_is(head, "keys") && items.len() == 2 {
        let map_arg = *items.get(1)?;
        if let Some((k, _)) = expr_ty(heap, map_arg, ctx).as_ref().and_then(Ty::map_kv) {
            return list_result(Some(k.clone()));
        }
    }
    // `(vals m)` → `nil | list<V>`.
    if value::symbol_is(head, "vals") && items.len() == 2 {
        let map_arg = *items.get(1)?;
        if let Some((_, v)) = expr_ty(heap, map_arg, ctx).as_ref().and_then(Ty::map_kv) {
            return list_result(Some(v.clone()));
        }
    }
    // `(assoc m k1 v1 …)` → `map<K, V>`, preserving the input's refinement.
    // We only carry the refinement forward; we don't try to refine based on the
    // new k/v arguments (too expensive, no false-positive risk either way).
    if value::symbol_is(head, "assoc") && items.len() >= 4 && (items.len() - 2).is_multiple_of(2) {
        let map_arg = *items.get(1)?;
        if let Some((k, v)) = expr_ty(heap, map_arg, ctx).as_ref().and_then(Ty::map_kv) {
            return Some(Ty::map_of(k.clone(), v.clone()));
        }
    }
    // `(map f coll)` → `nil | list<B>`, `B` = the callback's return type applied
    // to `coll`'s element type. Unknown callback / element → flat `list`.
    if value::symbol_is(head, "map") {
        let f = *items.get(1)?;
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result(b);
    }
    // `(keep f coll)` — `map` then drop the `nil` results; `nil | list<B>` for
    // `B` = the callback's return (over-approximated by keeping `nil` in `B`, a
    // sound superset). Unknown callback / element → flat.
    if value::symbol_is(head, "keep") {
        let f = *items.get(1)?;
        let coll = *items.get(2)?;
        let a = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[a], ctx);
        return list_result(b);
    }
    // `(interpose sep coll)` — weave `sep` between `coll`'s elements; the result
    // holds both, `nil | list<A | type(sep)>`. Both must be known, else flat.
    if value::symbol_is(head, "interpose") && items.len() == 3 {
        let sep_ty = expr_ty(heap, items[1], ctx);
        let a = expr_ty(heap, items[2], ctx).and_then(|t| t.elem_ty());
        return match (sep_ty, a) {
            (Some(s), Some(e)) => list_result(Some(s.union(e))),
            _ => None,
        };
    }
    // `(range …)` always produces numbers — `nil | list<number>` (empty for an
    // empty range). A sound superset whatever the bound types (int or float).
    if value::symbol_is(head, "range") {
        return list_result(Some(Ty::NUMBER));
    }
    // `(reduce f init coll)` / `(fold f init coll)` reduce to an accumulator typed
    // `ty(init) | B`, where `B` is the 2-arg callback's return (`(f acc x)`). The
    // accumulator can grow across steps, so it's over-approximated as `any` for
    // the callback inference (sound — a superset); the result joins the
    // empty-input case (`init`) with a step result (`B`). The no-init
    // `(reduce f coll)` form starts the accumulator at `coll`'s first element.
    // Both `init` and `B` must be known, else flat.
    if value::symbol_is(head, "reduce") || value::symbol_is(head, "fold") {
        let f = *items.get(1)?;
        let (init_ty, coll) = match items.len() {
            // (fold f init coll) / (reduce f init coll)
            4 => (expr_ty(heap, items[2], ctx), items[3]),
            // (reduce f coll) — initial accumulator is the first element
            3 if value::symbol_is(head, "reduce") => {
                let coll = items[2];
                let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
                (elem, coll)
            }
            _ => return None,
        };
        let elem = expr_ty(heap, coll, ctx).and_then(|t| t.elem_ty());
        let b = callback_ret(heap, f, &[Some(Ty::ANY), elem], ctx);
        return match (init_ty, b) {
            (Some(i), Some(b)) => Some(i.union(b)),
            _ => None,
        };
    }
    None
}

/// The result type of a list-producing combinator (`map`/`filter`): `nil |
/// list<elem>` — empty input maps/filters to `nil`. `None` element → `None`, so
/// the caller falls back to the flat curated `list` (never a too-narrow result).
fn list_result(elem: Option<Ty>) -> Option<Ty> {
    elem.map(|e| Ty::list_of(e).union(Ty::of(Tag::Nil)))
}

/// The return type of a HOF callback `f` whose parameters receive the given
/// `inputs` types (`[elem]` for `map`'s `(f x)`; `[any, elem]` for `reduce`/`fold`'s
/// `(f acc x)`, the accumulator over-approximated as `any`). A `None` input is an
/// unknown parameter type.
/// - a named **global** fn → its signature's return type (`sig_of`);
/// - a straight-line lambda with exactly `inputs.len()` plain params → `body`'s
///   type with each `pᵢ` bound to `inputs[i]` (identity preserves its input);
/// - anything else (a local var, an unknown form) → `None` (flat result).
///
/// The lambda case is the only new inference, and it only computes a *forward*
/// result type — it never *checks* the body, so it doesn't reopen the deferred
/// guarded-use false-positive class.
fn callback_ret(heap: &Heap, f: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    match f {
        // A local binding shadows the global table — its return type isn't known.
        Value::Sym(s) if ctx.is_local(s) => None,
        Value::Sym(s) => {
            // An overloaded callback (ADR-116) — resolve per matching arm from
            // `inputs` instead of a single flat `ret`, same as the call-form case.
            if let Some(sigs) = declared_heap_overload(heap, s) {
                return Some(resolve_overload_ret(&sigs, inputs));
            }
            sig_of(heap, s).map(|sig| sig.ret)
        }
        Value::Pair(_) => lambda_ret(heap, f, inputs, ctx),
        _ => None,
    }
}

/// The return type of a **simple** single-clause lambda `(fn (p…) body)` —
/// exactly `inputs.len()` plain-symbol parameters and one body expression —
/// computed by binding each `pᵢ` to `inputs[i]` and typing `body`. `None` for
/// anything subtler (wrong param count / multi-body / docstring / variadic /
/// destructuring / non-`fn` head), so the result stays flat.
fn lambda_ret(heap: &Heap, form: Value, inputs: &[Option<Ty>], ctx: &Ctx) -> Option<Ty> {
    let items = list_items(heap, form)?;
    let Some(Value::Sym(head)) = items.first().copied() else {
        return None;
    };
    if !is_fn_head(head) {
        return None;
    }
    // Exactly `(fn <param-list> <body>)` — one param list + one body expression.
    let parts = &items[1..];
    if parts.len() != 2 {
        return None;
    }
    let params = list_items(heap, parts[0])?;
    if params.len() != inputs.len() {
        return None; // arity must match what the combinator supplies
    }
    let mut sub = ctx.clone();
    for (param, input) in params.iter().zip(inputs) {
        let Value::Sym(p) = param else {
            return None; // not a plain-symbol parameter
        };
        sub = sub.bind(*p, input.clone());
    }
    expr_ty(heap, parts[1], &sub)
}
