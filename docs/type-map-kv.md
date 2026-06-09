# Map key/value types — `(map KeyType ValType)` in the type grammar

> Status: **slice 1 shipped** (Brood runtime), **slice 2 shipped** (checker
> flat-accept). `type-matches?` walks `entries` to verify each key/value pair;
> `parse_type` parses `(map K V)` and produces `Ty::of(Tag::Map)`. Slice 3 (full
> `map_kv` refinement in `Ty`) deferred until a real consumer drives it.

## Problem

After ADR-078, `(list int)` and `(vector string)` carry element types that flow
through `first`/`nth`/`map`/`filter`. Maps are still flat — a `map?` check is the
best `type-matches?` can do:

```lisp
(sig! lookup ((map keyword int) -> int))
(lookup {:a "oops"})  ; passes the contract — map? is true
                      ; but the value is a string, not int
```

The goal is a `(map K V)` type that validates key and value types at the
runtime-contract boundary and — eventually — informs the checker about what
`get`/`assoc`/`keys`/`vals` return.

## Grammar extension

```
type ::= …
       | (map key-type val-type)
```

`(map keyword int)` = "a map whose keys are all keywords and whose values are
all ints". `(map any any)` is equivalent to bare `map`. A one-arg `(map K)` is
deliberately not supported — if you only care about keys, use `(map K any)`.

## Representation

### Runtime (`type-matches?`) — Brood only, no Rust

The check walks `(entries m)` and verifies each `[k v]` pair:

```lisp
;; in the (pair? t) / (cond ...) branch of type-matches?:
(%eq h 'map)
  (and (map? v)
       (let (ktype (first (rest t))
             vtype (first (rest (rest t))))
         (or (and (nil? ktype) (nil? vtype))
             (every? (fn (kv)
                       (and (type-matches? ktype (first kv))
                            (type-matches? vtype (second kv))))
                     (entries v)))))
```

`entries` (alias for `map-pairs`) returns `[[k v] …]`; it's available at runtime
when `type-matches?` is called. `second` = `(fn (p) (first (rest p)))` or can be
inlined. This is the entire runtime change, pure Brood.

### Static checker (`annot.rs` + `types/mod.rs`)

Two slices with different cost:

**Slice 1 (no `Ty` change):** Treat `(map K V)` in `parse_type` as `Ty::of(Tag::Map)`:

```rust
if value::symbol_is(head, "map") && items.len() == 3 {
    // Parse K and V to validate the annotation, but produce a flat Ty::Map
    // for the checker (no refinement yet). Unknown K/V → drop, warn.
    parse_type(heap, items[1])?;
    parse_type(heap, items[2])?;
    return Some(Ty::of(Tag::Map));
}
```

This is enough for the checker to accept `(sig f ((map keyword int) -> …))`
without a parse error, and the flat `Ty::Map` is still informative for arity /
non-disjointness checks. Cost: near-zero Rust.

**Slice 2 (full refinement):** Add a `map_kv: Option<Arc<(Ty, Ty)>>` field to
`Ty` (alongside the existing `arrow` and `elem` refinements):

```rust
pub struct Ty {
    pub tags: u32,
    pub arrow: Option<Arc<Sig>>,
    pub elem: Option<Arc<Ty>>,
    pub map_kv: Option<Arc<(Ty, Ty)>>,  // new
}
```

Then:
- `parse_type` produces `Ty::map_of(k, v)`.
- `Ty::map_of` is a new constructor setting `tags = Map`, `map_kv = Some(…)`.
- `Ty::union`/`intersect` widen/narrow the `map_kv` refinement (if both sides
  have a refinement, union widens; intersect narrows; one-sided → drop to `None`).
- `seq_aware_call_ty` in `guards.rs` derives return types for `get` (returns `V |
  nil`), `keys` (returns `nil | list<K>`), `vals` (returns `nil | list<V>`), and
  `assoc` (returns `map<K V>` preserving the input's type).

Slice 2 is a meaningful Rust change. Ship slice 1 first; slice 2 only when a
real consumer makes it worthwhile (e.g. the LSP needs hover types for map fields,
or a user project that uses map-heavy code wants precise `get` results).

## Compatibility with the existing `map` base-type name

The bare symbol `map` in a type expression is already handled in `base_ty` →
`Ty::of(Tag::Map)`. The new `(map K V)` is a *compound* form (detected in the
`Pair` branch), so there's no parse conflict. The checker's existing curated sigs
use `Ty::of(Tag::Map)` and keep working unchanged.

## Soundness

- **Runtime:** walking `entries` on every call is O(n) in the map size. For
  most annotation-checked maps this is acceptable; if it's a hot path, prefer
  `sig` (static-only) over `sig!`. Same trade-off as `(list int)` element checks.
- **Checker (slice 1):** widening to flat `Ty::Map` is always sound — disjointness
  can never false-positive on a wider type.
- **Checker (slice 2):** `map_kv` refinement follows the same rules as `elem`:
  `is_disjoint` ignores it (tags-only), so a wrong map refinement can only *miss*
  a warning, never generate a spurious one. Sound by the "uncertain → flat fallback"
  rule.

## Tests to add

In `contract_test.blsp`:

```lisp
(defn c-lookup (m k) (get m k 0))
(sig! c-lookup ((map keyword int) keyword -> int))

;; correct
(assert= (c-lookup {:a 1 :b 2} :a) 1)
;; wrong key type
(assert-error (c-lookup {"a" 1} :a))
;; wrong value type
(assert-error (c-lookup {:a "one"} :a))
;; non-map argument
(assert-error (c-lookup (list 1 2) :a))
```

Static (slice 2) checker test: `(get (map-of-keyword-int) :k) : int | nil`
doesn't warn against a `number` sink; `(string-length (get m k))` warns when
`m : (map keyword int)`.

## Slicing order

1. **Brood runtime** (`prelude.blsp`, ~10 lines). Pure Brood, ships alone.
   Update grammar in `type-annotations.md`.
2. **Checker parse-accept** (`annot.rs`, ~5 lines). Parses and discards K/V,
   produces flat `Ty::Map`. No `Ty` struct change.
3. **Full refinement** (`types/mod.rs` + `guards.rs`, significant). Add
   `map_kv` field; wire `get`/`keys`/`vals`/`assoc` result rules. Do when a
   concrete consumer (LSP hover, a heavy map-using project) justifies the cost.
