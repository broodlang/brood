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

use super::ctx::{Ctx, PathKey};
use super::infer::expr_ty;
use super::walk::list_items;

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
