# Tuple / positional product types — `(tuple T1 T2 …)` in the type grammar

> Status: **shipped** (ADR-128). `Ty` carries a `tuple` refinement (fixed
> arity, per-position types) alongside the existing `elem` (uniform sequence
> element) and `fields` (record) refinements; a `[ ]` vector *literal* infers
> its exact positional shape instead of widening to a uniform element type;
> `(get)`-style positional sinks (`first`/`second`/`third`/`last`/`nth` with a
> literal index) resolve to the exact per-position type; `sig!`/
> `BROOD_CONTRACTS=1` enforce it at the runtime-contract boundary.

## Problem

Brood previously had no way to say "a vector of exactly these positions, in
this order" — `(vector E)` only expresses a *uniform* element type, so a
2-element `[name, age]`-shaped return value could only be typed as the flat
`(vector any)` or (with luck) a widened union like `(vector (or string
int))`, losing the fact that position 0 is always a `string` and position 1
is always an `int`.

```lisp
(sig parse-header ((string) -> (tuple string int)))
(defn parse-header (line) (let (parts (string-split line ":")) [(first parts) (string->int (second parts))]))
```

`(tuple …)` fixes this the same way `(record …)` (ADR-115) fixed the
equivalent problem for maps: a **refinement** on the existing runtime value
(a tuple is still a plain `[ ]` vector — no new `Value` kind), not a new
kind of data.

## Grammar extension

```
type ::= …
       | (tuple type*)
```

`(tuple int string)` = "a 2-element vector whose position 0 is an `int` and
position 1 is a `string`". `(tuple)` (zero elements) is a legitimate empty
tuple — no minimum-length requirement, unlike `(list E)`/`(vector E)` (which
take exactly one element type each; every item after `tuple` is its own
position's type, so the arity varies naturally with how many are given).

## Design: a refinement, not a new tag

`Ty` gained a fifth structural refinement field, `tuple: Option<Arc<Vec<Ty>>>`,
tagged to the `Vector` runtime tag alone (not `pair` too — ADR-003 already
keeps vectors and cons-list `pair`s separate, and a tuple's positions only
make sense for a `[ ]` literal's fixed structure). It follows the exact
layering pattern `fields` (record) already established onto `Map`:
mutually exclusive with `elem` in practice (a `Ty` is built by either
`vector_of` or `tuple_of`), but independent fields so the generic
union/intersect machinery treats every refinement pair alike.

**Subtyping** has three shapes to get right, all handled in `Ty::is_subtype`:

- **tuple ⊆ tuple**: exact arity match (unlike a record's open width
  subtyping, a tuple's arity *is* its shape — a 2-tuple isn't a subtype of a
  3-tuple in either direction), then covariant per position.
- **tuple ⊆ uniform vector**: `tuple<int, string>` *is* a `vector<int |
  string>` — every element the tuple could ever produce is one of those
  types. `Ty::elem_ty()` (the single choke point every `first`/`nth`/
  `is_subtype` consumer already reads) derives this union on the fly when a
  type has `tuple` but no plain `elem`, so this composes for free everywhere
  `elem_ty()` is already consulted — no separate tuple-awareness needed at
  most call sites.
- **uniform vector ⊄ tuple** (the rejected direction): a plain `vector<int>`
  can't prove it has exactly N elements of specific per-position types, so
  `self.tuple = None` correctly fails a subtype check against a specific
  tuple shape.

**Disjointness** (`Ty::is_disjoint`, the predicate the "argument N expects X,
got Y" family of warnings actually uses — not `is_subtype`) gets a genuinely
sound tuple-vs-tuple case, the same shape as the existing keyword/int/bool/
string literal-set special cases: two tuples are provably disjoint if their
arities differ (a vector value has exactly one length, so it can never be
both a 2-tuple and a 3-tuple) or if any single position's types are disjoint
(a value satisfying both shapes would need every position to satisfy both at
once). This only ever *adds* a genuinely-disjoint verdict — advisory-
soundness holds, same as every other refinement here.

## Literal inference: positional, not widened

A vector *literal* `[a b c]` now infers `Ty::tuple_of([type(a), type(b),
type(c)])` — its exact per-position types — instead of widening to a uniform
`Ty::vector_of(union)` the way it used to. This is a real behavior change to
existing inference, verified safe two ways before shipping: it's **strictly
more precise, never less sound** (a tuple is already a subtype of the
corresponding uniform vector via the `elem_ty()` fallback above, so anything
that type-checked under the old widened inference still does), and a full
`nest check` corpus diff across `std/` + `tests/` came back byte-identical
(91 warnings before and after) — no new warnings anywhere in the existing
codebase from tightening this. Any unknown element still widens the whole
literal to unrefined `vector` (the same all-or-nothing strictness the old
union-based inference already had).

## Positional sinks: `first`/`second`/`third`/`last`/`nth`

Each of these resolves to the tuple's *exact* position when the index is
statically known, rather than the coarse union every other element access
(`filter`, `rest`, `reverse`, …) still returns: `first` = position 0,
`second` = 1, `third` = 2, `last` = the final position, `nth` reads its own
literal-int index argument (a non-literal index falls through to the union
case — it can't be resolved this precisely). An in-range access on a
well-typed tuple is never `nil` (the arity is fixed and known), so the result
is the exact type with no `nil` union; a provably out-of-range literal index
resolves to exactly `nil` (matching the runtime, which returns `nil` rather
than erroring).

## Runtime contract (`sig!` / `BROOD_CONTRACTS=1`)

`type-matches?` (`std/prelude/core.blsp`) gained a `tuple` case alongside `record`:
checks the value is a vector, checks the arity matches exactly, then checks
each position against its declared type. See `tests/contract_test.blsp`'s
"tuple type contracts" section.

## Deferred

- **Nested generics inside a tuple position** (`(tuple ?A ?B)` unifying
  against a call's actual argument types) — the type-variable route
  (`SigWithVars`/`SigTerm`, type-variables.md) doesn't yet have a tuple case;
  only the non-variable `parse_type` path does. Gated on a real consumer
  (ADR-011).
- **`assoc`/positional-update sinks** — Brood vectors are immutable, so
  there's no in-place tuple mutation to type; a hypothetical
  `(tuple-with t i v)`-style constructor isn't modeled.
- **Nested tuples inferred through deeper literal contexts** (a tuple as a
  record field value, or inside another tuple) work today because inference
  is fully recursive (`expr_ty` calls itself on each position/field), but
  weren't a specific focus of the verification above beyond what the
  corpus diff already covers.
