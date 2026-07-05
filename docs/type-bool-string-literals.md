# Bool and string literal types (ADR-120)

> Status: **shipped**. `true`/`false`/`"GET"` in a `(sig …)` type position are
> now literal singleton types, exactly like the already-shipped keyword
> (ADR-105) and int (ADR-117) literals. Closes ADR-105's deferred item in
> full — keyword, int, bool, and string literal types are all shipped now.

## Design

Mirrors ADR-117's `lit_int` pattern twice more in `crates/lisp/src/types/mod.rs`:

- `const BOOL_BIT`/`const STR_BIT`, `lit_bool: Option<Arc<BTreeSet<bool>>>`,
  `lit_str: Option<Arc<BTreeSet<String>>>` — independent fields/tags, so any
  combination of keyword/int/bool/string/nil composes on one `Ty` with zero
  special-casing (as already exercised by exhaustiveness — see
  [`type-match-exhaustiveness.md`](type-match-exhaustiveness.md)).
- `bool` is natively `Ord`/`Eq`/`Hash`/`Copy` — a straight copy of the int
  pattern across all ~6 call sites (`union`/`merge_union_lit_bool`,
  `intersect`, `negate`, `is_subtype`, `is_disjoint`, `Display`).
- `string` has one real wrinkle: `Value::Str` is a heap handle (`StrId`), not
  inline data — two textually identical string literals can have different
  underlying ids, so storing `StrId` in the set would break equality.
  `lit_str` stores the actual `String` content instead (read out via
  `heap.string(id)`); `Ty::str_lit(s: &str)` takes the string slice, not a
  `Value`/`Heap` pair, so `Ty` itself stays heap-independent like every other
  constructor.
- Grammar (`check/annot.rs::parse_type`): `Value::Bool(b) => Some(Ty::bool_lit(b))`,
  `Value::Str(id) => Some(Ty::str_lit(heap.string(id)))`.
- Runtime (`type-matches?`): `(bool? t) (= t v)`, `(string? t) (= t v)`.

## `false` is now a legitimate literal type

ADR-105 (the keyword-literal era) noted "`false` is not a literal type — use
`nil` for an off arm." That restriction was scoped to avoiding `false`/`nil`
confusion in an *enumerated keyword* set specifically — it was never a
technical parsing limitation (booleans and keywords are different `Value`
variants; there was no ambiguity to resolve). Now that bool-literal types are
their own real `Ty` kind, `false` (and `true`) are legitimate singletons like
any other literal — `(sig f (false -> any))` means exactly what it looks
like.

## Deferred

Same boundary ADR-117 already settled: **no revisit of the `of_value`
call-site-argument question.** A literal keyword *argument* at a call site
gets static disjointness checking (via `Ty::of_value`); int/bool/string
literals don't, because extending `of_value` for int was tried and reverted
(cascaded into unrelated warning-message wording across 7 pre-existing
tests). Bool/string literals stay declared-sig-only, the same boundary int
literals landed at.

## Tests

`crates/lisp/src/types/mod.rs`: `bool_literal_*`/`str_literal_*` — render,
union-exact-but-widens, subtyping, disjointness, intersection — mirroring
every `int_literal_*` test exactly. `tests/contract_test.blsp`:
`describe "bool/string-literal type contracts"` — exact match passes, the
other value throws, a wrong-tagged value throws, an enumerated string set,
a value outside the set throws.
