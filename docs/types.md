# Brood types — set-theoretic, gradual, advisory

**Status:** steps 1–2 of 5 implemented (`crates/lisp/src/types.rs`). This doc is the
plan *and* the compatibility contract: the staircase says what to build next, the
[Compatibility contract](#compatibility-contract) says what every other change
must respect so we never drift off this path. Decision recorded in
[ADR-024](decisions.md) (refining [ADR-023](decisions.md)).

## The decision, in one paragraph

Brood's types follow the **Elixir model — set-theoretic and gradual** — not
TypeScript's pragmatic-but-unsound one. A type *is a set of values*; subtyping is
set inclusion; what can't be pinned down statically is `dynamic()` and mixes
soundly with the rest. Checking is **advisory**: it warns and optimises, it never
rejects a runnable program (the one exception — provably-sound special-form
*structure* errors — can't be wrong because special forms aren't redefinable).
The language stays fully dynamic; types never inhibit it. Mechanism lives in Rust
(`Ty`, the `Tag` atoms, primitive signatures); policy (`assert-type`, contracts)
lives in Brood (ADR-006).

Reading: Castagna, Duboc, Valim, *"The Design Principles of the Elixir Type
System"* (‹Programming›, 2024; on arXiv) and the semantic-subtyping / set-theoretic
lineage behind it.

## The model

A `Ty` **is a set of values**, and the type operations *are* set operations:

| Type op | Set op | In `types.rs` |
|---|---|---|
| union (`int \| float`) | `∪` | `Ty::union` (bitwise OR) |
| intersection | `∩` | `Ty::intersect` (AND) |
| negation ("not nil") | `¬` | `Ty::negate` (complement) |
| **subtyping** | `⊆` inclusion | `Ty::is_subtype` — *semantic*, no syntactic rules |

- **Atoms** are the 12 runtime [`Tag`](../crates/lisp/src/value.rs)s
  (`int float string symbol keyword bool nil pair vector fn macro native`). The
  type universe is built from these; `type-of` observes one at runtime.
- `Ty::NEVER` = `⊥` (empty set, subtype of everything); `Ty::ANY` = `⊤` (all
  tags); the named unions `Ty::NUMBER` (`int∪float`), `Ty::LIST` (`nil∪pair`)
  match the `number?`/`list?` predicates.
- **`dynamic()`** *(step 2, not built yet)* is the **gradual** type — and it
  lives *inside* the set-theoretic algebra, not bolted beside it. It's a bounded
  type `dynamic(bound)` (pure `dynamic()` = `dynamic(ANY)`) whose `bound` is an
  ordinary set-of-tags `Ty`, read as the interval between its optimistic (`⊥`)
  and pessimistic (`⊤`) materialisations. Crucially, **consistent subtyping is
  *derived from* ordinary set inclusion** — not a separate, non-set "consistency"
  axiom (the classic Siek–Taha framing). For our flat lattice the derived rule is
  simply: `dynamic(b)` is consistent-compatible with `t` iff `b ∩ t ≠ ⊥` (some
  materialisation fits); static-vs-static stays plain `<:`. So `dynamic()`
  composes with `∪`/`∩`/`¬` like any type and honours [contract point
  #2](#compatibility-contract). Anything whose type can't be pinned — above all a
  **redefinable global under hot reload** — is `dynamic()`, **not** `ANY` (`ANY`
  relates by subtyping and *would* error when an `int` is wanted; `dynamic()`
  defers). This is the valve that lets typing coexist with live redefinition.
  (Castagna & Lanvin, ICFP 2017; Castagna et al., POPL 2019 — the reconciliation
  Elixir uses.)
- **Structured types** (function arrows `int -> int`, a vector's element type)
  are a later step; today `Ty` is flat (sets of tags only).

## The staircase — tackle one at a time

Each step is self-contained, ships green, and is useful on its own. "Done when"
is the checkable boundary.

### Step 0 — runtime tags first-class ✅ (ADR-023)
First-class `Tag` + `(type-of x)`, self-identifying type errors, and an `Arity`
on every builtin enforced at one gate (`eval::call_native`).
**Done:** tag is observable; errors name op/wanted/got; arity is metadata.

### Step 1 — the set-theoretic `Ty` lattice ✅
`crates/lisp/src/types.rs`: `Ty` as a set of tags with union/intersect/negate/
difference, semantic subtyping, `NEVER`/`ANY`/`NUMBER`/`LIST`, `of_value` bridge,
`Display`. Pure algebra; nothing in the language consumes it yet.
**Done:** the algebra exists and is unit-tested in isolation.

### Step 2 — `dynamic()`, the gradual type ✅
`types.rs`: `GradualTy { bound: Ty, dynamic: bool }` — `dynamic(bound)` kept
*inside* the lattice (pure `dynamic()` = `dynamic(ANY)`). `consistent_with` is
**derived from set inclusion** (static → `bound ⊆ expected`; dynamic → `bound ∩
expected ≠ ⊥`), not a primitive consistency axiom — so pure `dynamic()` is
consistent with every inhabited type while `dynamic(number)` is still caught
against `string`. Composes via `union`/`intersect`/`negate`. The
"redefinable/free/global references are `dynamic()`" rule is documented (the
struct doc + ADR-024); no checker consumes it yet.
**Done:** the gradual type and its derived relation exist and are unit-tested.

### Step 3 — typed signatures on primitives ⬜
Give each `NativeFn` a result `Ty` (and argument `Ty`s) beside its `Arity` — same
single-source-of-truth pattern. e.g. `%add: (number, number) -> number`,
`cons: (any, any) -> pair`, predicates `: (any) -> bool`, `type-of: (any) -> keyword`.
**Done when:** every builtin declares a signature (compiler-enforced, like
`Arity`), and a signature is queryable for a given primitive.

### Step 4 — local, advisory inference ⬜
A pass over the **macro-expanded** forms (after `macros::macroexpand_all`,
ADR-022): literals → singleton `Ty`; primitive calls → result `Ty`;
**guard/pattern narrowing** mined from the matcher (`(if (int? x) …)`,
`match` clauses) for occurrence typing. Globals are `dynamic()`. Output is
**warnings** (provable misuse near its source) and, later, specialisation.
*Prepped:* the predicate→type bridge this needs is already in place —
`Ty::tested_by("int?") → int`, `"number?" → number`, `"list?" → list`, etc.
**Done when:** a body that provably misuses a value warns at compile time, and no
correct program is rejected.

### Step 5+ — structured types ⬜
Function arrows, vector/list element types, intersections for overloaded fns —
the fuller set-theoretic algebra. Additive; gated on real need (ADR-011).

## Compatibility contract

Every change — new primitive, new special form, new `Value` kind, new feature —
must keep these true, so future work stays on the set-theoretic path. Items
marked **(enforced)** are compile errors if violated; the rest are review rules.

1. **Every value has exactly one tag.** The 12 `Tag`s are the type atoms. A new
   `Value` variant must get a `Tag` (in `value::tag`) and a bit (in
   `types::bit`). **(enforced)** — both are exhaustive `match`es, so omission
   won't compile. Don't introduce a value kind that can't be a tag.
2. **A type is a set of values.** Don't add a typing concept that isn't a set
   (no nominal-only identity, no escape hatch that breaks set semantics).
   Structured types arrive as proper set-theoretic extensions, never bolt-ons.
3. **Subtyping is inclusion.** Never add an ad-hoc subtyping rule. `a <: b` iff
   `a`'s value set ⊆ `b`'s — full stop. This is precisely what keeps us off the
   TypeScript route.
4. **Redefinable bindings are `dynamic()`, never assumed static.** Any feature
   touching `def` / globals / hot reload must keep them `dynamic()` so a checker
   can never contradict a future redefinition. This is the "don't inhibit the
   language" invariant.
5. **Checking is advisory.** No change may let a type result *reject* a runnable
   program — except provably-sound special-form *structure* errors (special forms
   aren't redefinable, so those can't be wrong). Types warn and optimise; they
   never gate.
6. **Every primitive declares its type (step 3 onward).** A new builtin supplies
   a result `Ty` (+ arg `Ty`s) next to its `Arity`. Will be **(enforced)** once
   `NativeFn` carries the field — the same mechanism that made `Arity` mandatory.
7. **Policy in Brood.** If a type test or contract can be written in Brood over
   `type-of`/predicates, it goes in `std/`, not Rust (ADR-006).
8. **Pattern/guard forms expose their refinement.** New pattern kinds or guards
   must remain analysable for occurrence typing — the matcher is the inference
   goldmine (step 4). Don't add opaque guards that hide the type they imply.
9. **Errors, `type-of`, and `Ty` agree on names.** All use `Tag::name`
   spellings, so a `Ty` in a message reads the same as `type-of` returns.

## Where it lives

- `crates/lisp/src/types.rs` — the `Ty` lattice (step 1) and `GradualTy` (step 2).
- `crates/lisp/src/value.rs` — `Tag` (the atoms), `value::tag`, `NativeFn` (gets
  a signature in step 3).
- `crates/lisp/src/eval.rs` — `call_native` (arity gate today; the natural place
  the checker’s results would feed specialisation later).
- `crates/lisp/src/macros.rs` — `macroexpand_all`, the compile pass step 4 runs
  after.
