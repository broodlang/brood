# Set-theoretic types, applied to Brood

A maintained companion to [`elixir-set-theoretic-types.md`](elixir-set-theoretic-types.md)
(notes on Castagna/Duboc/Valim, *The Design Principles of the Elixir Type
System*, 2024) and to [`../types.md`](../types.md) (Brood's plan + compatibility
contract). This doc is the **Rosetta**: it takes each principle from the paper
and says what Brood's model actually does — match, simplify, or diverge — and
**why**. Keep it current as the type system grows; it's where a "are we still on
the set-theoretic path?" question gets answered.

Source map: theory → `elixir-set-theoretic-types.md`; as-built plan/contract →
`../types.md`; lattice code → `crates/lisp/src/types.rs`; checker →
`crates/lisp/src/types/check.rs` + `check/`.

---

## The one-paragraph difference

**The paper describes a *sound gradual* type system; Brood has built a
*set-theoretic advisory checker*.** Same algebra (types are sets of
values, subtyping is inclusion), same `dynamic()`-inside-the-lattice intent — but
the paper's checker may refuse code / drive runtime checks and must therefore carry
the full soundness machinery (strong/weak arrows, materialization through
application, exhaustiveness). Brood's checker **only ever warns**, only on a
**provable mismatch**, and is engineered for **zero false positives** (ADR-024,
contract #5). That goal difference — not incompleteness — explains most of the
gap below. Several paper features are *out of scope by design*, not missing.

---

## Principle-by-principle status

| Paper principle | Brood status | Where |
|---|---|---|
| Types are sets of values | ✅ exact — a `Ty` is a bitset over the runtime `Tag`s, plus refinements | `types.rs` `Ty` |
| Subtyping = semantic set inclusion | ✅ `is_subtype`: each term of the left fits some term of the right. Sound, deliberately not complete: a term covered only jointly reads as "not a subtype", which defers rather than warns | `types.rs` |
| Union / intersection / negation | ✅ `union` / `intersect` / `negate` (we keep all three primitive; paper encodes `∧` from `∨`,`¬`) | `types.rs` |
| Top `term()` / bottom `none()` | ✅ `Ty::ANY` / `Ty::NEVER` | `types.rs` |
| Singleton types (each atom is a type) | ✅ literal types for keywords, ints, bools and strings (ADR-105/117/120) | `types.rs` `LitSet` |
| Structured: function arrows | ✅ contravariant/covariant subtyping, and intersection of arrows: `(and (int -> int) (bool -> bool))` is an overload with per-call resolution | `types.rs`, `types/sig.rs` |
| Structured: sequence element types | ✅ `vector<T>`/`list<T>` (covariant; sound — seqs immutable) | `types.rs` |
| Structured: tuples | ✅ `(tuple int string)` — a positional refinement of a runtime vector | `types.rs` |
| Structured: maps/records unified | ✅ `(map K V)` and record shapes with width/depth subtyping (ADR-264) | `types.rs` |
| `dynamic()` inside the lattice | ✅ `GradualTy`, consumed by the gradual checks (ADR-110, `walk::gradual_of`) and `nest check --strict` | `types.rs`, `check/walk.rs` |
| Consistent subtyping *derived* from set inclusion (not a Siek–Taha axiom) | ✅ `consistent_with`, used by the same checks | `types.rs` |
| Strong vs weak arrows | 🟡 not modelled statically; a `sig` is a runtime contract under `nest run`/`nest test` (ADR-381) | — |
| Guards refine types (occurrence typing) | ✅ for `(pred? x)`, `%eq`-literal, `and`-shortcircuit; both branches; complement in `else` | `check/guards.rs`, `check/walk.rs` |
| Narrowing **inside** a tested non-variable expr (`is_integer(p.age)`) | ✅ path narrowing over `(get r :age)` and fixed indices, any depth; a computed key stays unsupported | `check/` |
| Guards can *fail*, not just be false (then-only) | ✅ `then_only` flag stops unsound `else` narrowing for `%eq`/`and` | `check/guards.rs` |
| Multi-clause fn = intersection of arrows / input-dependent return | 🟡 per-arm inferred signatures for a same-file multi-arm function; a declared overload resolves per call | `check/ctx.rs`, `check/sigs.rs` |
| Exhaustiveness / redundancy from patterns | ✅ exhaustiveness over literal enums (ADR-118/121) and sealed abilities; redundancy of literal clauses (ADR-122) | `check/exhaustive.rs` |
| Parameter-type inference from guards | 🟡 `file_signatures` infers from the checker's own form-based pass (ADR-188/190/261) | `check/infer.rs` |
| Advisory, never gates | ✅ **stronger** than the paper — never gates the live image; `nest check`'s nonzero exit is the one batch reject | contract #5 |

Legend: ✅ match · 🟡 partial / simplified · ❌ absent.

---

## Where Brood diverges *on purpose*

1. **Advisory-only, zero-false-positive.** The rule is **disjointness, not
   subtyping**: warn only when an argument's type shares *no value* with the
   parameter (`Ty::is_disjoint`). A superset, an `any`, or
   an unknown all overlap and are silent. This is why Brood needs none of the
   paper's machinery for *rejecting* programs — and why a refinement may only ever
   **suppress** a warning, never raise one (the "widen toward `None`" invariant).

2. **`Option<Ty>` for "do I know this type?", `GradualTy` where it matters.** The
   disjointness pass asks `Some(t)` → check, `None` → silent; `None` *is* operationally
   `dynamic()` there. The gradual checks (ADR-110) use `GradualTy` for what disjointness
   cannot see, and `--strict` reads a dynamic value flowing into a precise type. A
   redefinable global is `dynamic()`, never assumed static (contract #4).

3. **A refinement proves disjointness only where it can.** A tuple or a required record
   field of a disjoint type is disjoint. `vector<int>` vs `vector<string>` is *not* (both
   hold the empty vector) — no false positive off an element mismatch. The precise
   callback-arity check is a *separate* dedicated step, not a disjointness result.

4. **We keep `∧` and `¬` primitive** (bitset makes them free) rather than encoding
   `∧` from `∨`+`¬`. Harmless — same algebra.

---

## Open soundness notes & known bugs

### B1 — `negate()` on a *refined* type: fixed
`Ty::negate` once dropped the refinement and so narrowed: `¬(vector<int>)` returned
the non-vectors, a strict subset of the truth, which could manufacture a spurious
disjointness. It now widens a refined term's complement to its tag
(`Ty::negate_term`), an over-approximation that can only ever suppress a warning.

### B2 — `GradualTy` / `consistent_with`: closed
They had no consumers when this was first written. The gradual checks (ADR-110) are
their consumers now; see the status note in [`../types.md`](../types.md).

### B3 — Strong/weak arrows: the soundness hole we're currently *dodging*
We don't model whether a function checks its arguments, and we get away with it
**only because the checker never propagates a `dynamic`/unknown result-refinement
through an application and then refines it.** The paper's pitfalls #1/#3/#4
(`dynamic ≠ top`; `¬dynamic` is a minefield; refining `?` after a *weak* function
is unsound) all bite the moment someone makes `expr_ty` flow types through a call
and tighten them. **Guard rail:** do not add result-refinement-through-dynamic
without the strong/weak distinction (or its equivalent: only refine through callees
whose codomain is guaranteed by a runtime check).

### B4 — `Display` drops `nil` from sequence unions: fixed
A nilable sequence renders its `nil` (`types/display.rs`).

---

## What's genuinely missing (and gated on a real consumer, per ADR-011)

Most of this list has shipped (see the table). What is left:

- **Occurrence typing through a computed key** — a non-literal key or index is
  statically unpinnable.
- **Strong/weak arrows** as a static notion, beyond runtime `sig` contracts.

The honest framing: Brood has the **set-theoretic core, structured types, and
occurrence typing**, wired to an advisory checker, and leaves refusing programs to
the batch `nest check`.
