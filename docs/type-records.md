# Record/shape types — `(record :k1 T1 :k2 T2 …)` in the type grammar

> Status: **slice 1 + 2 shipped** (Brood runtime, checker refinement, sinks,
> and literal inference). `type-matches?` checks each declared field's
> presence/type at the runtime-contract boundary; `Ty` carries a full `fields`
> refinement with width/depth subtyping; `(get r :k)` on a declared or
> inferred record resolves to the exact field type; a `{…}` map literal infers
> its own record shape with no annotation needed. Remaining smaller items
> (closed records, `assoc`/`keys`/`vals` field-precise sinks) deferred until a
> real consumer drives them — see [Deferred](#deferred).

## Problem

`(map K V)` ([`type-map-kv.md`](type-map-kv.md)) gives a map *uniform* key/value
types — `(map keyword int)` says every value in the map is an `int`. Most
config/options/record-shaped maps aren't uniform: different keys carry different
types, and some keys are optional.

```lisp
(sig! make-window ((record :title string :width int) -> any))
(make-window {:title "main"})              ; missing :width — rejected
(make-window {:title 42 :width 800})       ; :title is not a string — rejected
```

Before this, the best `(sig …)` could say about such a value was bare `map` —
zero field-level checking. `(record …)` validates each declared field's
presence and type at the runtime-contract boundary, with required-by-default
fields and an explicit opt-in for optional ones — and, on the checker side, an
exact per-field `get` result type instead of a flat `V | nil`.

## Grammar extension

```
type ::= …
       | (record key-type-pair…)
key-type-pair ::= keyword field-type
field-type ::= type | (optional type)
```

`(record :name string :age (optional int))` = "a map with a required `:name`
key of type `string`, and an optional `:age` key that, if present, is an
`int`". Fields not listed are unconstrained and **allowed** — records are
**open** (structural width subtyping in the permissive direction), not closed:
any map with the declared fields present and correctly typed satisfies the
record, regardless of what else it carries. A closed-record variant (reject
unknown keys) is a pure addition later if a real need appears — see
[Deferred](#deferred).

Why list-headed rather than reusing the `{…}` map-literal reader syntax: every
other compound type — `(map K V)`, `(vector E)`, `(list E)`, `(and A B)`,
`(or A B)` — is a `(head args…)` list that `parse_type` dispatches on by head
symbol. `(record …)` slots into that same dispatch. (The checker's *literal
inference*, below, does read `{…}` directly — but that's inferring a type
*from a value*, a different problem from parsing a type *annotation*.)

## Representation

### Runtime (`type-matches?`) — Brood only, no Rust

`std/prelude.blsp`, alongside the existing `map` branch:

```lisp
;; in the (pair? t) / (cond ...) branch of type-matches?:
(%eq h 'record)
  (and (map? v)
       (every? (fn (field)
                 (let (k (first field)
                       ftype (second field))
                   (if (and (pair? ftype) (%eq (first ftype) 'optional))
                     (or (nil? (get v k))
                         (type-matches? (second ftype) (get v k)))
                     (type-matches? ftype (get v k)))))
               (partition 2 (rest t))))
```

`partition` (already in `prelude.blsp`) turns the flat field list `(:name
string :age (optional int))` into `((:name string) (:age (optional int)))`
pairs. **No separate presence check is needed for required fields**: `(get v
k)` on a missing key returns `nil`, and `type-matches?` on the bare field type
then fails on its own unless that type happens to accept `nil` — the same
trick `(map K V)`'s branch already relies on for its key/value checks.

### Static checker — the `fields` refinement (`types/mod.rs`)

`Ty` carries a `fields: Option<Arc<BTreeMap<Symbol, (Ty, bool)>>>` refinement
(field name → declared type, `required?`), alongside `arrow`/`elem`/`map_kv`
and tagged `MAP_BIT` like `map_kv` — a record is still a runtime `map` value;
this only refines it, the same trick `keyword_lit` uses layering onto the
`Keyword` tag. `Ty::record_of(fields)` builds one; `Ty::record_fields()` reads
it back.

`parse_type` (`annot.rs`) builds the field map directly off the `(record …)`
form, peeling each field's `(optional T)` wrapper (`unwrap_optional`) to get
its `required` flag:

```rust
if value::symbol_is(head, "record") {
    let rest = &items[1..];
    if rest.len() % 2 != 0 {
        return None; // malformed — odd field-list length
    }
    let mut fields = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        let Value::Keyword(name) = pair[0] else { return None };
        let (field_form, required) = match unwrap_optional(heap, pair[1]) {
            Some(inner) => (inner, false),
            None => (pair[1], true),
        };
        fields.insert(name, (parse_type(heap, field_form)?, required));
    }
    return Some(Ty::record_of(fields));
}
```

**Subtyping (`is_subtype`) — width and depth, deliberately conservative.** For
every field `other` declares, `self` must also declare it (required if
`other` requires it) with a covariant field type; a field `other` doesn't
declare imposes no constraint, so `self` may carry extra fields freely (width,
the open-record direction). If `self` simply doesn't declare a field `other`
does — even one `other` only marks *optional* — subtyping returns `false`
rather than trying to prove the relationship anyway:

```rust
fn record_fields_is_subtype(
    self_fields: &BTreeMap<Symbol, (Ty, bool)>,
    other_fields: &BTreeMap<Symbol, (Ty, bool)>,
) -> bool {
    for (name, (other_ty, other_required)) in other_fields {
        match self_fields.get(name) {
            None => return false,
            Some((self_ty, self_required)) => {
                if *other_required && !*self_required { return false; }
                if !self_ty.is_subtype(other_ty) { return false; }
            }
        }
    }
    true
}
```

This is **sound but not complete** by construction (`docs/types.md` contract
#5): it may miss a true subtype relation (e.g. an empty record *is* a subtype
of any-record-with-only-optional-fields, but this algorithm says no), but it
never claims a false one. See `crates/lisp/src/types/mod.rs`'s
`record_subtyping_is_width_and_depth_but_conservative` test for the exact
cases this covers and deliberately doesn't.

**Union/intersect — reused verbatim, no new algorithm.** `fields` is threaded
through the *existing* generic `merge_union`/`merge_intersect` helpers exactly
like `map_kv` already is: two equal field maps survive a union/intersect
unchanged; two different ones widen to `None` (no declared shape). This was a
deliberate simplification over a fancier field-wise union (union each shared
field, demote a required-on-one-side field to optional) — the blunt
widen-unless-identical rule is already the established sound pattern for
*every* refinement in this lattice, so records get it for free rather than
inventing new merge logic. Less precise on a union of two different record
shapes, but sound, and consistent with how `arrow`/`elem`/`map_kv` already
behave.

**`is_disjoint` — untouched, tags-only.** Like every other refinement, a
`fields` mismatch is never inspected by disjointness — it can only *miss* a
warning, never manufacture a false one.

**`display`** — a record renders as `{name: string, age?: int}` (`?` marks an
optional field, sorted by field name since `Symbol` is an interned `u32` and
sorts by intern order, not spelling — same trap `lit`'s rendering already
avoids).

### Static checker — sinks and literal inference (`check/guards.rs`)

**`(get r :k [default])`** on a record with a **literal keyword** key resolves
to the exact field type (unioned with `nil`, since `get` always admits the
"absent" case) — more specific than the flat `map_kv` fallback. A dynamic or
undeclared key falls through: records are open, so an unknown key's type is
genuinely unknown, not an error.

**Record-literal type inference** — `expr_ty` gained a `Value::Map` arm (it
previously had none at all, unlike vector literals which already infer
`vector_of(element_union(…))`): every keyword-literal key in a `{…}` literal is
definitely present (it's data, evaluated once), so each resolvable `:key
value` pair becomes a *required* field. A non-keyword key, or a value whose
own type is unknown, is simply **omitted** from the inferred shape — sound
(under-declaring a field only widens what the type claims), never a false
positive. This means `(get {:a 1} :a)` gets a precise type with **no `sig`
annotation at all**.

## Deferred

Smaller items, each additive, gated on a real consumer (ADR-011):

- **Closed records** (reject unknown keys) — a separate opt-in marker (e.g.
  `(record! …)` or a trailing marker in the field list) if open-by-default
  ever proves too permissive for a real use case.
- **`assoc`/`keys`/`vals` field-precise sinks** — `(assoc r :k v)` returning an
  updated record shape, and `(keys r)`/`(vals r)` unioning across declared
  field types, weren't built (only `get` was, as the highest-value case). Both
  fall through to the flat/unresolved case today (sound, just less precise).
- **A less conservative subtyping algorithm** — the current one requires
  `self` to declare every field `other` does, even optional ones (see the
  `is_subtype` section above); a smarter version could prove more relations
  (e.g. "self has no info about field X, other marks X optional" is actually
  fine) at the cost of real additional design work.
- **Field-wise union/intersect** — see the "reused verbatim" note above; a
  union that combines two different record shapes field-by-field (rather than
  widening to no shape at all) is more precise but wasn't needed to keep
  things sound.

## Soundness

- **Runtime:** `type-matches?`'s record branch is O(fields) per call (plus one
  `get` per field, itself O(1) amortized on the CHAMP map) — same cost profile
  as `(map K V)`'s O(entries) walk. Prefer static-only `sig` over `sig!` on a
  hot path.
- **Checker:** every piece above was verified against contract #5 — see
  `record_subtyping_is_width_and_depth_but_conservative`,
  `record_union_widens_on_field_mismatch_but_keeps_a_match`, and
  `record_is_disjoint_only_on_tags_like_every_other_refinement` in
  `crates/lisp/src/types/mod.rs`, plus `record_field_refinement_flows_through_checker`
  in `crates/lisp/src/types/check.rs`. The record-literal inference (the
  highest-blast-radius piece — it changes the inferred type of *every* map
  literal project-wide) was diffed against `nest check` output across the
  whole `std/` + `tests/` corpus with the new inference arm disabled vs.
  enabled: **zero new warnings**.

## Tests

`tests/contract_test.blsp`, `describe "record type checks (record …)"`:
runtime-contract coverage (required/optional/open, non-map rejection).
`crates/lisp/src/types/check.rs`: `record_type_annotation_parses_and_accepts_valid_calls`
(grammar + malformed-annotation handling) and
`record_field_refinement_flows_through_checker` (the `get` sink + literal
inference, both the false-negative-avoidance and the true-positive cases).
`crates/lisp/src/types/mod.rs`: `record_renders_as_a_field_shape`,
`record_subtyping_is_width_and_depth_but_conservative`,
`record_union_widens_on_field_mismatch_but_keeps_a_match`,
`record_is_disjoint_only_on_tags_like_every_other_refinement`.
