# Parametric HOF result types — design note

> Status: **slices 1 + 2 shipped** (`map`/`filter` + `reduce`/`fold`). Branch
> `parametric-types`. Extends the structured-types work (ADR-078): function arrows
> + sequence element types. Implemented via Option B (per-HOF result rules in
> `seq_aware_call_ty`) — no lattice change. Option A (type variables for
> user-defined generics) remains deferred (no consumer).

## Problem

Element types die at the first higher-order call. After ADR-078 a literal carries
its element type — `[1 2 3] : vector<int>` — and `first`/`last`/`nth` flow it out.
But `map`/`filter`/`reduce`/`fold` are typed with a *flat* result:

```
map : (fn|native, seq) -> seq     ; today (curated sig)
```

so `(map inc [1 2 3])` is typed plain `list`, and `(first (map inc [1 2 3]))` is
`any` — the element type is lost. The goal is to make it flow:

```
(map  inc      vector<int>)  : list<number>     ; B = inc's return
(filter even?  vector<int>)  : list<int>         ; element type preserved
(first (map inc [1 2 3]))    : number | nil      ; flows through
```

This is the "result type derived from argument types" step — the only Step 5+ item
with real payoff, since HOFs are pervasive.

## Representation decision

Two ways to express "the result depends on the arguments":

**Option A — type variables + unification.** Give the lattice a `Ty::Var(n)` and
write `map : (A->B, seq<A>) -> list<B>`; on a call, unify the actual arg types
against the param pattern, bind `A`/`B`, substitute into the result. This is real
(if lightweight) parametric polymorphism — general enough for *user-defined*
generic functions later. **Cost:** a new `Ty` kind that ripples through
`union`/`intersect`/`negate`/`is_subtype`/`is_disjoint`/`Display`, plus a unifier,
plus compatibility-contract scrutiny (a type var isn't a set of values). High
surface, real FP-risk in the set ops.

**Option B — per-HOF result rules in the checker (CHOSEN for slice 1).** No lattice
change. Extend the existing `seq_aware_call_ty` (`check/guards.rs`) — the same place
`first`/`list`/`vector` already compute a refined result from the args — with cases
for `map`/`filter`/`reduce`/`fold`. The "dependency" is just Rust that reads the
arg types (`as_arrow`, `elem_ty`) and builds the result. This is exactly the pattern
already shipped for the extractors; it's minimal, advisory-sound, and adds nothing
to the core (ADR-011: ship the simple form, defer power).

**Decision:** Option B now. Option A is the general future form — note it, defer it
until a *user-defined* generic function needs typing (no consumer today; the curated
HOFs are a fixed, small set). If the per-HOF rules grow unwieldy, the intermediate
step is a **declarative result rule on `Sig`** (e.g. `ResultRule::ElementOfArg(2)` /
`ResultRule::ListOfCallbackRet(0)`) — data-driven, still no lattice change — rather
than jumping to full type variables.

## The rules

All results are sound *over-approximations*: when any input is unknown, fall back to
the flat curated result (`list`/`any`) — never a too-narrow type (which could drive
a downstream disjointness false positive). `is_disjoint` stays tags-only, so even a
wrong refinement can't manufacture a warning — but we still aim for correctness.

In Brood `map` always returns a list (`(reverse (fold … nil coll))`), empty input →
`nil`. So a map/filter result is `nil | list<E>`, built as
`Ty::list_of(E).union(Ty::of(Tag::Nil))`.

| call | result rule | needs |
|------|-------------|-------|
| `(filter pred coll)` | `nil \| list<A>`, `A = elem(coll)` | element of arg2 only — trivial, no callback analysis |
| `(map f coll)` | `nil \| list<B>`, `B = ret(f applied to A)` | callback return type |
| `(reduce f init coll)` / `(fold f init coll)` | `B ∪ ty(init)`, `B = ret(f)` | callback return + init type (slice 2) |

**Slice 1 = `filter` + `map`.** `filter` is the trivial, immediately-useful case
(`(first (filter even? [1 2 3])) : int`). `reduce`/`fold` (accumulator-typed) are
slice 2.

### Getting `B` (the callback's return type)

A new helper `callback_ret(heap, callback_form, elem_in, ctx) -> Option<Ty>`:

- **Named global fn** (`(map inc xs)`): `B = sig_of(callback).map(|s| s.ret)`. Reuses
  the existing primitive / curated / inferred sig lookup. `inc` → `number`,
  `even?` → `bool`, etc. (Skip if the name is a local shadow — `ctx.is_local`.)
- **Straight-line lambda literal** `(fn (p) body)` (single simple clause, one param,
  one body expression — the same shape `lambda_literal_arity` and `infer_sig`
  already gate on): bind `p -> elem_in` (or `ANY` when the element type is unknown)
  in a sub-`ctx`, return `expr_ty(body, sub_ctx)`. Sound because a straight-line
  body is unconditional — identity `(fn (x) x)` yields `A`; `(fn (x) (+ x 1))`
  yields `number` regardless of `A`. A branchy / multi-clause / multi-body lambda →
  `None` (bail to flat). This is the **only new inference**, and it's confined to
  *computing a forward result type*, never to *checking* the body (so it does not
  reopen the deferred-#4 guarded-use false-positive class).
- **Anything else** (a local var, an unknown form) → `None` → flat result.

## Exact sites to touch

- `crates/lisp/src/types/check/guards.rs`
  - `seq_aware_call_ty`: add `map` / `filter` arms (and later `reduce`/`fold`).
  - new `fn callback_ret(...)` helper (uses `sig_of`, `ctx.bind`, `expr_ty`).
- No change to `crates/lisp/src/types/mod.rs` (no new `Ty` kind — the win of Option
  B). `Ty::list_of` / `union` / `elem_ty` / `as_arrow` already exist.
- `crates/lisp/src/types/check/sigs.rs` — unchanged (the curated `map`/`filter`
  sigs stay as the *flat fallback* / arity+arrow source; the refinement layers on
  top in `guards.rs`).

## Soundness & tests

- **Soundness rule:** uncertain → flat. Empty-input → include `nil`. Element type
  computed as a superset (unknown → unrefined). `is_disjoint` unchanged (tags-only).
- **Unit tests** (`check.rs`): `(first (map inc [1 2 3])) : number` (no warn against
  a number sink); `(string-length (first (map inc [1 2 3])))` *warns* (number ≠
  string); `(first (filter even? [1 2 3])) : int`; identity lambda preserves
  element type; unknown callback / branchy lambda / unknown coll → no refinement, no
  warning.
- **FP audit:** the gold-standard before/after `brood --check` diff over all project
  `.blsp` files (build the pre-change baseline, assert zero warnings added). Same
  method used for ADR-078.

## Slicing

1. **Slice 1:** `filter` (element-preserving) + `map` (`list<callback-ret>`), with
   `callback_ret` for named fns + straight-line lambdas. Tests + FP audit.
2. **Slice 2:** `reduce`/`fold` (accumulator type = `callback-ret ∪ init`).
3. **Deferred:** Option A (type variables) for user-defined generics — no consumer
   yet.
