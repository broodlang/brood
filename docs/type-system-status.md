# Type system — status & what's left

**As of 2026-07-30.** A wrap-up of the checker/type-system work, what it now does, and the
honest backlog. For the model and the compatibility contract see [types.md](types.md); for
the "why" of each piece, the ADRs cited below in [decisions.md](decisions.md).

The invariant that governs everything here: **the checker is advisory and sound — it warns
only on a *provable* misuse and never false-positives.** Every item below was landed against
a hard gate: the full `std/` + `tests/` sweep stays clean (zero false positives) and the
`types::check` unit suite stays green.

---

## What we have

### The lattice (`types/mod.rs`)
Set-theoretic + gradual (ADR-023/024). A `Ty` is a set of runtime tags with
union/intersect/negate and *semantic* subtyping; `dynamic()` (`GradualTy`) is the valve for
redefinable globals. Refinements on the flat lattice: function **arrows**, sequence **element
types**, **`(map K V)`**, **record shapes**, **literal singletons** (keyword/int/bool/string),
**tuples**, and the named unions `number` / `list` / **`seqable`**.

### Signatures the checker reads (`types/check/sigs.rs`)
Simplest-first: **primitives** (every `NativeFn` carries a `Sig`), **curated** stdlib (a
hand-vetted table for variadic/HOF closures), and **inference**. Inference is now broad and
sound:
- **Params** from *unconditional* type demands (a guarded use never constrains a param).
- **Returns** from the body tail via `expr_ty`, unioning `if`/`cond`/`let`/`do`/`case`.
- **Recursion** — a self-recursive branch contributes ⊥, so a tail-recursive `--acc`/`--loop`
  infers from its base cases (ADR-188 devlog).
- **Complex closures** — multi-arity / `&optional` / rest get a params-less return-only sig
  (union of arm tails); arity is checked independently.
- **Same-file functions** (ADR-188) — `check_file` Pass 2.8 infers a file's own `defn`s from
  their *forms* (the file isn't loaded while checked) over a bounded leaf-up **fixpoint**, so
  same-file callers finally get checked.

### Typed abilities (ADR-180/181/186)
- Op specs carry types: `(area [self (factor float)] :-> float)`. The **return** flows into
  inference at every call site; **impl bodies** are checked against it; **arguments** at typed
  params are checked. `:-> any` / bare params impose nothing.
- **Any ability name is a type**: a `:sealed` ability → the finite union of its members'
  record shapes; an **open** ability → the permissive `any` (so a `sig` using it survives; the
  real "does this value implement it" safety is the op-call missing-impl check).
- Ability call-site checks: missing-impl warnings (literal / ctor / inferred-variable args),
  sealed exhaustiveness, per-module op-name uniqueness.
- Record patterns in `match` and sealed-`match` exhaustiveness (ADR-187, `check/exhaustive.rs`).
- Provided (default) op bodies (ADR-185).

### Devirtualization (ADR-182) — `BROOD_MONO`, **off by default**
Tier 1 rewrites an ability op call with a literal or direct-record-constructor first arg to a
direct impl call (skips `identity-of`/`impl-for`). Flag-off is provably inert; flag-on is
byte-identical + GC-safe. Benchmark: ~5.7× on literal dispatch, ~1.8× on constructor dispatch
(microbenchmark — Tier 1 doesn't move the standard rows, whose hot loops pass *variables*).

### Reach — the checker runs everywhere
- **Batch:** `nest check` / `nest test` / `nest run` / `brood <file>` / `brood --check`.
- **MCP:** `load` / `check` return structured diagnostics.
- **LSP:** editor diagnostics (syntactic **+** semantic `check_file`); **hover** shows a
  resolvable name's type signature.
- **REPL:** advisory warnings before each result, using live-image inference (every def is
  loaded there, so inference is at its most complete).
- **Reachability (ADR-189):** `nest check` flags a qualified `mod/name` whose module the file
  never (transitively) requires — the KI-17 load-order-luck bug — with zero false positives.

---

## What's left

Ordered roughly by value. None of these are blockers; each is additive or a
pay-when-it-hurts item, and the guiding rule (ADR-011) is to defer until a concrete need
appears.

### Inference — mostly closed
- ✅ **Same-file *parameter* inference** — shipped (ADR-190, occurrence typing): a caller's
  arguments are checked against a derived param type, no annotation needed.
- ✅ **`and`/`or`-chained guard narrowing** — shipped: every `and` conjunct narrows the
  then-branch; a same-variable `or` narrows both branches (then → union, else → complement).
- ✅ **Higher-order callback result inference** — already worked (`(map f xs)` flows `f`'s
  element type; `(string-length (first (map inc xs)))` flags).
- ⏸ **Per-arm multi-arity params** — the one remaining piece. A multi-arity closure still gets
  a params-less return-only sig, so a call's args aren't checked against the matching arm.
  Sound to leave (a missed check is a false negative, never a false positive); closing it needs
  an inferred-overload path + per-argc arm selection in the call-check, for marginal value
  (ADR-011).

### Abilities / dispatch
- **Return-type dispatch** — selecting an impl by expected return; needs bidirectional
  inference. The long-standing open item in [protocol-dispatch-design.md](protocol-dispatch-design.md).
- **Qualified cross-module ability type names** (`mod/Ability` in a `sig`) — today ability
  type names resolve by bare name.
- **Tier-2 monomorphization** — devirtualizing an *inferred-variable* op call
  (`(map area shapes)`), the real hot-loop win. It's the miscompile surface; needs the
  checker→compiler channel + whole-fleet validation. See
  [ability-monomorphization.md](ability-monomorphization.md).

### Runtime
- **Runtime contracts for ability ops** under a `BROOD_CONTRACTS`-style flag (ADR-180 deferred
  item c) — checker-only today, matching `sig`'s default.

### Checker correctness
- **KI-17 — FIXED (ADR-189).** `nest check` now flags a user-written qualified `mod/name`
  whose module the file doesn't transitively require (it resolved only by load-order luck).
  The whole-project driver builds each file's transitive require-closure and threads it to
  `check-file`; zero false positives across `std/` + `tests/`. See
  [ADR-189](decisions.md) / [known-issues.md](known-issues.md).

### Tooling
- **LSP hover / inlay inferred types for *buffer* functions.** Hover shows types only for
  *loaded* names today; a buffer's own edited functions aren't loaded (and hover must not eval
  them), so their inferred types aren't shown. Needs a from-CST inference path or an isolated
  scratch load.
- **`sig` adoption across std.** Only ~1% of std's 2160 defns carry a hand-written `sig`
  (path, set, json, plus the file/fuzzy pilot). More coverage = more caller checking, and it
  dogfoods the type system — but "better inference" is the higher-leverage alternative that
  scales without manual annotation.

### Precision residues (sound to leave)
- The **merely-wider residue** — a body typed exactly `number` declared `int` (e.g.
  `(/ x 2)`): pinning it needs occurrence/range analysis; flagging it would false-positive, so
  it stays deferred (ADR-011).
- **Element-typed `(seq T)`** — `seqable` is unrefined today; a genuine element-typed seqable
  needs extending the `elem` refinement beyond `Pair|Vector`.

---

## Where things live

| Concern | File |
|---|---|
| Lattice (`Ty`, ops, named unions) | `crates/lisp/src/types/mod.rs` |
| Checker entry + passes (`check_file`) | `crates/lisp/src/types/check.rs` |
| Signature sources + inference | `crates/lisp/src/types/check/sigs.rs`, `infer.rs` |
| Type-annotation grammar (`sig`, `seqable`, ability-as-type) | `crates/lisp/src/types/check/annot.rs` |
| Ability/multimethod checks | `crates/lisp/src/types/check/protocol.rs` |
| Record-pattern exhaustiveness | `crates/lisp/src/types/check/exhaustive.rs` |
| Devirtualization (`BROOD_MONO`) | `crates/lisp/src/eval/compile/inline.rs` |
| REPL advisory check | `std/tool/repl.blsp` |
| LSP hover / diagnostics | `crates/lsp/src/{hover,main}.rs` |

ADRs: **180** typed op returns/params · **181** sealed ability as a type · **182** mono
devirtualization · **185** provided op bodies · **186** any ability name is a type · **187**
record patterns + exhaustiveness · **188** same-file inference · **189** per-file
require-reachability lint (KI-17) · **190** occurrence typing (inferred params check callers)
· **191** staged call head (KI-19). Chained-guard `and`/`or` narrowing shipped 2026-07-30
(no separate ADR — it closes the ADR-011-deferred inference gaps).
