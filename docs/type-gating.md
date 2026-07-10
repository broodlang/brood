# Gating: full gradual consistency in the checker's decisions (design)

**Status:** ✅ **shipped** — Gap A (same-file + cross-file) and Gap B (B0 + B1)
are all built, tested, and verified end-to-end (2026-07-10). This doc now reads
as the as-built design; the only follow-on left is the reload *re-check trigger*
beyond `nest run --watch` (REPL/LSP push), tracked in the reload-soundness doc.
Scopes what was the roadmap's 🎯 item — "wiring
`dynamic()` / full gradual consistency into the checker … actual gating decisions
(not just advisory assignment checks)." Companion to
[`type-soundness-reload.md`](type-soundness-reload.md) (ADR-123/124/125/126),
which already shipped the *workflow* half (re-check on reload, the CI hard gate).
This doc is about the checker's *internal decision logic*, not the reload loop.

## What "gating" means now

"Gating" here is **not** blocking a `def`/reload — the reload-soundness doc
settled that (the live image never rejects; `nest check` is the batch/CI gate;
runtime tag checks make it all memory-safe regardless). What's left is narrower
and purely about *which warnings the checker is able to emit*: the checker has a
gradual-typing relation (`GradualTy::consistent_with`) that distinguishes

- **precise** types (`stat(t)`) → checked with `⊆` (catches a *merely-wider*
  mismatch, e.g. a `number` where `int` is wanted), from
- **dynamic** types (`dynamic_within(t)`, over-approximations / redefinable
  globals) → checked with `∩ ≠ ⊥` (defers unless *provably* disjoint — never
  over-warns, reload-safe).

Today that relation is consumed in only **three assignment checks** (ADR-110):
`(def x …)` vs a value sig, the body-vs-declared-return check, and declared
globals in value position. The rest of the checker — most importantly the
**call-argument check** — still uses raw `Ty::is_disjoint` (the `∩`-only half).
"Gating" = making the argument/operand decisions use the *full* gradual relation,
and giving globals a real tracked current type to feed it. Two independent gaps,
below, each verified against the current binary.

## Current state (verified, not assumed)

| Decision point | Declared global (`(sig g T)`) | Undeclared global (`(def g 5)`) | Merely-wider precise (`number` arg → `int` param) |
|---|---|---|---|
| `(def r (…g…))` value position | ✅ warns | ⛔ deferred | — |
| Call argument `(f g)` / `(f x)` | ✅ warns (∩) | ⛔ deferred | ⛔ **not caught** |
| Body vs declared return | ✅ (⊆, gradual) | — | ✅ caught (⊆) |

The return check catches merely-wider (it runs the gradual relation with `⊆` for
precise types); the **argument check does not** — it only fires on provable
disjointness. That asymmetry is Gap B. Gap A is the empty middle column.

## Gap A — a current type for an *undeclared* global

**Status: shipped.** `check_file` Pass 2.7 infers a current-image value type for an
undeclared global defined **exactly once** by `(def g <non-fn-expr>)` — the RHS's
`expr_ty` — and records it in `Ctx::inferred_value_ty`. `expr_ty` (the arg check)
and `gradual_of` (value/return checks) consult it after the declared value type,
always as `dynamic_within` (the `∩` relation → reload-safe, warns only on provable
disjointness). Same-file is conservative: defined-exactly-once (a redefined global
is ambiguous → stays `dynamic()`), non-function values.

**Cross-file: also shipped** — and it needed no new store or pre-pass. An
undeclared global used in *another* file is typed from its **current heap value**
(`global_value_ty` → `Ty::of_value(obs_global …)`), the exact mechanism
`infer_sig` already uses for functions: the image is loaded before checking, so
the value is in hand, and `obs_global` records the dependency so a change
re-checks the reader (ADR-125). Consulted last, after the declared and
same-file-inferred types, always as `dynamic_within`. It **excludes dynamic
variables** (`defdyn`): their heap value is only the default, but `binding`
rebinds them to any type in a dynamic extent, so typing a use against the default
would be unsound — this exclusion also closed a latent hole in the same-file path.
It shares `infer_sig`'s one narrow, *pre-existing* false-positive class (a
top-level use that ran at load before a same-name redefinition), which the project
already accepts for functions; it introduces nothing new. Verified: a `(def g 5)`
in the image with `(string-length g)` elsewhere warns; a dynvar used
polymorphically via `binding` does not; the whole corpus (every cross-module
reference) stays at zero warnings.

Today a global with no `(sig …)` is `dynamic()` (bound `ANY`), so every use of it
defers. The reload-soundness doc's "Step 1" (globals get a real current type)
shipped only for *declared* value sigs (ADR-124); an **inferred** type for an
undeclared global is the explicitly-noted remainder.

**Proposal.** Track each global's current type from its definition, seeded by the
sources we already have:
- `(def g <literal-or-precise-expr>)` → `expr_ty` of the RHS (e.g. `(def g 5)` →
  `int`).
- `(defn g …)` → the inferred sig (now including the return-only tier).

Crucially, expose it to the checker as **`dynamic_within(inferred)`, never a
precise `stat`** — an undeclared global has no author-asserted contract and *is*
redefinable, so its inferred type is a current-state observation, not a promise.
The `dynamic` flag means every decision uses `∩` (defer unless provably disjoint),
so:
- `(def g 5)` then `(string-length g)` → `int ∩ string = ⊥` → **warns** (a real
  misuse of the current image);
- if `g` is later redefined to a string, the reload re-check (ADR-125) re-derives
  and the warning clears — no stale hard proof.

**Soundness.** Sound for the current image state, which is exactly the
reload-soundness model. The `dynamic`/`∩` treatment is what keeps it
false-positive-free: it can only fire on a *provably* disjoint use, never on a
merely-plausible-redefinition. (Contrast: treating an undeclared global as a
precise `stat` type and using `⊆` **would** be unsound — a redefinition the author
intends would be flagged. We must not do that.)

**Open sub-question — mid-file redefinition.** A global's "current type" is the
*most recent* `def` before a use; the checker walks a file top-to-bottom and
`ctx.file_globals` tracks names, not evolving types. Options: (i) last-def-wins
within a file (track the type alongside the name, update on each `def`); (ii)
union of all defs in the file (coarser, always sound); (iii) only track globals
defined exactly once in the file (simplest, covers the common case). Recommend
(iii) first — a redefined-in-same-file global is rare and can stay `dynamic()`.

## Gap B — the argument check should use the full gradual relation

The call-argument check (`walk.rs`, the `is_disjoint` loop) fires only when an
argument is *provably disjoint* from the parameter. It should instead use
`GradualTy::consistent_with`, exactly as the return check does:
- a **precise** argument (a literal, a `(sig …)`-typed param, integer-closed
  arithmetic) → `⊆`, so a *merely-wider* precise argument is caught
  (`number` sig-param passed where `int` is wanted);
- a **dynamic** argument (a call result, an inferred/undeclared global) → `∩`,
  unchanged from today — no new over-warning.

This is the same `gradual_of` machinery the return check already uses; the work is
routing the argument through it instead of through bare `expr_ty` + `is_disjoint`.

**Soundness.** For the *dynamic* branch it's identical to today (`∩ = ⊥` ⇔
`is_disjoint`). The only behavior change is the *precise* branch's `⊆`, which can
newly warn on a merely-wider **precise** argument — the same judgment the return
side already accepts for sig-typed values
(`wider_sig_param_returned_as_narrower_is_flagged`).

### Prerequisite (discovered by prototyping — Gap B is blocked on it)

Prototyping Gap B surfaced a hard prerequisite: **it is unsound without
int/bool/string literal-singleton precision.** `Ty::of_value` gives an int/bool/
string literal the *flat* `int`/`bool`/`string` tag (only keywords carry a
singleton), so a literal argument's static type is an **over-approximation** of
its value. Two failures, one root cause:

- A literal `200` passed where `(or 200 404 500)` is wanted: `gradual_of(200) =
  stat(int)`, and `int ⊄ {200,404,500}` → **false positive**, even though the
  value `200` is in the set. (`∩` doesn't misfire — `int ∩ {200,404,500} ≠ ⊥` —
  which is exactly why the current arg check is on `∩`.)
- "Fixing" that by making literals `dynamic` (`∩`) then loses a *real* catch: a
  partial-overlap union like `(if (> x 0) x "neg")` declared `int` yields
  `int | string`, which `⊆ int` flags (correct — the body can return a string)
  but `∩ int ≠ ⊥` misses (the `int` arm overlaps). The string *literal* branch
  going dynamic contaminates the union to dynamic.

These conflict irreconcilably: `⊆` needs the literal to be *faithful*
(`{200}`, not `int`), and faithfulness for int/bool/string literals is precisely
the **argument-literal-precision** feature (`docs/type-int-literals.md`,
`type-bool-string-literals.md`) that was *tried and reverted*. So Gap B's real
shape is a two-parter: **(B0) track int/bool/string literal singletons** (make
`Ty::of_value` and the value-position typing carry `{200}` not `int`), *then*
(B1) route the argument check through the gradual relation. B1 alone
false-positives; B0 alone adds precision the checker already knows how to carry.

**Status: B0 + B1 shipped — Gap B complete.** B0: `Ty::of_value` carries int/bool
singletons and `expr_ty` builds a string's `str_lit` (it has the heap), so a
literal's static type is faithful (`200 : {200}`, a subtype of `int`) — removing
the FP at its root (`stat({200}) ⊆ (or 200 404 500)` now holds) and sharpening
every literal diagnostic (`got 5`, not `got int`). B1: the argument check now runs
the same `gradual_of` / `consistent_with` the return check uses — `⊆` for a
precise argument (so a merely-wider `number` passed where `int` is wanted is
caught), `∩` (`!is_disjoint`) for a dynamic one (a call result / redefinable
global — no new over-warning, reload-safe). Two supporting fixes shipped with B1:
`gradual_of` consults a narrowing on *any* symbol (not just lexical locals, so a
guard-narrowed free variable keeps it), and `consistent_with`'s dynamic branch
uses `is_disjoint` (refinement-aware — catches record/tuple/literal-set
conflicts). Verified: `types::` 371/371, `nest check` 0 (no merely-wider false
positive in the corpus).

## Recommended sequencing (revised after prototyping)

1. **B0 — int/bool/string literal-singleton precision** — ✅ **shipped**.
   `Ty::of_value` carries int/bool singletons; `expr_ty` builds the string
   `str_lit`. Sharpens the existing return/def checks (removes the latent
   literal-set FP) and makes literal diagnostics name the value. ≈19 test message
   strings updated. `types::` 370/370, `nest check` 0.
2. **B1 — arg-check → gradual relation.** ✅ **shipped** (after B0). The `⊆`
   upgrade is sound (a literal is a faithful singleton, so `⊆` no longer
   over-approximates), and it closes the return/argument asymmetry. Verified zero
   corpus warnings.
3. **Gap A — undeclared-global current type as `dynamic_within`** — ✅ **shipped**
   (defined-exactly-once, same-file, non-function value globals). Independent of
   B0/B1; sound on its own (the `∩` relation). ✅ **Cross-file too** — typed from
   the loaded image's heap value (`global_value_ty`), like `infer_sig` for
   functions; dynamic variables excluded.
4. **Re-check coverage.** All of the above rely on ADR-125's reload re-check to
   re-derive after a `def`; that trigger is already shipped for `nest run
   --watch`. REPL-level and LSP-push triggers stay open (noted in the reload
   doc), not blocking.

Each step is independently shippable and independently corpus-verified. The
prototyping already done establishes that **B1 without B0 is not sound** — that's
the main design result here.

## The invariant this revises

`CLAUDE.md` and `docs/types.md` contract #5 state "checking never rejects a
runnable program." That stays literally true — nothing here gates the live image;
`nest check`'s nonzero exit is the only "reject," and only in batch/CI. But the
*spirit* tightens: the checker will now emit a warning on a **merely-wider precise
misuse** (Gap B) and on a **provably-disjoint use of an undeclared global's
current-image type** (Gap A). Neither is a false positive under the
reload-soundness model — the first is an author-contract tension, the second is a
real misuse of the current image that a reload re-checks. ✅ **Done:** contract #5
in [`types.md`](types.md) (and the matching invariant in `CLAUDE.md`) now read
*the checker never gates the live image, and never warns on a use that is valid
for the image's current state* — the precise, reload-aware form of the old
"runnable program" phrasing.

## Explicitly out of scope

- **Parameter inference across branches** (the unsound-without-occurrence-typing
  half of local inference) — orthogonal; gating consumes whatever types inference
  produces, it doesn't need more of them.
- **Hard-gating the live image / blocking a `def`** — rejected in the
  reload-soundness doc, unchanged.
- **A precise (`stat`) type for an undeclared global** — unsound under
  redefinition; undeclared globals stay `dynamic_within`.
