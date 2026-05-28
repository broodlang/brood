# Brood types — set-theoretic, gradual, advisory

**Status:** steps 1–2 done; 3–4 started — a v0 advisory checker (`(check 'form)`)
is the lattice's first consumer (`crates/lisp/src/types/{mod,check}.rs`). This doc is the
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

| Type op | Set op | In `types/mod.rs` |
|---|---|---|
| union (`int \| float`) | `∪` | `Ty::union` (bitwise OR) |
| intersection | `∩` | `Ty::intersect` (AND) |
| negation ("not nil") | `¬` | `Ty::negate` (complement) |
| **subtyping** | `⊆` inclusion | `Ty::is_subtype` — *semantic*, no syntactic rules |

- **Atoms** are the 12 runtime [`Tag`](../crates/lisp/src/core/value.rs)s
  (`int float string symbol keyword bool nil pair vector fn macro native`). The
  type universe is built from these; `type-of` observes one at runtime.
- `Ty::NEVER` = `⊥` (empty set, subtype of everything); `Ty::ANY` = `⊤` (all
  tags); the named unions `Ty::NUMBER` (`int∪float`), `Ty::LIST` (`nil∪pair`)
  match the `number?`/`list?` predicates.
- **`dynamic()`** *(step 2, `GradualTy`)* is the **gradual** type — and it
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
  Elixir uses.) **Note:** the advisory *checker* (Step 4) doesn't use `GradualTy`
  — it carries `Option<Ty>` (known / unknown). `dynamic()` is foundation for a
  later gradual-*assignment* checker, not the disjointness pass.
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
`crates/lisp/src/types/mod.rs`: `Ty` as a set of tags with union/intersect/negate/
difference, semantic subtyping, `NEVER`/`ANY`/`NUMBER`/`LIST`, `of_value` bridge,
`Display`. Pure algebra; nothing in the language consumes it yet.
**Done:** the algebra exists and is unit-tested in isolation.

### Step 2 — `dynamic()`, the gradual type ✅
`types/mod.rs`: `GradualTy { bound: Ty, dynamic: bool }` — `dynamic(bound)` kept
*inside* the lattice (pure `dynamic()` = `dynamic(ANY)`). `consistent_with` is
**derived from set inclusion** (static → `bound ⊆ expected`; dynamic → `bound ∩
expected ≠ ⊥`), not a primitive consistency axiom — so pure `dynamic()` is
consistent with every inhabited type while `dynamic(number)` is still caught
against `string`. Joins branch types via `union`; gradual `intersect`/`negate`
are deferred until a consumer needs them (ADR-011 — don't ship unproven
operators). The "redefinable/free/global references are `dynamic()`" rule is
documented (the struct doc + ADR-024); no checker consumes it yet.
**Done:** the gradual type and its derived relation exist and are unit-tested.

### Step 3 — signatures the checker reads ✅
A callee's signature (argument `Ty`s + result `Ty`) comes from three sources,
simplest-first — deliberately **no inference engine** (see the rationale in
[How it runs](#how-it-runs--and-why-its-outside-the-runtime)):

- ✅ **Primitives** — every [`NativeFn`](../crates/lisp/src/core/value.rs)
  carries a [`Sig`](../crates/lisp/src/types/mod.rs) field next to its `Arity`
  (compatibility-contract point #6, **enforced** — there's no way to construct
  a `NativeFn` without one). The checker reads it via a global-env lookup
  (`check::primitive_sig`); there is no parallel hand-maintained table.
  Primitives whose args/result aren't usefully pinned use the explicit
  `Sig::any()` lane (`(...any) -> any`) — overlaps every input, so the
  disjointness checker never warns against it.
  Example sigs: `%add: (number,number)→number`, `first: (list|vector)→any`,
  `string-length: (string)→int`, `string->number: (string)→number|nil`.
- ✅ **Curated stdlib** — a small hand-written table for the variadic /
  `reduce`-based / higher-order Brood closures the checker can't infer but that
  matter: `+ - * / < <= > >= mod`, `map`, `filter`, `reduce`. Hand-vetted, so
  sound. This is what makes `(+ 1 "x")` catchable even though `+` is
  `(reduce %add 0 xs)`.
- ✅ **Basic inference** (`check::infer_sig`) — *only* for a fn whose body is a
  **single straight-line expression** (no `if`/`cond`/`when`/`let`/`match`/
  recursion, no `&optional`/rest params): each closure parameter inherits the
  type the callee expects at the position(s) where the parameter is used
  directly (intersected across positions); the closure's return is the
  callee's. Anything with a branch / binding / recursion → infer nothing.
  Sound **because a straight-line use is unconditional** — no control-flow
  analysis, no fixpoint, no false-positive class. The callee is itself only
  looked up via the *non-inferring* `primitive_sig`/`curated_sig` (so a chain
  `defn a (x) (b x)` / `defn b (x) (a x)` can't loop). Catches one-liner
  wrappers (`inc`, `twice`, simple user `defn`s); skips everything subtle.

**Deferred (⬜):** inference through branches / guards / recursion / higher-order.

### Step 4 — the advisory checker 🟡 (v0 shipped; plan below)
`crates/lisp/src/types/check.rs`: walk a macro-expanded form and **warn when a
call passes a provably-wrong argument** — its type is *disjoint* from what the
callee accepts (`(first 5)`; `(+ 1 "x")` once `+` has a curated sig).
Disjointness (not subtyping) is the rule, so a superset / unknown argument is
never a false positive.

- **Vocabulary: `Option<Ty>`, not `GradualTy`.** The checker only asks "do I know
  this argument's type?": `Some(t)` → check disjointness against the param;
  `None` (a variable, an unknown call) → stay silent. The gradual machinery
  isn't needed until we check *assignments*; the disjointness checker doesn't, so
  it stays out of the hot path.
- **Skip inside `try` / `error-of` / `assert-error`** — those forms deliberately
  exercise failures, so don't flag code within them (keeps `nest test` quiet on
  error-path tests).
- **Advisory, always** — returns warnings; never raises, never gates (contract #5).
- ✅ **v0 shipped:** the `(check 'form)` builtin + `brood --check <file>`
  (located warnings).
- ✅ **Step-3 coverage:** primitive sigs sourced from `NativeFn` (enforced;
  no parallel table), curated stdlib sigs for `+`/`<`/`map`/…, and inference
  for straight-line single-expression closures (so a user `(defn inc (x) (+ x
  1))` participates without a hand-written sig).
- ✅ **Guard narrowing + let-binding tracking** (the second behavioural payoff):
  the checker now threads a `Ctx { sym → Ty }` of locally-known types through
  the walk. A `let`/`let*` binding seeds the variable with the RHS's
  `expr_ty`; an `if`'s test narrows in both branches via [`Ty::tested_by`]
  (`(if (int? x) … …)` ⇒ in the *then* branch `x` is `int`, in the *else* it's
  `not int`); `(not <inner>)` flips. Inner shadowing overrides — a fresh
  binding to an unknown RHS *removes* an outer narrowing rather than
  intersecting (otherwise the outer leaks through the shadow).
- ✅ **Let-bound guard aliases.** `(let (cond (int? x)) (if cond …))` now
  narrows `x` (not the bool `cond`) inside the if. The `Ctx` carries a second
  table `guards: sym → (var, asserted-ty)`; a `let` records the alias when
  the RHS is itself a recognised guard, and `guard_assertion` on a bare `Sym`
  test looks it up. Sound because Brood is immutable — between the let and
  the if neither `x` nor `cond` can change. Self-aliasing (`(let (x (int? x))
  …)`) is rejected (the outer `x` is shadowed). 20 new tests in
  `types::check::tests`. Cond-/match-/and-/or-chained guards (which expand to
  nested `(let g (if g …))` whose `g` aliases the *combined* test, not a
  single variable) are still deferred — they'd need either pre-expansion
  handling or post-expansion shape recognition.
- ⬜ **next:** unbound-symbol and arity diagnostics; running automatically in
  `brood <file>` / `nest test` / `nest check`.

### Step 5+ — structured types ⬜
Function arrows, vector/list element types, intersections for overloaded fns —
the fuller set-theoretic algebra. Additive; gated on real need (ADR-011). **Note:
this *replaces* the `u16`-bitset representation of `Ty`** (likely an
`enum { Set(u16), Arrow(..), Vec(elem), … }`), it doesn't extend it — which is a
reason to keep the flat lattice lean now rather than over-build on the bitset.

## How it runs — and why it's outside the runtime

The checker is a **pre-step at the file/project boundary**, never woven into
evaluation:

- `brood check <file>` — check a single file (the language binary).
- `nest check` — check the whole project (the CI / editor entry point).
- `brood <file>` — check, then run a file.
- `nest test` — check the project, then run the tests.
- **Not** in the REPL / `load` / per-form `eval` (maybe later) — so there's no
  per-eval noise and no suppression machinery beyond the `try`/`error-of` skip.

**Checking is upstream of hot reload, never part of it.** "Don't reload code we
can already see will fail" is a property of the *workflow that orchestrates the
reload* — today: run `brood check` first; later: the editor's reload command
(itself Brood) checks, then reloads — **not** of the `def`/reload primitive. The
runtime never consults the checker, so: contract #5 holds with **no carve-out**,
there is nothing to override, and a wrong signature can at worst print a stray
warning upstream — it can never wedge a reload. (Reloads should be *atomic* —
broken new code leaves the running version in place — but that's hot-reload
hygiene, independent of the type checker.) The type system stays **entirely
external**: it observes and advises; it is never in the execution or reload path.

Why no inference *engine* (ADR-011): full body inference needs control-flow /
dominance analysis to avoid false positives from *guarded* uses (a param used as
a number only inside `(if (number? x) …)` doesn't make the param a number). That
machinery is the bulk of the complexity and the only false-positive source — so
we cut it, keeping curated sigs + the trivially-sound straight-line case, and add
more only when a concrete need justifies it.

## Compatibility contract

Every change — new primitive, new special form, new `Value` kind, new feature —
must keep these true, so future work stays on the set-theoretic path. Items
marked **(enforced)** are compile errors if violated; the rest are review rules.

1. **Every value has exactly one tag.** The `Tag`s are the type atoms, and a
   tag's `#[repr(u8)]` discriminant *is* its lattice bit. A new `Value` variant
   must get a `Tag` (in `value::tag`, **enforced** — exhaustive match) and be
   added to `types::ALL_TAGS`; `TAG_COUNT`/`UNIVERSE` then follow automatically.
   The `tag_universe_is_consistent` test checks bits are dense and in order, so a
   tag *missing from* or *misordered in* `ALL_TAGS` fails CI (the gap a plain
   match can't catch, since Rust can't enumerate variants). Don't introduce a
   value kind that can't be a tag.
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
6. **Every primitive declares its type. (enforced)** A new builtin supplies a
   result `Ty` (+ arg `Ty`s) next to its `Arity` — `NativeFn` carries a `Sig`
   field, the same mechanism that makes `Arity` mandatory: omitting it is a
   compile error. The "no useful info" case uses `Sig::any()` (overlaps every
   input, never warns), so the contract holds for permissive builtins too.
7. **Policy in Brood.** If a type test or contract can be written in Brood over
   `type-of`/predicates, it goes in `std/`, not Rust (ADR-006).
8. **Pattern/guard forms expose their refinement.** New pattern kinds or guards
   must remain analysable for occurrence typing — the matcher is the inference
   goldmine (step 4). Don't add opaque guards that hide the type they imply.
9. **Errors, `type-of`, and `Ty` agree on names.** All use `Tag::name`
   spellings, so a `Ty` in a message reads the same as `type-of` returns.

## Where it lives

(After the `core/` / `syntax/` / `eval/` / `types/` module split.)

- `crates/lisp/src/types/mod.rs` — the `Ty` lattice (step 1), `GradualTy`
  (step 2), and `tested_by` (the guard-narrowing bridge for step 4).
- `crates/lisp/src/types/check.rs` — the advisory checker: the signature sources
  (step 3) and the disjointness walk (step 4).
- `crates/lisp/src/core/value.rs` — `Tag` (the atoms), `value::tag`, `NativeFn`
  (gains a signature when step 3's table moves onto it).
- `crates/lisp/src/eval/mod.rs` — `call_native` (the arity gate).
- `crates/lisp/src/eval/macros.rs` — `macroexpand_all`, the pass the checker runs
  after.
