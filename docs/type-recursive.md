# Recursive types — `(rec X …)` in the lattice (ADR-349)

> Status: **shipped** 2026-09-14. `Ty` carries a μ binder (`mu`) and a self-reference term
> (`rec_ref`); the set relations unroll a recursive type coinductively; the inference
> fixpoints fold an ascent that nests itself into one; the grammar spells it `(rec X body)`.

## Problem

A value type that contains itself had no representation. A JSON value is `nil`, a bool, a
number, a string, a vector of JSON values or a map of them, and the checker's fixpoint over
`std/json.blsp`'s decoder climbed `vector<any>`, `vector<… | vector<any>>`, one level
deeper every round, until `Ty::widened_below` cut it at depth two (ADR-341's stopgap). The
cut is sound and it is a lie of omission: what the decoder returns *is* sayable, and
everything below the cut read as `any`.

## Representation

- `rec_ref: bool` on a **term**: the self-reference `X`. Its tags are `UNIVERSE`, so a
  reader that does not resolve it sees `any` (sound). The set relations treat it as an
  UNKNOWN set — inside nothing but `any` or another reference, containing nothing, never
  provably disjoint, merging with nothing but another reference, `S ∩ T ⊆ T` in an
  intersection, `¬S` unknown. This is what a *dangling* reference means: a reference whose
  binder was not unrolled first.
- `mu: bool` on the **whole `Ty`**: the binder. Every `rec_ref` in its refinement tree —
  outside a nested binder — is this type. One level: a reference names the nearest binder,
  and a nested binder's body is closed. `term_eq` and `PartialEq` compare it; a recursive
  type pushed into a union as an alternative stays a *closed* term with its binder, while
  `body_terms` strips a type's own binder to reach its (open) body.
- `Ty::unroll` substitutes the type for its references one level: the result is not a
  binder, the copies inside are. `Ty::mu(body)` binds; a body with no reference is not
  recursive and comes back as it is (`normalise_mu`).

## The set relations

Every relation unrolls a binder before descending, so the references a consumer meets are
recursive types, not placeholders: `elem_ty`, `map_kv`, `record_field_ty`,
`positional_elems`, `tuple_elem_at` read through the binder; `intersect` and `negate` go
through the unrolling; `union` keeps a recursive side as a closed alternative (exact), or —
on the single-term merge path — lets the references name the merged term, a superset (sound,
and exact when only one side is recursive). `widened_below` leaves a recursive type alone:
it is finite already.

`is_subtype` and `is_disjoint` are **coinductive**: the pair under comparison is assumed
(`REC_ASSUMPTIONS`, a thread-local stack) while their unrollings are compared, and meeting
the same pair again inside answers the assumption — `true` for an inclusion, `false` (not
proven) for a disjointness. Regular types have finitely many pairs of subterms, so the
descent terminates. The same routing applies to a binder that sits inside a union
(`term_is_subtype_of_union`, the term loops of `intersect`/`negate`/`is_disjoint`).

## Inference: fold the ascent, confirm the fold

A fixpoint round computes `next` from `prev`. When `prev` appears INSIDE `next` — as an
element, a value, a field — the round is `next = G[prev]` for a context `G`, and
`Ty::fold_recursive` proposes `μX. G[X]`: `next` with every nested occurrence of `prev`
replaced by the self-reference. A candidate is not an answer. The next round runs on it,
and only when that round **folds back to the candidate** — `G[μ]` is `μ` — is it accepted:
that makes `μ` a fixpoint of the round, hence above the least one, hence sound. Until then
the ascent continues from the round's own value, which is above the un-folded sequence
(`prev ⊆ μ`, `G` monotone). A value that recurses through another function's parameter
grows every other round, so both of the last two values are tried.

Applied in Pass 2.9's two fixpoints — the caller-derived parameters
(`sigs::caller_derived_params`) and the returns (`check.rs`) — ahead of the depth
widening, which remains for what neither converges nor folds.

## Grammar and rendering

`(rec X body)`: `X` inside `body` is the whole; `(rec json (or nil bool number string
(vector json) (map string json)))`. Rendered `(rec X nil | … | vector<X>)`; nested binders
take `Y`, `Z`. `to_source` round-trips it. A body that names no `X` is the body.

## Deferred

- **Nested self-reference across binders** (`X` inside an inner `rec` naming the outer):
  one level of binding only — an inner binder is closed. Inference never produces the
  other shape; the grammar would need de Bruijn indices to say it.
- **A named type alias** (`(deftype json …)`) so a `sig` can name the recursive type once:
  the spelling is `(rec …)` inline for now.
- **The runtime contract**: `type-matches?` has no `rec` case; a `sig!` over a recursive
  type accepts (an unknown compound accepts, by its existing rule).
