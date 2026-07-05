# Whole-program soundness under hot reload — designed, not built (ADR-123)

**Status:** design only, no runtime code. Recorded so the hard parts (the
reload-conflict resolution, the dependency-tracking mechanism, where a hard
gate can and can't live) are on paper before implementation starts, per
ADR-011 — gated on this actually being picked up as the next slice of type-
system work, not built speculatively.

## The problem

`docs/roadmap.md`'s type-system section previously framed whole-program static
soundness — gating on global `def`/`defn` types, not just local bindings — as
something Brood **won't** pursue, because it looked like it must conflict with
Erlang-style hot reload (ADR-013: a `def` rebinds a global at runtime,
unconditionally, and every process sharing that runtime's code region sees it
on its next lookup). That framing is being revised: the goal is now full
Elixir-parity soundness. This doc works out how that's actually compatible with
reload, rather than declaring the two incompatible by assumption.

## The load-bearing fact that makes this tractable

**Runtime type safety in Brood is already 100% independent of the static
checker.** Confirmed by tracing the compiler/JIT: every `Value` carries a
runtime `Tag`, and every operation — arithmetic, calls, field access, even the
JIT's unboxed fast paths — does a real runtime tag check before proceeding.
`crates/lisp/src/lib.rs` labels the `types` module "the advisory type lattice +
checker (nothing gates on it)" for a reason: `types/check.rs` and
`eval/compile.rs` are fully separate pipelines that both just consume the same
AST. There is no code path where a statically-proved type causes a runtime
check to be skipped.

**Consequence:** a hot reload that invalidates a prior static proof cannot
crash the process or corrupt memory. Worst case, some caller now hits a clean,
catchable runtime type error at the point of actual misuse — exactly the same
class of error Brood already has for any dynamically-typed mismatch, reload or
not. So "soundness" here is a claim about the checker's *static* guarantee
staying valid, not a memory-safety property that needs protecting. This means
we're free to choose *when* and *how hard* to enforce it without inventing new
runtime machinery — the runtime was never the unsound part.

## Why a hard reject-on-reload is the wrong mechanism anyway

Even granting we want whole-program soundness, blocking a `def` when it breaks
some caller's prior proof would fight the project's actual reason to exist: a
self-editing, live-reloadable image where you routinely have inconsistent
intermediate states while fixing something. Erlang and Common Lisp images
don't gate redefinition on whether every existing caller remains statically
valid, and for the same reason — the redefinition often *is* the fix for a
caller that was wrong.

There's also no natural place to put the gate: `def` in the shared-code-region
model (ADR-013) is a single promote-and-rebind, visible to every process
sharing that runtime on its next lookup. A hard gate would need to (a) know
every live caller across every process sharing the runtime, and (b) decide
what happens to in-flight calls that already captured the old assumption —
both of which the append-only, no-rollback code region isn't designed for.

## The design: soundness re-asserted per reload, not proven once forever

Instead of a permanent whole-program proof, treat soundness as a claim about
**the image's current state**, continuously re-derived as `def`s happen:

1. **Globals get a real, trackable current type**, not a permanent `dynamic()`.
   Seed it from the existing curated/inferred/`(sig …)` sources. Store it
   keyed by the same module-qualified global name `%register-sig` already
   uses.

2. **The checker records a dependency edge whenever a call site is gated using
   a global's type** — `(call-site file:line, global name, the Ty relied
   upon)`. This is new: today's checker only *consumes* global types
   (falling back to `dynamic()`); it needs to also *emit* this edge as a
   by-product of the existing gating passes (`check_if`/`check_call` and
   friends already compute the type they're gating on — this is recording
   that computation, not a new inference). Aggregate into a reverse-dependency
   index: global name → set of dependent call sites.

3. **On every `def` that rebinds a global**, run a **targeted re-check**:
   check the new definition's body as usual, then look up the redefined
   global's dependents and re-verify each recorded assumption against the
   *new* type (an `is_subtype` check — the same machinery narrowing already
   uses). Anything that no longer holds gets a fresh advisory warning. The
   reload still happens unconditionally — this never blocks `def`, it only
   updates what's currently known to be sound.

4. **Where a real hard gate does make sense: batch/CI tooling, not the live
   image.** `nest check`/`nest test` (and a future `nest check --strict` or
   `BROOD_CHECK_STRICT=1`) can treat a nonzero warning count — including these
   reload-dependency warnings — as a failing exit code. That gives genuine
   "reject if it doesn't typecheck" semantics for an automated pipeline,
   exactly where a build has always been allowed to fail, without changing a
   single thing about how the interactive/live image behaves. `(sig! …)`
   remains the opt-in hard runtime gate for anyone who wants an actual
   enforced boundary inside the running image.

This keeps the two failure modes cleanly separated: the **live image never
rejects** (ADR-013 + the "never gates" invariant stay true for anything
running), while **whole-program soundness becomes a real, continuously
tracked, and batch-enforceable property** — which is the part that was
actually missing, not a new kind of runtime restriction.

## What needs building (the hard parts, deferred until picked up)

- **Per-global current-type store.** Promote globals from a hardcoded
  `dynamic()` to a real `Ty`, invalidated and replaced on each `def`. Needs a
  clear answer for what a redefined global's type is *before* its first
  `(sig …)`/curated entry exists (falls back to inferred-from-body, same as
  today's local inference). **Slice shipped (ADR-124):** a declared `(sig x T)`
  value type is now visible cross-module (not just within its own file),
  matching what arrow sigs already had — the precondition for a dependency
  index to mean anything, not the store or the index itself. Still missing:
  an inferred type for a global with *no* declared sig at all.
- **The reverse-dependency index.** Built as a by-product of the existing
  check passes; needs to not regress `check-file`'s cost — this is exactly the
  kind of fingerprinting ADR-119's Phase 2 cache already has to solve
  (`hash(content) + hash(deps)`), so the two designs should share the
  dependency-fingerprint mechanism rather than inventing two.
- **The reload hook.** Where `def` currently promotes + rebinds
  (`crates/lisp/src/eval/mod.rs`/wherever the special form lives), fire the
  targeted re-check. Must be cheap enough not to penalize a hot loop that
  `def`s internally at runtime outside of live-editing (rare, but exists) —
  likely gated to only run when a checker/LSP session is actually attached,
  not unconditionally on every `def` call.
- **Invalidation rule precision.** Reuse `is_subtype` for "does the new type
  still satisfy the old assumption" — needs care around narrowed/refined types
  (a dependent might have relied on a *refinement*, e.g. a literal singleton,
  not just the base tag).
- **Surfacing.** Where do the fresh warnings show up — LSP push diagnostics,
  a `nest run --watch` overlay, a REPL message on `def`? Needs a decision
  before implementation; likely all three consume the same underlying event.
- **Interaction with ADR-013's per-runtime scope.** Re-check fires once per
  `def` on the shared runtime image, not once per process sharing it —
  dependency data is a property of the runtime's code region, not any one
  process.

## Relation to sibling designs

[`incremental-check.md`](incremental-check.md) (ADR-119) already needs a
dependency fingerprint for its Phase 2 cache invalidation, for an unrelated
reason (skipping re-check of unchanged files). That fingerprint and this
design's reverse-dependency index are the same underlying data — building one
should build both, whichever comes first.

## Alternatives rejected

- **Hard reject the reload** if any dependent breaks — fights the live-image
  premise and has no natural implementation point in the shared-code-region
  model (see above).
- **Restrict what a reload may change** (e.g. widen-only global types) to keep
  prior proofs permanently valid — considered, but it constrains hot reload's
  actual use (a bugfix often *narrows* or *changes the shape* of what a
  function returns) more than the re-check approach costs, for no safety
  benefit (per the load-bearing fact above, nothing crashes either way).
- **Do nothing / leave globals `dynamic()` forever** — the status quo; this is
  what's being revised, not what's being chosen.
