# Set-theoretic types, applied to Brood

A maintained companion to [`elixir-set-theoretic-types.md`](elixir-set-theoretic-types.md)
(notes on Castagna/Duboc/Valim, *The Design Principles of the Elixir Type
System*, 2024) and to [`../types.md`](../types.md) (Brood's plan + compatibility
contract). This doc is the **Rosetta**: it takes each principle from the paper
and says what Brood's model actually does — match, simplify, or diverge — and
**why**. Keep it current as the type system grows; it's where a "are we still on
the set-theoretic path?" question gets answered.

Source map: theory → `elixir-set-theoretic-types.md`; as-built plan/contract →
`../types.md`; lattice code → `crates/lisp/src/types/mod.rs`; checker →
`crates/lisp/src/types/check.rs` + `check/`.

---

## The one-paragraph difference

**Elixir is building a *sound gradual* type system; Brood has built a
*set-theoretic advisory disjointness linter*.** Same algebra (types are sets of
values, subtyping is inclusion), same `dynamic()`-inside-the-lattice intent — but
Elixir's checker may refuse code / drive runtime checks and must therefore carry
the full soundness machinery (strong/weak arrows, materialization through
application, exhaustiveness). Brood's checker **only ever warns**, only on a
**provable disjointness**, and is engineered for **zero false positives** (ADR-024,
contract #5). That goal difference — not incompleteness — explains most of the
gap below. Several paper features are *out of scope by design*, not missing.

---

## Principle-by-principle status

| Paper principle | Brood status | Where |
|---|---|---|
| Types are sets of values | ✅ exact — a `Ty` is a `u32` bitset over the 17 runtime `Tag`s | `mod.rs` `Ty.tags` |
| Subtyping = semantic set inclusion | ✅ `is_subtype` is bit-containment (+ refinements) | `mod.rs:308` |
| Union / intersection / negation | ✅ bitwise OR / AND / complement (we keep all three primitive; paper encodes `∧` from `∨`,`¬`) | `mod.rs:252–298` |
| Top `term()` / bottom `none()` | ✅ `Ty::ANY` / `Ty::NEVER` | `mod.rs:125–127` |
| Singleton types (each atom is a type) | ❌ not modelled — `:ok` is just `keyword`, `5` is just `int`. We narrow *via* `%eq`-against-a-literal guards but the lattice has no singletons | — |
| Structured: function arrows | 🟡 single arrow refinement, contravariant/covariant subtyping; **no intersection of arrows** | `mod.rs:165`, `Sig::is_subtype` |
| Structured: sequence element types | 🟡 `vector<T>`/`list<T>` (covariant; sound — seqs immutable) | `mod.rs:182–200` |
| Structured: tuples | ❌ Brood has no tuple kind (vectors carry one element type, not positional) | — |
| Structured: maps/records unified | ❌ `map` is one flat tag; no key/value typing | — |
| `dynamic()` inside the lattice | 🟡 `GradualTy` exists & is unit-tested — **but has no consumer** (dead code) | `mod.rs:468` |
| Consistent subtyping *derived* from set inclusion (not a Siek–Taha axiom) | 🟡 `consistent_with` does this for the flat lattice — **but unused** | `mod.rs:640` |
| Strong vs weak arrows | ❌ not modelled (and not needed *yet* — see below) | — |
| Guards refine types (occurrence typing) | ✅ for `(pred? x)`, `%eq`-literal, `and`-shortcircuit; both branches; complement in `else` | `check/guards.rs`, `check/walk.rs:586` |
| Narrowing **inside** a tested non-variable expr (`is_integer(p.age)`) | ❌ only bare `(pred? sym)` | — |
| Guards can *fail*, not just be false (then-only) | ✅ `then_only` flag stops unsound `else` narrowing for `%eq`/`and` | `check/guards.rs:86` |
| Multi-clause fn = intersection of arrows / input-dependent return | ❌ multi-arm closures get no inferred sig | `check/sigs.rs:127` |
| Exhaustiveness / redundancy from patterns | ❌ out of scope (we lint non-tail recursion instead) | `check/recursion.rs` |
| Parameter-type inference from guards | 🟡 only straight-line one-expression bodies (`infer_sig`) | `check/sigs.rs:120` |
| Advisory, never gates | ✅ **stronger** than the paper — Brood *never* rejects | contract #5 |

Legend: ✅ match · 🟡 partial / simplified · ❌ absent.

---

## Where Brood diverges *on purpose*

1. **Advisory-only, zero-false-positive.** The rule is **disjointness, not
   subtyping**: warn only when an argument's type shares *no tag* with the
   parameter (`is_disjoint`, tags-only — `mod.rs:342`). A superset, an `any`, or
   an unknown all overlap and are silent. This is why Brood needs none of the
   paper's machinery for *rejecting* programs — and why a refinement may only ever
   **suppress** a warning, never raise one (the "widen toward `None`" invariant).

2. **`Option<Ty>` instead of `GradualTy` in the checker.** The disjointness pass
   only asks "do I know this type?" — `Some(t)` → check, `None` → silent. `None`
   *is* operationally `dynamic()` for a pure disjointness check, so the gradual
   apparatus isn't wired in. Globals (redefinable under hot reload) are simply
   never tracked → always `None` → never flagged, which honours contract #4
   ("redefinable bindings are `dynamic()`, never assumed static") behaviourally.

3. **Refinements never affect disjointness.** `is_disjoint` is tags-only by
   construction, so `vector<int>` vs `vector<string>` is *not* disjoint (both are
   vectors) — no false positive off an element/arrow mismatch. The precise
   callback-arity check is a *separate* dedicated step, not a disjointness result.

4. **We keep `∧` and `¬` primitive** (bitset makes them free) rather than encoding
   `∧` from `∨`+`¬`. Harmless — same algebra.

---

## Open soundness notes & known bugs

### B1 — `negate()` on a *refined* type is unsound (latent)
`Ty::negate` is `!tags & UNIVERSE` with the refinement dropped (`mod.rs:292`). The
doc-comment claims "the result is always unrefined (widen — sound)", but for a
refined input this **narrows**, it doesn't widen:

```
negate(vector<int>)  -- true complement = (non-vectors) ∪ (vectors with a non-int elem)
                     -- impl returns    = (non-vectors)            ← strict SUBSET, drops the vector tag
```

A complement that returns a *subset* of the truth is the unsound direction: fed
into `is_disjoint` it could manufacture a spurious disjointness → a false
positive. **Not currently triggerable** — `negate` is only ever called on *flat*
types in live code (`check/guards.rs:123` and `check/walk.rs:612`, both on
`tested_by`/`%eq` results, all flat — verified). And `difference` (the only other
caller) is test-only. So this is a **Step-5 footgun**, not a live bug.

The sound over-approximation of `¬(vector<int>)` is to **keep the container tag**
(some vectors *are* in the complement), i.e. add `vector` back unrefined — which
collapses to `ANY` here. Fix options: (a) make `negate` keep any refined tag in
the output, (b) `debug_assert!` the input is flat, or (c) document it as a known
approximation valid only for flat inputs. Note `lattice_laws_hold` (`mod.rs:849`)
only samples **flat** types, so De Morgan / double-negation are *unverified* for
refined types — the green test suite gives false confidence here.

### B2 — `GradualTy` / `consistent_with` are dead code
The paper's central contribution lives in `mod.rs:468–660`, fully unit-tested, with
**zero consumers** (grep: only referenced in a comment and its own tests). Brood
today is a set-theoretic *disjointness* checker, not a gradual one. This is
*defensible* (the disjointness pass genuinely doesn't need it) but should be
**called what it is** in `../types.md` — "foundation for a later gradual-assignment
checker", not part of the working system. Decision to make: wire it in when a real
gradual-*assignment* consumer arrives, or keep it as a clearly-labelled island.

### B3 — Strong/weak arrows: the soundness hole we're currently *dodging*
We don't model whether a function checks its arguments, and we get away with it
**only because the checker never propagates a `dynamic`/unknown result-refinement
through an application and then refines it.** The paper's pitfalls #1/#3/#4
(`dynamic ≠ top`; `¬dynamic` is a minefield; refining `?` after a *weak* function
is unsound) all bite the moment someone makes `expr_ty` flow types through a call
and tighten them. **Guard rail:** do not add result-refinement-through-dynamic
without the strong/weak distinction (or its equivalent: only refine through callees
whose codomain is guaranteed by a runtime check).

### B4 — `Display` drops `nil` from sequence unions (cosmetic)
`nil | list<int>` renders as `list<int>` (`mod.rs:422` masks out `Nil`). A
`(map …)` result diagnostic will say `got list<int>` when the value can be `nil`.
Low severity; can mislead.

---

## What's genuinely missing (and gated on a real consumer, per ADR-011)

- **Intersection of arrows** → input-dependent return types for multi-clause
  functions. The single biggest expressiveness gap vs the paper.
- **Singleton/literal types** → `:ok` vs `:error`, exhaustive case analysis.
- **Map/record types** (key ⇒ value, required/optional, open) → static `KeyError`
  elimination, the paper's unified-maps payoff.
- **Tuple/positional product types.**
- **Exhaustiveness & redundancy** checking from `match`/`case`.
- **Occurrence typing through non-variable test expressions.**

None are bugs; all are additive and deliberately deferred until a concrete need
justifies the complexity. The honest framing: Brood has the **set-theoretic core
and a tasteful subset of occurrence typing**, wired to an advisory linter — and has
*not* yet built the gradual-typing or structured-type-richness layers the paper's
soundness story is mostly about.
