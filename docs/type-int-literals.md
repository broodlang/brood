# Int-literal types — `5` as a type (ADR-117)

> Status: **shipped** — the first slice of ADR-105's deferred item ("bool/int/
> string literals are the same machinery... a deferred follow-on"). A bare
> int like `5` in a `(sig …)` type position is now a literal singleton type,
> exactly like the already-shipped keyword-literal type (`:ok`). Bool and
> string literals followed as the same pattern again (ADR-120,
> [type-bool-string-literals.md](type-bool-string-literals.md)) — that
> deferral is now closed. Call-site argument literal precision (matching a
> literal int *argument* against a declared literal set) is still deferred —
> see [Deferred](#deferred) — this slice covers declared-sig literal sets only.

## Problem

ADR-105 shipped keyword-literal types (`:ok` in type position means "exactly
that keyword") with a one-line deferral: "bool/int/string literals are the
same machinery... a deferred follow-on." Taken at face value that sounds like
a small mechanical extension. It isn't, for two concrete reasons:

1. **`Value` has no `Ord`/`Eq`/`Hash`** (only `Clone`/`Copy`/`Debug`), and
   `Value::Float(f64)` structurally can't get them (NaN) — so a generic
   `BTreeSet<Value>` literal set is impossible. Each literal kind needs its
   own concretely-typed storage.
2. **The existing `lit: Option<Arc<BTreeSet<Symbol>>>` is hardwired to one
   tag** (`KEYWORD_BIT`) at every one of its ~6 call sites (`union`,
   `intersect`, `negate`, `is_subtype`, `is_disjoint`, `Display`). Supporting
   `(or :ok 5)` — a keyword literal and an int literal at once — isn't free
   with a single field.

## Why this generalizes cleanly anyway

Point 2 resolves via a pattern this repo already established twice:
`arrow`/`overload` are two **independent** fields both tagged `FN_BITS`
(ADR-116), and `map_kv`/`fields` are two independent fields both tagged
`MAP_BIT` (ADR-115). Adding a **third independent field**, `lit_int` tagged
`INT_BIT`, follows the same precedent — since it's tied to a *different* tag
bit than `lit`'s `KEYWORD_BIT`, a `Ty` can carry both simultaneously with
zero special-casing. `(or :ok 5)` just ends up with `lit: Some({:ok})` *and*
`lit_int: Some({5})` — no "which kind won" logic needed, and no change to how
`lit` itself works.

## Representation (`types/mod.rs`)

```rust
const INT_BIT: u32 = 1u32 << bit(Tag::Int);
// ...
lit_int: Option<Arc<BTreeSet<i64>>>,
```

`Ty::int_lit(n: i64) -> Ty` (mirrors `keyword_lit`) and `Ty::as_lit_int(&self)
-> Option<&BTreeSet<i64>>` (mirrors `as_lit`). Every one of the ~6 call sites
`lit`/`KEYWORD_BIT` touches got a parallel `lit_int`/`INT_BIT` block, same
shape, new tag — `union` (a new `merge_union_lit_int`, exact set-union, open
side widens), `intersect` (same `BTreeSet::intersection` + empty-clears-
the-bit logic), `negate` (one line: keep `INT_BIT` if `lit_int.is_some()`),
`is_subtype` (`is_subset`, mirrored), `is_disjoint` (a second precise
exception: `shared == INT_BIT` alongside the existing `shared == KEYWORD_BIT`
one), and `Display` (rendered together with any keyword literals present,
since both can coexist — `:ok | 5`, sorted keywords first then numerically
sorted ints, then any other open tags).

## Grammar (`check/annot.rs::parse_type`)

One new match arm, right after the keyword one:

```rust
Value::Int(n) => Some(Ty::int_lit(n)),
```

No ambiguity to worry about (unlike keywords vs. base-type symbols) — an int
literal can't collide with any symbol-spelled base type name. `BigInt` values
(outside `i64` range) aren't handled: `parse_type` has no `Value::BigInt`
arm, so a huge integer literal in type position falls through to the
implicit `_ => None` (dropped, not guessed — consistent with the
"unrecognised → drop" rule everywhere else in this function).

`parse_type_term` (the type-variable-aware path used by `SigWithVars`) needed
no change — it already delegates any head it doesn't special-case (which
includes a bare `Value::Int`, since that's not even a `Value::Pair`) straight
to `parse_type`.

## Runtime (`std/prelude.blsp`'s `type-matches?`)

One new branch, next to the keyword one:

```lisp
(int? t) (= t v)                   ; an int type-expr is a literal too (ADR-117)
```

Before this, a bare int in type position fell all the way to the `else true`
catch-all — silently accepted, never enforced. `sig!`/`BROOD_CONTRACTS=1` now
actually check it.

## Deferred

- **Bool and string literals** — shipped, see ADR-120
  ([type-bool-string-literals.md](type-bool-string-literals.md)). The open
  bool design question this doc originally left unresolved (whether `false`
  stays a legitimate singleton once bool literals are a real `Ty` kind, or
  the keyword-era "use `nil` instead" guidance carries forward) was
  resolved there: `false` is now a legitimate literal type — that guidance
  was scoped to avoiding `false`/`nil` confusion in an *enumerated keyword*
  set specifically, not a technical restriction.
- **`BigInt` literals** — out of `i64` range; would need `lit_int`'s storage
  widened to a `BigInt`-aware representation. Not needed for the common case.
- **Call-site argument literal precision — tried, reverted, explicitly
  deferred.** Keywords get more than declared-sig precision: `Ty::of_value`
  (the bridge from a runtime value to its static type) turns a *literal
  keyword appearing in code* into its singleton type too, so
  `(c-mode :bogus)` against a declared `(or :maximized :fullboth :fullscreen
  nil)` is a provable disjointness the **static checker** catches, not just
  the runtime contract. Extending `of_value` to do the same for `Value::Int`
  was tried — and reverted — because it's a materially bigger, riskier change
  than it looks: `of_value` feeds *every* literal int expression's inferred
  type throughout the checker, not just call arguments, so making every
  int literal a singleton changed the *rendered text* of unrelated
  misuse-warning messages project-wide (e.g. `"got int"` → `"got 5"`),
  breaking 7 pre-existing, unrelated tests on exact wording
  (`eq_against_a_literal_is_a_guard`, `let_binding_propagates_its_rhs_type`,
  `match_literal_pattern_narrows_the_scrutinee`, and four others in
  `types/check.rs`). This slice's scope is **declared-sig literal sets only**
  — a function's declared return type or parameter type can be an int-literal
  set and that flows to callers correctly (verified,
  `int_literal_return_type_flows_through_checker`), but a literal int
  *argument* at a call site isn't itself recognized as a singleton the way a
  literal keyword argument already is. Picking this up needs a real design
  pass on where else `of_value`'s result is consumed and whether the warning-
  message wording changes are acceptable (likely yes, they're arguably *more*
  correct) or need their own handling.

## Soundness

Every new algebra piece has a targeted unit test in
`crates/lisp/src/types/mod.rs`, mirroring the keyword-literal tests exactly:
`int_literal_renders_as_its_value`, `int_literal_union_is_exact_but_open_int_widens`,
`int_literal_subtyping`, `int_literal_disjointness_is_precise`,
`int_literal_intersection`, plus `keyword_and_int_literals_coexist_on_one_ty`
(the `(or :ok 5)` case). A checker-level test in `crates/lisp/src/types/check.rs`
(`int_literal_return_type_flows_through_checker`) proves a declared int-literal-set
return type flows through `sig_of`/`expr_ty` to a call site correctly.

Verified against the whole `std/`+`tests/` corpus the same way records/arrows
were: `nest check` with the new `Value::Int(n)` parse arm disabled vs. enabled
— byte-identical, zero new warnings (int literals aren't used in any
annotation in the corpus today).

## Tests

`crates/lisp/src/types/mod.rs`: the 6 unit tests listed above, under "int-literal
(singleton) types — ADR-117", right after the keyword-literal tests.
`crates/lisp/src/types/check.rs`: `int_literal_return_type_flows_through_checker`.
`tests/contract_test.blsp`: `describe "int-literal type contracts"` — enumerated
value passes, an int outside the set throws, a non-int throws, a single literal
matches only that exact value (mirrors the keyword-literal contract block).
