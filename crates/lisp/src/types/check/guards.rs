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
use crate::types::{Range, Ty};

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
            | kw::MATCH
            | kw::CASE
            | kw::COMMENT
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
// Names that route through `SkipBody`: `quote`, `quasiquote`, `comment` —
// syntax, never evaluated, so nothing in them is a reference.
//
// `try`, `%try`, `error-of` and `assert-error` route through `ErrorTesting`
// instead (KI-67): the walk DOES descend, with every lint but `unbound`
// suppressed. They deliberately exercise failures, so `(error-of (cons 1))`
// must stay silent about the misuse — but an unbound symbol in there is a dead
// call site, not the failure under test, and skipping the body outright let a
// rename wave ship a broken `try` with every gate green. `%try` matters
// post-expansion: macroexpand rewrites `(try …)` to
// `(%try (fn () body) (fn (e) handler))` before `check_file` walks the tree.

/// A recognised type guard over a single variable: when `test` is truthy, `sym`
/// provably has type `ty`. `then_only` marks a guard whose *negation is unsound*
/// — a falsy `test` does **not** establish `¬ty`, so the else-branch must not be
/// narrowed (the `and` short-circuit is the case: a falsy `and` may have failed
/// on a *later* conjunct, so the first conjunct can still hold). An ordinary type
/// predicate is biconditional (`then_only = false`): the else-branch narrows to
/// `¬ty` soundly. `else_only` is the dual — a truthy `test` establishes NOTHING about
/// `sym` (`(empty? xs)` holds for `nil`, `[]`, `""` and `#{}` alike, and the lattice has
/// no "empty vector"), while a falsy one establishes `¬ty` (`xs` is not `nil`): only the
/// else-branch narrows. Negation swaps the two: `(not (empty? xs))` is a `then_only`
/// guard of `¬nil`, and `(not <then_only>)` is an `else_only` guard of the complement,
/// sound for the same reason each side is.
pub(super) struct Guard {
    pub(super) sym: Symbol,
    pub(super) ty: Ty,
    pub(super) then_only: bool,
    pub(super) else_only: bool,
    /// What a FALSY test proves, stated positively, when that is sharper than `¬ty` —
    /// `(empty? xs)` false is "a countable of length at least 1", which the complement
    /// of the then-type only says as a subtraction no length reader can see (ADR-350).
    /// `None`: the else-branch narrows by `ty.negate()`, as ever.
    pub(super) else_ty: Option<Ty>,
}

impl Guard {
    /// The type the else-branch narrows `sym` to.
    pub(super) fn else_type(&self) -> Ty {
        self.else_ty
            .clone()
            .unwrap_or_else(|| self.ty.clone().negate())
    }
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
    /// The guarded access form itself — `(nth a 0)` — whose own type seeds the narrowing.
    pub(super) subject: Value,
    pub(super) ty: Ty,
    pub(super) then_only: bool,
}

/// The unary primitives a guard may narrow **through** — a call to one of these is a
/// [`PathKey::Call`] step, so `(if (failure? (string/->number s)) d (string/->number s))`
/// types the second occurrence as `number`, not `number | failure`.
///
/// The soundness argument is the one `path_types` already rests on: Brood data is
/// immutable, so an argument cannot change between two evaluations, and each of these is a
/// function of its argument alone — the two occurrences denote one value. Every entry here
/// is a *parser*, which is not a coincidence: these are the functions that answer
/// `T | failure` (ADR-310), so they are the ones people guard and re-evaluate.
///
/// **An allow-list, deliberately.** The opposite polarity — "narrow through anything not
/// known to be effectful" — would silently admit `os/env`, `now`, `random` and every
/// primitive added later, each of which can answer differently the second time; that is
/// the direction that makes the bound stop being an upper bound. An unlisted head simply
/// does not narrow, which costs precision and nothing else.
pub(super) const DETERMINISTIC_UNARY: &[&str] = &[
    "string/->number",
    "encoding/hex-decode",
    "encoding/hex-decode-bytes",
    "encoding/base64-decode",
    "encoding/base64-url-decode",
    "encoding/base64-decode-bytes",
    "encoding/base64-url-decode-bytes",
    "datetime/parse-date",
    "datetime/parse-time",
    "datetime/parse-iso8601",
    "url/percent-decode",
    "url/query-decode",
];

/// Is this path's base a GLOBAL — a `def`/`defdyn` name rather than a local binding?
///
/// Asked of the heap rather than the `Ctx`, deliberately: a `Ctx` only knows the binders it
/// happens to track, and inference reaches `branch_scopes` through contexts that track
/// none — so `!ctx.is_local(..)` rejects ordinary parameters and silently switches the
/// narrowing off (measured: 8 strict warnings over `std/`, all of them locals). "Not
/// globally bound" is the property that actually matters and the heap always knows it.
fn base_is_global(heap: &Heap, base: Symbol) -> bool {
    super::sigs::is_globally_bound(heap, base)
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
pub(in crate::types::check) fn path_of(heap: &Heap, expr: Value) -> Option<(Symbol, Vec<PathKey>)> {
    // A loop, not recursion, and capped: `expr_ty` asks this at its FIRST level for
    // every form, so a chain of n accessors cost O(n) frames and an O(n) key vector
    // per level — O(n²) memory, O(n³) copying on `(first (first … x))` 8k deep. A path
    // longer than [`MAX_PATH_KEYS`] is no narrowing anyone wrote by hand.
    let mut keys_outer_first: Vec<PathKey> = Vec::new();
    let mut expr = expr;
    loop {
        if let Value::Sym(s) = expr {
            keys_outer_first.reverse();
            return Some((s, keys_outer_first));
        }
        if keys_outer_first.len() >= MAX_PATH_KEYS {
            return None;
        }
        let items = list_items(heap, expr)?;
        // The keyword-call read `(:k m)` is `(get m :k)` spelled the other way round — the
        // spelling every `(when (:proc state) (os/close (:proc state)))` uses.
        if let (Some(&Value::Keyword(k)), 2) = (items.first(), items.len()) {
            keys_outer_first.push(PathKey::Field(k));
            expr = items[1];
            continue;
        }
        let Some(&Value::Sym(head)) = items.first() else {
            return None;
        };
        let (inner, key) = if value::symbol_is(head, "get") && items.len() == 3 {
            let Value::Keyword(k) = items[2] else {
                return None;
            };
            (items[1], PathKey::Field(k))
        } else if (value::symbol_is(head, "nth") || value::symbol_is(head, "%vector-ref"))
            && items.len() == 3
        {
            // `%vector-ref` is the `match` compiler's spelling of a positional read
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
        } else if items.len() == 2
            && DETERMINISTIC_UNARY
                .iter()
                .any(|name| value::symbol_is(head, name))
        {
            (items[1], PathKey::Call(head))
        } else {
            return None;
        };
        keys_outer_first.push(key);
        expr = inner;
    }
}

/// The longest access path [`path_of`] will describe. Anything deeper is machine-made.
const MAX_PATH_KEYS: usize = 32;

/// If `test` is a type predicate applied to an access path — or its `(not …)` —
/// return the [`PathGuard`] it asserts. Handles arbitrary nesting of field/index
/// steps via [`path_of`]; a computed key/index or a non-path form is left alone
/// (no narrowing, no false positive), and a bare variable (empty path) is
/// deferred to the plain [`guard_assertion`]. Mirrors that function's structure.
pub(super) fn path_guard_assertion(heap: &Heap, test: Value) -> Option<PathGuard> {
    // Deep-form stack safety: `(not (not (not …)))` recurses one frame per `not`.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        path_guard_assertion_inner(heap, test)
    })
}

fn path_guard_assertion_inner(heap: &Heap, test: Value) -> Option<PathGuard> {
    let items = list_items(heap, test)?;
    let Value::Sym(head) = *items.first()? else {
        // A keyword-call read `(:k m)` is a path too — the bare-path case below.
        return bare_path_guard(heap, test);
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
    // `(= <get-path> lit)` — an equality test on a path against an exact literal
    // (ADR-350): biconditional exactly as the variable form is, and for the same reason
    // — the literal's complement is representable. This is how a tagged-tuple dispatch
    // `(if (= (nth a 0) :error) … (nth a 1))` narrows `a`'s first position in BOTH
    // branches, so the positional read in the else branch sees only the `:ok` shape.
    if items.len() == 3 && (head_name == kw::EQ_PRIM || head_name == "=") {
        let is_path = |form: Value| path_of(heap, form).is_some_and(|(_, keys)| !keys.is_empty());
        let (path, lit) = if is_path(items[1]) {
            (items[1], items[2])
        } else {
            (items[2], items[1])
        };
        let ty = match lit {
            Value::Keyword(_) | Value::Int(_) | Value::Bool(_) | Value::Nil => Ty::of_value(lit),
            Value::Str(id) => Ty::str_lit(&heap.string(id)),
            _ => return None,
        };
        let (base, keys) = path_of(heap, path)?;
        if keys.is_empty() {
            return None;
        }
        return Some(PathGuard {
            base,
            keys,
            subject: path,
            ty,
            then_only: false,
        });
    }
    // `<get-path>` as the test itself — `(when (:proc state) (os/close (:proc state)))`:
    // a truthy read proves the path is not `nil | false`, a falsy one that it is
    // (biconditional, as the bare-variable form in `guard_assertion` is), so the idiom
    // that guards a maybe-field by reading it narrows the read under it. `Ty::truthy()`
    // is the one definition of falsiness in the checker.
    if let Some(guard) = bare_path_guard(heap, test) {
        return Some(guard);
    }
    // `(pred? <get-path>)` — a type predicate over a (possibly nested) field path.
    if items.len() != 2 {
        return None;
    }
    let (ty, then_only) = predicate_guard_ty(heap, None, head)?;
    let (base, keys) = path_of(heap, items[1])?;
    if keys.is_empty() {
        return None; // a bare variable — `guard_assertion` handles that
    }
    Some(PathGuard {
        base,
        keys,
        subject: items[1],
        ty,
        then_only,
    })
}

/// The guard a bare access path asserts as a test: `(when (:proc state) …)` / `(if (get
/// r :n) …)` — a truthy read proves the path is not `nil | false`, a falsy one that it
/// is (biconditional, as the bare-variable form in [`guard_assertion`] is). A bare
/// variable (empty path) is that function's. `Ty::truthy()` is the one definition of
/// falsiness in the checker.
fn bare_path_guard(heap: &Heap, test: Value) -> Option<PathGuard> {
    let (base, keys) = path_of(heap, test).filter(|(_, keys)| !keys.is_empty())?;
    Some(PathGuard {
        base,
        keys,
        subject: test,
        ty: Ty::truthy(),
        then_only: false,
    })
}

/// If `test` is a recognisable type guard over a single variable, return the
/// [`Guard`] it implies. A leading `(not …)` flips the assertion via
/// [`Ty::negate`]. A bare `Sym` is looked up in `ctx`'s guard-alias table (a
/// `let`-stored guard result — `(let (cond (int? x)) (if cond …))`). `None` for
/// any test that isn't a pure single-variable guard.
pub(super) fn guard_assertion(heap: &Heap, test: Value, ctx: &Ctx) -> Option<Guard> {
    // Deep-form stack safety: recurses on `(not …)`, from `check_if` — 3 677 frames
    // deep on the negated-guard test before this wrap.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        guard_assertion_inner(heap, test, ctx)
    })
}

fn guard_assertion_inner(heap: &Heap, test: Value, ctx: &Ctx) -> Option<Guard> {
    if let Value::Sym(s) = test {
        // A let-stored guard alias — recorded for biconditional guards (see `check_let`),
        // so it narrows the else-branch too. A `when`-shaped alias is NOT the guard here:
        // its value is data (`(let (src (when k (lookup k))) …)`), so the test narrows
        // `src` itself by truthiness below, and `and_conjunct_guards` adds `k` beside it.
        if let Some((sym, ty, else_ty, false)) = ctx.guard(s) {
            return Some(Guard {
                sym,
                ty,
                then_only: false,
                else_only: false,
                else_ty,
            });
        }
        // **Truthiness.** A bare local as the test is itself a guard: `nil`, `false`
        // and a `failure` are falsy (`eval::truthy`), so a true test means the local is
        // none of them. This is what `if-let` / `when-let` expand to — `(let (v expr) (if v
        // then else))` — and without it the *then* branch sees the binding's
        // unnarrowed type. Invisible while a map lookup was untyped; once a closed
        // literal made `(get {:x 10} :y)` exactly `nil` (ADR-264), `(if-let (v (get
        // {:x 10} :y)) (inc v) …)` read as handing `nil` to `inc` — a false positive
        // on a branch that cannot run.
        //
        // **Biconditional, because the type is now exact.** Truthy is `¬(nil ∪ false)`.
        // That was unsayable while negating a literal set widened to its whole tag —
        // the closest was `not nil`, a sound *necessary* condition for a true test but
        // not invertible (a false test does not imply `nil`, since `false` is falsy
        // too), so this guard had to be one-sided, and marking it biconditional read
        // `(not v)` as "v is nil" and reported live code as dead.
        //
        // `Ty::negate` now complements a **bool** literal set exactly — the one literal
        // kind with a finite domain, where `¬{false}` really is `{true}` — so the type
        // below *is* truthy, and its complement really is falsy. Both branches narrow.
        if ctx.is_lexical_local(s) {
            return Some(Guard {
                sym: s,
                // `Ty::truthy()` — the ONE definition of falsiness in the checker, so it
                // cannot drift from the evaluator's. It used to be spelled out here as
                // `nil ∪ false` negated, which silently excluded `failure` when that kind
                // arrived and left `(or (parse s) 0)` reading `number | failure`.
                ty: Ty::truthy(),
                then_only: false,
                else_only: false,
                else_ty: None,
            });
        }
        return None;
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
        // Negation swaps the one-sided flags: the then-branch of `(not G)` is the
        // else-branch of `G` and vice versa (see `Guard`).
        // …and an explicit else-type becomes the then-type, the then-type the else-type.
        let negated = inner.else_type();
        return Some(Guard {
            sym: inner.sym,
            ty: negated,
            then_only: inner.else_only,
            else_only: inner.then_only,
            else_ty: Some(inner.ty),
        });
    }
    // `(%eq sym literal)` / `(%eq literal sym)` — equality against a literal
    // asserts the variable has the literal's runtime tag. The `match` pattern
    // compiler emits this for literal patterns (e.g. `(match x (5 …))`
    // lowers through `(let (m x) (if (%eq m 5) …))` — and the let-alias
    // machinery threads the narrowing back to `x`). Variadic `=` reaches us
    // pre-expanded as `%eq` calls when arities are 2, so we only need to
    // recognise the primitive shape.
    // `(= a b)` reaches the checker unexpanded (`=` is a Brood variadic over `%eq`), and
    // with two operands it IS `%eq` — so a `cond` clause `(= item :done)` narrows the
    // clauses below it exactly as the primitive spelling does.
    if items.len() == 3 && (head_name == kw::EQ_PRIM || head_name == "=") {
        // `(= :table (type-of x))` — a tag test spelled through `type-of`, which the
        // optimiser's tally rewrite emits on the way out of every in-place fold (ADR-360
        // §6: `type-of` is a total PrimOp1, a predicate call is not). Exactly the guard
        // `(table? x)` is, both branches: `type-of` answers one keyword per tag, so the
        // else branch is the complement. Without it a LOADED `seq/frequencies` read
        // `map | table` at every call (KI-173, 2026-09-20).
        if let Some((sym, ty)) = type_of_eq_guard(heap, ctx, items[1], items[2])
            .or_else(|| type_of_eq_guard(heap, ctx, items[2], items[1]))
        {
            return Some(Guard {
                sym,
                ty,
                then_only: false,
                else_only: false,
                else_ty: None,
            });
        }
        if let Some((sym, ty)) = literal_eq_guard(heap, items[1], items[2])
            .or_else(|| literal_eq_guard(heap, items[2], items[1]))
        {
            // **The else-branch narrows exactly when the guard's type is exactly the
            // values `=` compares against** — a literal set, whose complement became
            // representable, or `nil`, whose tag holds a single value. That is what
            // makes a tagged-union dispatch refine: after `(= tag :ok)` fails, a
            // `(or :ok :err)` tag is `:err` rather than unnarrowed.
            //
            // Anything else stays one-sided, and a string literal is the case that
            // matters: `of_value` has no heap to read the bytes, so `(= m "x")` yields
            // the bare `string` tag. Negating *that* claims `m` is not a string at all
            // — which is how this guard came to be one-sided in the first place (it
            // flagged a valid `(string/length m)` in the else branch).
            let exact = ty.as_lit().is_some()
                || ty.as_lit_int().is_some()
                || ty.as_lit_bool().is_some()
                || ty.as_lit_str().is_some()
                || ty == Ty::of(Tag::Nil);
            return Some(Guard {
                sym,
                ty,
                then_only: !exact,
                else_only: false,
                else_ty: None,
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
    // `(empty? x)`: nothing sayable when true (`nil`, `[]`, `""`, `#{}` all are), and `x`
    // is not `nil` when false — the else-only guard, and the idiom every list walk is
    // built on: `(if (empty? xs) acc (… (first xs) …))`.
    if head_name == "empty?" && !ctx.is_lexical_local(head) && !ctx.is_file_global(head) {
        // BICONDITIONAL by length (ADR-350): true, `x` is `nil` or a countable of length
        // 0 (no `pair` — a list of length 0 is `nil`); false, `x` is a countable of
        // length at least 1, or something that is not `nil` and not countable at all.
        return match items[1] {
            Value::Sym(s) => Some(Guard {
                sym: s,
                ty: Ty::of(Tag::Nil).union(
                    Ty::ANY
                        .difference(Ty::of(Tag::Nil))
                        .with_len(Range::point(0)),
                ),
                then_only: false,
                else_only: false,
                else_ty: Some(
                    Ty::ANY
                        .difference(Ty::of(Tag::Nil))
                        .with_len(Range::at_least(1)),
                ),
            }),
            _ => None,
        };
    }
    let (ty, then_only) = predicate_guard_ty(heap, Some(ctx), head)?;
    match items[1] {
        Value::Sym(s) => Some(Guard {
            sym: s,
            ty,
            then_only,
            else_only: false,
            else_ty: None,
        }),
        _ => None,
    }
}

/// What a truthy `(head x)` proves about `x`, and whether ONLY a truthy one proves
/// anything (`then_only` — the predicate's negation is unsound, `Ty::implied_by`): the
/// built-in predicates' table (`Ty::tested_by`), else a DECLARED type guard (ADR-301) — a
/// sig whose result is `(is T)`, read from this file's declarations when a `ctx` is at
/// hand, else from the loaded image (`sig_of`, which also covers the prelude's own). A
/// local shadowing the name is not the predicate.
pub(super) fn predicate_guard_ty(
    heap: &Heap,
    ctx: Option<&Ctx>,
    head: Symbol,
) -> Option<(Ty, bool)> {
    let head_name = value::symbol_name(head);
    if let Some(t) = Ty::tested_by(&head_name) {
        return Some((t, false));
    }
    if let Some(t) = Ty::implied_by(&head_name) {
        return Some((t, true));
    }
    // A LEXICAL binder shadowing the name is not the predicate. `is_local` also answers true
    // for this file's own globals, and a same-file `(defn myint? …)` is exactly where a
    // declared guard lives — so that test returned `None` here for every user guard, and
    // ADR-301 narrowed nothing but the prelude's (KI-164).
    if ctx.is_some_and(|c| c.is_local(head) && !c.is_file_global(head)) {
        return None;
    }
    ctx.and_then(|c| c.declared_sig(head))
        .or_else(|| super::sigs::sig_of(heap, head))
        .and_then(|sig| sig.guard)
        .map(|t| (t, false))
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
    if inner.else_only {
        return None; // a truthy conjunct that proves nothing proves nothing of the `and`
    }
    Some(Guard {
        then_only: true, // a falsy `and` doesn't establish `¬E`
        ..inner
    })
}

/// The `(cond, rest)` of a two-armed short-circuit expansion `(let (g cond) (if g A B))`
/// where `A`/`B` are each either `g` or the remaining chain. `want_then_g` selects the
/// shape: `true` for `and` — `(if g rest g)` (then is the remainder, else is `g`); `false`
/// for `or` — `(if g g rest)` (then is `g`, else is the remainder). Returns `(cond, rest)`.
fn chain_shape(heap: &Heap, test: Value, want_then_g: bool) -> Option<(Value, Value)> {
    let items = list_items(heap, test)?;
    if items.len() != 3
        || !matches!(items.first(), Some(&Value::Sym(h)) if value::symbol_is(h, kw::LET))
    {
        return None;
    }
    let bs = list_items(heap, items[1])?;
    if bs.len() != 2 {
        return None;
    }
    let Value::Sym(g) = bs[0] else { return None };
    let cond = bs[1];
    let body = list_items(heap, items[2])?;
    let is_if = matches!(body.first(), Some(&Value::Sym(s)) if value::symbol_is(s, kw::IF));
    if body.len() != 4 || !is_if {
        return None;
    }
    let is_g = |v: Value| matches!(v, Value::Sym(s) if s == g);
    // `and`: (if g REST g) → then is the remainder. `or`: (if g g REST) → else is it.
    if want_then_g {
        // and-shape: test == g, else == g, remainder is the THEN slot.
        if is_g(body[1]) && is_g(body[3]) {
            return Some((cond, body[2]));
        }
    } else {
        // or-shape: test == g, then == g, remainder is the ELSE slot.
        if is_g(body[1]) && is_g(body[2]) {
            return Some((cond, body[3]));
        }
    }
    None
}

/// The two scopes an `(if test …)` branches into: the then-branch narrowed by what a truthy
/// `test` proves (a single guard, every conjunct of an `and`-expansion, a same-variable
/// `or`-union), the else-branch by the complement of what is biconditional (a plain guard,
/// every disjunct of an `or`-expansion; never a `then_only` `and`-conjunct — a falsy `and`
/// may have failed on a later conjunct). The one construction the checker's three `if` readers share —
/// `check_if` (the walk), `gradual_of` (the checked value type) and `expr_ty` (the inferred
/// type, which drives a function's inferred return) — so all three see the same branch
/// types. `expr_ty` used to union both branches under the UNnarrowed scope, so
/// `(or (string/->number s) -1)` inferred `nil | number`: the truthy half of an `or` is the
/// one case this reads every time, and every `--strict` caller of such a function paid
/// for the `nil` that cannot occur.
pub(super) fn branch_scopes(heap: &Heap, test: Value, ctx: &Ctx) -> (Ctx, Ctx) {
    let (mut then_ctx, mut else_ctx) = match guard_assertion(heap, test, ctx) {
        Some(g) => {
            let then_ctx = if g.else_only {
                ctx.clone()
            } else {
                ctx.narrow(g.sym, g.ty.clone())
            };
            let else_ctx = if g.then_only {
                ctx.clone()
            } else {
                ctx.narrow(g.sym, g.else_type())
            };
            (then_ctx, else_ctx)
        }
        None => (ctx.clone(), ctx.clone()),
    };
    // A **path** guard narrows both branches too — `(if (failure? (parse s)) d (parse s))`
    // types the else occurrence as the non-failure half. The checking walk layers this
    // separately (with a base-record refinement inference does not need); without it here,
    // an inferred RETURN type kept a `failure` arm the guard had just ruled out, and
    // ADR-316 then reported that arm at every call site — a false positive on a function
    // that cannot fail.
    // The base must not be a GLOBAL. Immutability is what makes the two occurrences the
    // same value, and it covers a local binding — never a global, which another process can
    // `def` between the guard and the use (late binding over the shared code region,
    // ADR-013). A `defdyn` is the same case: its root binding is a global.
    if let Some(pg) = path_guard_assertion(heap, test).filter(|pg| !base_is_global(heap, pg.base)) {
        // Seed both sides with the guarded expression's OWN type. `narrow_path` intersects
        // with whatever the path already narrowed to, which starts at `any` — so without
        // this the else branch reads `¬failure`, a true statement and a wider one than the
        // `number` the expression structurally has, and the inferred return says
        // `(not failure)` where it should say `number`.
        let guarded = super::infer::expr_ty(heap, pg.subject, ctx).unwrap_or(Ty::ANY);
        then_ctx = then_ctx.narrow_path(
            pg.base,
            pg.keys.clone(),
            guarded.clone().intersect(pg.ty.clone()),
        );
        if !pg.then_only {
            else_ctx = else_ctx.narrow_path(pg.base, pg.keys, guarded.intersect(pg.ty.negate()));
        }
    }
    for g in and_conjunct_guards(heap, test, ctx) {
        then_ctx = then_ctx.narrow(g.sym, g.ty);
    }
    for g in or_disjunct_guards(heap, test, ctx) {
        else_ctx = else_ctx.narrow(g.sym, g.else_type());
    }
    if let Some((sym, union)) = or_same_var_narrowing(heap, test, ctx) {
        then_ctx = then_ctx.narrow(sym, union);
    }
    // …and what a COMPARISON proves: an int's interval, a collection's length, an index
    // bound (ADR-350).
    apply_comparison_facts(heap, test, ctx, then_ctx, else_ctx)
}

/// Every conjunct guard of an `and`-expansion test — a truthy `and` proves **all**
/// conjuncts hold, so each narrows the *then*-branch (each `then_only`: a falsy `and`
/// proves nothing). `[]` when `test` is not an and-expansion (so a plain guard, already
/// handled by [`guard_assertion`], adds nothing here). Sound to apply all to the then-ctx.
pub(super) fn and_conjunct_guards(heap: &Heap, test: Value, ctx: &Ctx) -> Vec<Guard> {
    let mut out = Vec::new();
    // A bare local bound `when`-shaped — `(let (src (when k E)) (if src …))` — proves its
    // condition `k` truthy in the then-branch, beside its own truthiness (which
    // `guard_assertion` states). Then-only: a falsy `src` may be `E`'s own nil.
    if let Value::Sym(s) = test {
        if let Some((k, ty, _, true)) = ctx.guard(s) {
            out.push(Guard {
                sym: k,
                ty,
                then_only: true,
                else_only: false,
                else_ty: None,
            });
        }
    }
    let mut cur = test;
    let mut matched = false;
    loop {
        match chain_shape(heap, cur, true) {
            Some((cond, rest)) => {
                matched = true;
                if let Some(g) = guard_assertion(heap, cond, ctx).filter(|g| !g.else_only) {
                    out.push(Guard {
                        then_only: true,
                        ..g
                    });
                }
                cur = rest;
            }
            None => {
                // The last conjunct is a bare guard expression (only counted once we've
                // seen at least one `and` link, so a non-`and` test yields nothing).
                if matched {
                    if let Some(g) = guard_assertion(heap, cur, ctx).filter(|g| !g.else_only) {
                        out.push(Guard {
                            then_only: true,
                            ..g
                        });
                    }
                }
                break;
            }
        }
    }
    out
}

/// If `test` is an `or`-expansion whose disjuncts are **all** biconditional guards over
/// the **same** variable, return `(sym, ⋃ tyᵢ)`: the then-branch narrows `sym` to the
/// union (a truthy `or` ⇒ some disjunct holds — and only with one variable is that a
/// statement about a named one). The else-branch is [`or_disjunct_guards`]'s, which
/// needs no shared variable. `None` the moment a disjunct is `then_only`, targets
/// another variable, or isn't a recognised guard.
pub(super) fn or_same_var_narrowing(heap: &Heap, test: Value, ctx: &Ctx) -> Option<(Symbol, Ty)> {
    let mut cur = test;
    let mut sym: Option<Symbol> = None;
    let mut union = Ty::NEVER;
    let mut matched = false;
    // Fold one disjunct's guard into the accumulator; returns `false` to abort.
    let take = |g: Option<Guard>, sym: &mut Option<Symbol>, union: &mut Ty| -> bool {
        match g {
            Some(guard) if !guard.then_only && !guard.else_only => {
                match sym {
                    None => *sym = Some(guard.sym),
                    Some(s) if *s == guard.sym => {}
                    _ => return false, // a different variable — no single-var narrowing
                }
                *union = union.clone().union(guard.ty);
                true
            }
            _ => false,
        }
    };
    loop {
        match chain_shape(heap, cur, false) {
            Some((cond, rest)) => {
                matched = true;
                if !take(guard_assertion(heap, cond, ctx), &mut sym, &mut union) {
                    return None;
                }
                cur = rest;
            }
            None => {
                if !matched {
                    return None;
                }
                if !take(guard_assertion(heap, cur, ctx), &mut sym, &mut union) {
                    return None;
                }
                break;
            }
        }
    }
    sym.map(|s| (s, union))
}

/// Every disjunct guard of an `or`-expansion test that a FALSY `or` refutes — the dual of
/// [`and_conjunct_guards`]: a falsy `or` proves **every** disjunct falsy, so each
/// disjunct's negation narrows the *else*-branch, each on its own variable (`(or
/// (nil? root) (empty? files))` proves `root` is not `nil` AND `files` is not `nil`
/// after it fails). A `then_only` disjunct is left out — `(and (int? x) …)` as a
/// disjunct being falsy proves nothing about `x` — and so is one the reader cannot
/// name. `[]` when `test` is not an or-expansion. Returned as `else_only` guards: what a
/// truthy `or` proves is one disjunct, not a named one (see [`or_same_var_narrowing`]).
pub(super) fn or_disjunct_guards(heap: &Heap, test: Value, ctx: &Ctx) -> Vec<Guard> {
    let mut out = Vec::new();
    let mut cur = test;
    let mut matched = false;
    loop {
        match chain_shape(heap, cur, false) {
            Some((cond, rest)) => {
                matched = true;
                if let Some(g) = guard_assertion(heap, cond, ctx).filter(|g| !g.then_only) {
                    out.push(Guard {
                        else_only: true,
                        else_ty: None,
                        ..g
                    });
                }
                cur = rest;
            }
            None => {
                if matched {
                    if let Some(g) = guard_assertion(heap, cur, ctx).filter(|g| !g.then_only) {
                        out.push(Guard {
                            else_only: true,
                            else_ty: None,
                            ..g
                        });
                    }
                }
                break;
            }
        }
    }
    out
}

/// If `a` is `(type-of sym)` and `b` a keyword naming a tag, the guard `(sym, that tag)`.
/// `None` for a keyword that names no tag (`(= :foo (type-of x))` asserts nothing a type
/// can say) and for any other shape.
fn type_of_eq_guard(heap: &Heap, ctx: &Ctx, a: Value, b: Value) -> Option<(Symbol, Ty)> {
    let Value::Keyword(keyword) = b else {
        return None;
    };
    let call = list_items(heap, a)?;
    let [Value::Sym(head), Value::Sym(sym)] = call[..] else {
        return None;
    };
    if !value::symbol_is(head, "type-of") || ctx.is_lexical_local(head) || ctx.is_file_global(head)
    {
        return None;
    }
    Ty::of_type_keyword(keyword).map(|ty| (sym, ty))
}

/// If `a` is a symbol and `b` is a self-evaluating literal, return the guard
/// `(a, type-of(b))`. Used by `guard_assertion`'s `%eq` arm to recognise both
/// `(%eq sym lit)` and `(%eq lit sym)`. Returns `None` when `b` is itself a
/// variable — equality between two unknowns asserts nothing.
fn literal_eq_guard(heap: &Heap, a: Value, b: Value) -> Option<(Symbol, Ty)> {
    let Value::Sym(s) = a else { return None };
    // A literal is anything that's not a symbol / pair / vector / map.
    // Strings, ints, floats, keywords, booleans, nil all self-evaluate and
    // have a definite tag; pairs/vectors/maps are constructions whose pieces
    // could be unknown.
    match b {
        Value::Sym(_) | Value::Pair(_) | Value::Vector(_) | Value::Map(_) => None,
        // A STRING literal is read through the heap. `Ty::of_value` is heap-free by
        // design, so it can only answer the flat `string` tag — which made `(= m "x")`
        // narrow `m` to `string` where `(= n 1)` narrows `n` to `1`, and left the else
        // branch one-sided into the bargain (the `exact` test above already asks for
        // `as_lit_str`). Int and keyword literals have refined here since ADR-120; this
        // is strings catching up, and it is what lets `(if (= expr "1") (string/->number
        // expr) 0)` know its parse cannot fail.
        Value::Str(id) => Some((s, Ty::str_lit(&heap.string(id)))),
        other => Some((s, Ty::of_value(other))),
    }
}

/// Match-exhaustiveness check over literal-enum scrutinees (ADR-118).
///
/// `match` compiles `(match expr clause…)` to a `let`+`if`+`%eq` chain whose
/// innermost failure is `(throw [:match-error 'context target 'patterns])`
/// (`%match-no-match`, `std/prelude.blsp`) — and that exact shape is only
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
/// every member, [`match_coverage`] answers `Missing` with a message naming them.
///
/// Conservative by construction: a non-literal pattern among those tried
/// (a destructuring pattern, a guarded bind) answers `Unknown` rather than
/// half-reasoning about coverage; a scrutinee whose type isn't a pure
/// literal-enum does too. Never a false positive, may miss a real gap.
pub(super) enum MatchCoverage {
    /// Every value the scrutinee's type admits is tried by some clause.
    Covered,
    /// The scrutinee is an enumerable literal type and these members are not tried.
    Missing(String),
    /// Not decidable here: the scrutinee's type is not a closed literal set, or a pattern
    /// is not a literal.
    Unknown,
}

/// The coverage of the `match` a `(throw [:match-error …])` shape belongs to — `None` when
/// `throw_arg` is not that shape at all (an ordinary throw). The walk reports the `Missing`
/// case (ADR-118); a `:total` declaration (ADR-351) demands `Covered`.
pub(super) fn match_coverage(heap: &Heap, throw_arg: Value, ctx: &Ctx) -> Option<MatchCoverage> {
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
        return Some(MatchCoverage::Unknown);
    };
    let Some(target_ty) = expr_ty(heap, Value::Sym(target), ctx) else {
        return Some(MatchCoverage::Unknown);
    };
    // Name the *surface* form in the diagnostic. `match`/`case`/refutable `let`
    // all lower to the same `match*` failure, so the embedded context keyword is
    // the only thing that distinguishes them — without it a `case` was reported
    // as "match: not exhaustive", naming a form the author never wrote.
    // (The context arrives as the *unevaluated* `(quote :match)` / `(quote :case)`
    // form, like the pattern list below — unwrap it, and fall back to "match" for
    // a non-keyword context, e.g. a `fn` clause's fn-name context.)
    let surface = list_items(heap, elems[1])
        .filter(|q| q.len() == 2 && matches!(q[0], Value::Sym(s) if value::symbol_is(s, "quote")))
        .and_then(|q| match q[1] {
            Value::Keyword(k) => Some(value::symbol_name_ref(k).to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "match".to_string());

    // Unwrap `(quote patterns-list)` to the raw pattern list.
    let Some(quote_items) = list_items(heap, elems[3]) else {
        return Some(MatchCoverage::Unknown);
    };
    if quote_items.len() != 2 {
        return Some(MatchCoverage::Unknown);
    }
    let Value::Sym(q) = quote_items[0] else {
        return Some(MatchCoverage::Unknown);
    };
    if !value::symbol_is(q, "quote") {
        return Some(MatchCoverage::Unknown);
    }
    let Some(patterns) = list_items(heap, quote_items[1]) else {
        return Some(MatchCoverage::Unknown);
    };

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
        return Some(MatchCoverage::Unknown);
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
        let Some(members) = target_ty.as_lit() else {
            return Some(MatchCoverage::Unknown);
        };
        for &s in members {
            declared.insert(format!(":{}", value::symbol_name_ref(s)));
        }
    }
    if target_ty.contains_tag(Tag::Int) {
        let Some(members) = target_ty.as_lit_int() else {
            return Some(MatchCoverage::Unknown);
        };
        for &n in members {
            declared.insert(n.to_string());
        }
    }
    if target_ty.contains_tag(Tag::Bool) {
        let Some(members) = target_ty.as_lit_bool() else {
            return Some(MatchCoverage::Unknown);
        };
        for &b in members {
            declared.insert(b.to_string());
        }
    }
    if target_ty.contains_tag(Tag::Str) {
        let Some(members) = target_ty.as_lit_str() else {
            return Some(MatchCoverage::Unknown);
        };
        for s in members {
            declared.insert(format!("{s:?}"));
        }
    }

    // Render every tried pattern the same way; any non-literal pattern
    // (destructuring, a guarded bind, a pin) bails — no coverage reasoning
    // attempted for those (sound: misses a real gap rather than guessing).
    let mut tested: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for &p in &patterns {
        let Some(label) = render_literal_pattern(heap, p) else {
            return Some(MatchCoverage::Unknown);
        };
        tested.insert(label);
    }

    let mut missing: Vec<&String> = declared.difference(&tested).collect();
    if missing.is_empty() {
        return Some(MatchCoverage::Covered);
    }
    missing.sort();
    let joined = missing
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(MatchCoverage::Missing(format!(
        "{surface}: not exhaustive — missing {joined}"
    )))
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
/// `%eq`-guarded `if` (a catch-all body, a `%match-no-match` throw, or a
/// divergent hand-written `if`) — nothing more to reason about.
pub(super) fn find_redundant_clause(
    heap: &Heap,
    form: Value,
    sym: Symbol,
    lit: Value,
) -> Option<Value> {
    // A loop down the `else` chain, not recursion: a generated `match` (a codepoint
    // table) lowers to a cascade as long as it has clauses.
    let mut form = form;
    loop {
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
        form = items[3];
    }
}

// ---- comparison facts: intervals, lengths and index bounds (ADR-350) ------------------

/// One side of a comparison the interval rules can read: a local (narrowed by its int
/// interval), the count of a local (`(count xs)` / `(string/length xs)` — narrowed by its
/// length), or a literal int.
enum CmpSide {
    Local(Symbol),
    /// `(+ i k)` / `(inc i)` — a local plus a literal offset (C12). Its value is `i + k`,
    /// so its interval is the local's shifted by `k` and narrowing it narrows the local by
    /// `r − k`. The shape a scan's index is written in.
    Offset(Symbol, i64),
    Count(Symbol),
    Lit(i64),
}

/// The facts a comparison establishes on each branch: each is `(symbol, type)` to narrow
/// by (the type is `int[…] ∪ ¬int` for a local, a length refinement for a collection), and
/// the index bounds `(i, xs, k)` — `i + k < (count xs)` — the branch establishes.
#[derive(Default)]
pub(super) struct CmpFacts {
    pub(super) then_narrow: Vec<(Symbol, Ty)>,
    pub(super) else_narrow: Vec<(Symbol, Ty)>,
    pub(super) then_index: Vec<(Symbol, Symbol, i64)>,
    pub(super) else_index: Vec<(Symbol, Symbol, i64)>,
}

impl CmpFacts {
    fn swapped(self) -> CmpFacts {
        CmpFacts {
            then_narrow: self.else_narrow,
            else_narrow: self.then_narrow,
            then_index: self.else_index,
            else_index: self.then_index,
        }
    }
    fn extend_then(&mut self, other: CmpFacts) {
        self.then_narrow.extend(other.then_narrow);
        self.then_index.extend(other.then_index);
    }
    fn extend_else(&mut self, other: CmpFacts) {
        self.else_narrow.extend(other.else_narrow);
        self.else_index.extend(other.else_index);
    }
    fn is_empty(&self) -> bool {
        self.then_narrow.is_empty()
            && self.else_narrow.is_empty()
            && self.then_index.is_empty()
            && self.else_index.is_empty()
    }
}

/// `int` within `r`, or anything that is not an int at all — what a comparison proves of
/// a local: nothing of a float or a comparable record, and the interval of an int.
fn int_guard_ty(r: Range) -> Ty {
    Ty::ANY.difference(Ty::of(Tag::Int)).union(Ty::int_in(r))
}

/// Every countable value of length within `r` — and `nil` only when `r` admits 0, since
/// the empty list counts 0 and nothing else does.
fn len_guard_ty(r: Range) -> Ty {
    let base = if Range::subset(Range::point(0), r) {
        Ty::ANY
    } else {
        Ty::ANY.difference(Ty::of(Tag::Nil))
    };
    base.with_len(r)
}

/// `(+ i k)` / `(inc i)` — a local plus a literal offset, the index expression a scan
/// writes (`(nth s (+ i 1))`). `(i, k)`; a bare local is `(i, 0)`. Only the shapes the
/// corpora actually contain: `std/json.blsp`, `std/ansi.blsp` and `std/url.blsp` between
/// them write `(+ i 1)`, `(+ i 2)`, `(+ i 4)`, `(+ i 5)` and `(inc i)` (C12).
pub(super) fn local_plus_offset(heap: &Heap, form: Value, ctx: &Ctx) -> Option<(Symbol, i64)> {
    match form {
        Value::Sym(s) if ctx.is_lexical_local(s) => Some((s, 0)),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            let Some(&Value::Sym(head)) = items.first() else {
                return None;
            };
            if ctx.is_lexical_local(head) {
                return None; // a local shadowing `+`/`inc` is not this form
            }
            match items[1..] {
                [Value::Sym(i)] if value::symbol_is(head, "inc") && ctx.is_lexical_local(i) => {
                    Some((i, 1))
                }
                [Value::Sym(i), Value::Int(k)] | [Value::Int(k), Value::Sym(i)]
                    if value::symbol_is(head, "+") && ctx.is_lexical_local(i) =>
                {
                    // A NEGATIVE offset is refused: the rule needs `i + b ≥ 0`, which the
                    // index's own interval decides, and `i - 1` under `i ≥ 0` is not it.
                    (k >= 0).then_some((i, k))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn cmp_side(heap: &Heap, form: Value, ctx: &Ctx) -> Option<CmpSide> {
    match form {
        Value::Int(n) => Some(CmpSide::Lit(n)),
        Value::Sym(s) if ctx.is_lexical_local(s) => Some(CmpSide::Local(s)),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            let [Value::Sym(head), Value::Sym(target)] = items[..] else {
                // Not a one-argument form: an offset index (`(+ i 1)`) is the other shape
                // this reads. Before C12 a guard over one produced NO facts at all — the
                // `?` here discarded the whole comparison — so `(and (< (+ i 1) n) …)`
                // narrowed nothing and bounded nothing.
                return local_plus_offset(heap, form, ctx).map(|(i, k)| CmpSide::Offset(i, k));
            };
            let counts = value::symbol_is(head, "count")
                || value::symbol_is(head, "string/length")
                || value::symbol_is(head, "vector-length");
            if counts && !ctx.is_lexical_local(head) && ctx.is_lexical_local(target) {
                return Some(CmpSide::Count(target));
            }
            local_plus_offset(heap, form, ctx).map(|(i, k)| CmpSide::Offset(i, k))
        }
        _ => None,
    }
}

/// The interval a side's value lies in — a local's int interval (every int, when its
/// type is not known), a count's length interval, a literal's point.
fn side_range(side: &CmpSide, ctx: &Ctx) -> Range {
    match side {
        CmpSide::Lit(n) => Range::point(*n),
        CmpSide::Local(s) => ctx
            .get(*s)
            .and_then(|t| t.int_range())
            .unwrap_or(Range::ALL),
        CmpSide::Count(xs) => ctx
            .get(*xs)
            .and_then(|t| t.count_range())
            .unwrap_or(Range::at_least(0)),
        // `i + k` lies in `i`'s interval shifted by `k`.
        CmpSide::Offset(i, k) => Range::plus(
            ctx.get(*i)
                .and_then(|t| t.int_range())
                .unwrap_or(Range::ALL),
            Range::point(*k),
        ),
    }
}

/// The narrowing that puts a side's value within `r`: a local's int member, a count's
/// collection's length. A literal narrows nothing.
fn side_narrowing(side: &CmpSide, r: Range) -> Option<(Symbol, Ty)> {
    match side {
        CmpSide::Lit(_) => None,
        CmpSide::Local(s) => Some((*s, int_guard_ty(r))),
        CmpSide::Count(xs) => Some((*xs, len_guard_ty(r))),
        // Bounding `i + k` by `r` bounds `i` by `r − k`.
        CmpSide::Offset(i, k) => Some((*i, int_guard_ty(Range::minus(r, Range::point(*k))))),
    }
}

/// The collection `rhs` counts, when a `let` binding it makes its name a COUNT ALIAS
/// (ADR-350): `(let (n (count xs)) …)` means `n` is `xs`'s length for the scope, so a guard
/// on `n` narrows `xs`'s length ([`Ctx::narrow`]) and bounds `n`'s comparands as indices of
/// it ([`counted_collection`]).
///
/// Shared because a `let` is bound in THREE places — the walk (`binders::let_bind_scope`),
/// inference (`infer::expr_ty`) and the return check (`calls::gradual_of_compound`) — and
/// only the first recorded this, so the fact reached an argument check and not a return
/// one: `(let (n (count words)) (if (>= n 4) (nth words 3) ""))` declared `string` warned
/// `nil | string`, while the identical read passed into a `(string -> int)` was clean
/// (2026-09-17, C12). One function, called from all three, so they cannot drift again.
pub(super) fn count_alias_target(heap: &Heap, rhs: Value, ctx: &Ctx) -> Option<Symbol> {
    let items = list_items(heap, rhs)?;
    let [Value::Sym(head), Value::Sym(target)] = items[..] else {
        return None;
    };
    let counts = value::symbol_is(head, "count")
        || value::symbol_is(head, "string/length")
        || value::symbol_is(head, "vector-length");
    (counts && !ctx.is_lexical_local(head) && ctx.is_lexical_local(target)).then_some(target)
}

/// The collection a side is the count of: `(count xs)` itself, or a local `n` a `let`
/// bound to one (`Ctx::count_alias`).
fn counted_collection(side: &CmpSide, ctx: &Ctx) -> Option<Symbol> {
    match side {
        CmpSide::Count(xs) => Some(*xs),
        CmpSide::Local(n) => ctx.count_alias(*n),
        // `(+ n 1)` is a length PLUS something, not a length: nothing is the count of it.
        CmpSide::Offset(_, _) | CmpSide::Lit(_) => None,
    }
}

/// The facts of one comparison `(op L R)` — `<`, `<=`, `>`, `>=` in either spelling, and
/// `=` over a count. Strict and non-strict, each way round:
/// - then-branch of `L < R`: `L ≤ hi(R) − 1`, `R ≥ lo(L) + 1`, and `L` is an index of the
///   collection `R` counts; else-branch: `L ≥ lo(R)`, `R ≤ hi(L)`.
/// - `L <= R`: then `L ≤ hi(R)`, `R ≥ lo(L)`; else `L ≥ lo(R) + 1`, `R ≤ hi(L) − 1`, and `L`
///   is an index of what `R` counts is NOT established (equality is allowed).
/// A `>`/`>=` is the same with the sides swapped.
fn comparison_facts(heap: &Heap, test: Value, ctx: &Ctx) -> Option<CmpFacts> {
    let items = list_items(heap, test)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if ctx.is_lexical_local(head) {
        return None;
    }
    let name = value::symbol_name(head);
    let (swap, strict, equality) = match name.as_str() {
        "<" | "%lt" => (false, true, false),
        "<=" | "%le" => (false, false, false),
        ">" | "%gt" => (true, true, false),
        ">=" | "%ge" => (true, false, false),
        "=" | "%eq" => (false, false, true),
        _ => return None,
    };
    let (a, b) = (
        cmp_side(heap, items[1], ctx)?,
        cmp_side(heap, items[2], ctx)?,
    );
    let (l, r) = if swap { (b, a) } else { (a, b) };
    let mut facts = CmpFacts::default();
    if equality {
        // `(= (count xs) 3)`: the length is exactly the other side's value. (Two locals'
        // ints are the literal guard's business, and its else-branch has no interval.)
        let (rl, rr) = (side_range(&l, ctx), side_range(&r, ctx));
        if let Some(meet) = Range::meet(rl, rr) {
            facts.then_narrow.extend(side_narrowing(&l, meet));
            facts.then_narrow.extend(side_narrowing(&r, meet));
        }
        return (!facts.is_empty()).then_some(facts);
    }
    let (rl, rr) = (side_range(&l, ctx), side_range(&r, ctx));
    let step = i64::from(strict);
    // then: L < R (or ≤)
    if let Some(hi) = rr.hi {
        facts
            .then_narrow
            .extend(side_narrowing(&l, Range::at_most(hi - step)));
    }
    if let Some(lo) = rl.lo {
        facts
            .then_narrow
            .extend(side_narrowing(&r, Range::at_least(lo + step)));
    }
    // else: L ≥ R (or >)
    let back = 1 - step;
    if let Some(lo) = rr.lo {
        facts
            .else_narrow
            .extend(side_narrowing(&l, Range::at_least(lo + back)));
    }
    if let Some(hi) = rl.hi {
        facts
            .else_narrow
            .extend(side_narrowing(&r, Range::at_most(hi - back)));
    }
    // The relational fact: `i + k < (count xs)` — a bound of an index EXPRESSION by a
    // count. `k = 0` is the plain `i < (count xs)` this started as (ADR-350).
    let index_side = |side: &CmpSide| match side {
        CmpSide::Local(i) => Some((*i, 0)),
        CmpSide::Offset(i, k) => Some((*i, *k)),
        _ => None,
    };
    if strict {
        if let (Some((i, k)), Some(xs)) = (index_side(&l), counted_collection(&r, ctx)) {
            facts.then_index.push((i, xs, k));
        }
    } else if let (Some((i, k)), Some(xs)) = (index_side(&l), counted_collection(&r, ctx)) {
        // `i + k ≤ n` gives `i + (k−1) < n`, which is what makes `std/json.blsp`'s
        // `(and (<= (+ i 10) n) … (nth s (+ i 4)) …)` read an element: 4 ≤ 9. At `k = 0`
        // it says nothing — `i ≤ n` is not `i < n` — so nothing is recorded.
        if k >= 1 {
            facts.then_index.push((i, xs, k - 1));
        }
    }
    if !strict {
        if let (Some(xs), Some((i, k))) = (counted_collection(&l, ctx), index_side(&r)) {
            // `(<= (count xs) (i + k))` false ⇒ `i + k < (count xs)`.
            facts.else_index.push((i, xs, k));
        }
    }
    (!facts.is_empty()).then_some(facts)
}

/// The comparison facts of a whole test: a comparison, `(not …)` of one (swapped), every
/// conjunct of an `and`-expansion (then-side only: a falsy `and` proves nothing of its
/// conjuncts), every disjunct of an `or`-expansion (else-side only).
pub(super) fn test_comparison_facts(heap: &Heap, test: Value, ctx: &Ctx) -> CmpFacts {
    // Deep-form stack safety, as `guard_assertion`.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        test_comparison_facts_inner(heap, test, ctx)
    })
}

fn test_comparison_facts_inner(heap: &Heap, test: Value, ctx: &Ctx) -> CmpFacts {
    if let Some(facts) = comparison_facts(heap, test, ctx) {
        return facts;
    }
    let Some(items) = list_items(heap, test) else {
        return CmpFacts::default();
    };
    if let Some(&Value::Sym(head)) = items.first() {
        if items.len() == 2 && value::symbol_is(head, kw::NOT) {
            return test_comparison_facts(heap, items[1], ctx).swapped();
        }
    }
    let mut out = CmpFacts::default();
    let mut cur = test;
    let mut matched = false;
    while let Some((cond, rest)) = chain_shape(heap, cur, true) {
        matched = true;
        out.extend_then(test_comparison_facts(heap, cond, ctx));
        cur = rest;
    }
    if matched {
        out.extend_then(test_comparison_facts(heap, cur, ctx));
        return out;
    }
    let mut cur = test;
    while let Some((cond, rest)) = chain_shape(heap, cur, false) {
        matched = true;
        out.extend_else(test_comparison_facts(heap, cond, ctx));
        cur = rest;
    }
    if matched {
        out.extend_else(test_comparison_facts(heap, cur, ctx));
    }
    out
}

/// Apply a test's comparison facts to the two branch scopes.
pub(super) fn apply_comparison_facts(
    heap: &Heap,
    test: Value,
    ctx: &Ctx,
    mut then_ctx: Ctx,
    mut else_ctx: Ctx,
) -> (Ctx, Ctx) {
    let facts = test_comparison_facts(heap, test, ctx);
    for (sym, ty) in facts.then_narrow {
        then_ctx = then_ctx.narrow(sym, ty);
    }
    for (sym, ty) in facts.else_narrow {
        else_ctx = else_ctx.narrow(sym, ty);
    }
    for (i, xs, k) in facts.then_index {
        then_ctx = then_ctx.add_index_bound(i, xs, k);
    }
    for (i, xs, k) in facts.else_index {
        else_ctx = else_ctx.add_index_bound(i, xs, k);
    }
    (then_ctx, else_ctx)
}
