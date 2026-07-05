# Whole-program soundness under hot reload (ADR-123/124/125)

**Status:** the core mechanism is **shipped**. ADR-124 gave globals a real,
cross-module-visible current type; the planned reverse-dependency index
turned out to already exist (ADR-119 Phase 2, built the same day for an
unrelated reason); ADR-125 shipped the live-session trigger
(`nest run --watch` re-checks on every successful reload). Only the optional
batch/CI hard-gate (`nest check --strict`) remains unbuilt, and nothing
currently depends on it.

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
   uses. **Shipped (ADR-124):** declared value-type sigs are now visible
   cross-module via the heap-wide store, matching what arrow sigs already had.

2. ~~The checker records a dependency edge whenever a call site is gated using
   a global's type, aggregated into a reverse-dependency index: global name →
   set of dependent call sites.~~ **Superseded — this infrastructure already
   exists, built for an unrelated reason.** ADR-119 Phase 2 (the incremental
   `nest check` cache, shipped the same day as this doc) instruments every
   sanctioned read of global state (`types/check/deps.rs`'s `obs_*` wrappers)
   to record, per file, exactly the set of external facts its check depended
   on (`check-file-deps` → `[warnings dep-keys fingerprint]`). Critically, it
   does this **without ever building a persisted reverse index** — invalidation
   is *pull*, not *push*: each cached file's fingerprint is cheaply
   *re-observed* against the current image (`check-deps-fp`) and compared to
   what was stored; a mismatch means "something this file depended on
   changed," with no need to know *what* changed or maintain a `global →
   dependents` map at all. That's a strictly simpler solution to the same
   problem Step 2 was designed to solve — so there is no separate index left
   to build here.

3. **On every `def` that rebinds a global, re-run the existing cheap
   check.** Instead of a bespoke reload hook that walks a dependency index,
   the re-check is just: invoke the same `check-file-deps`/`check-deps-fp`
   pair Phase 2 already ships, on whichever files are in scope for the
   current session. The reload still happens unconditionally regardless of
   the result — this never blocks `def`, it only updates what's currently
   known to be sound. **What's actually still open** is not a data structure
   but a *trigger*: Phase 2's cache today is consulted only by the batch
   `nest check` CLI tool, never by a running REPL/eval session, so there's no
   live analogue of "a `def` just happened, re-check whoever depended on it"
   yet. The open design question is where that trigger lives — on file save
   (`nest run --watch` already watches files), on every `def` in a REPL
   session (finer-grained, more expensive), or purely LSP-driven (the editor
   asks for fresh diagnostics on its own schedule, and the answer happens to
   already be fast because of Phase 2). No decision made yet; this is now the
   entire remaining scope of ADR-123's "hard part."

4. **Where a real hard gate does make sense: batch/CI tooling, not the live
   image.** `nest check`/`nest test` (and a future `nest check --strict` or
   `BROOD_CHECK_STRICT=1`) can treat a nonzero warning count as a failing exit
   code. That gives genuine "reject if it doesn't typecheck" semantics for an
   automated pipeline, exactly where a build has always been allowed to fail,
   without changing a single thing about how the interactive/live image
   behaves. `(sig! …)` remains the opt-in hard runtime gate for anyone who
   wants an actual enforced boundary inside the running image.

This keeps the two failure modes cleanly separated: the **live image never
rejects** (ADR-013 + the "never gates" invariant stay true for anything
running), while **whole-program soundness becomes a real, continuously
tracked, and batch-enforceable property** — which is the part that was
actually missing, not a new kind of runtime restriction. The pleasant
surprise: almost none of the mechanism needs to be *built* — it needs to be
*triggered* at the right moment, reusing what ADR-119 Phase 2 already ships.

## What needs building (revised — much smaller than originally scoped)

- **Per-global current-type store.** Promote globals from a hardcoded
  `dynamic()` to a real `Ty`, invalidated and replaced on each `def`. **Slice
  shipped (ADR-124):** a declared `(sig x T)` value type is now visible
  cross-module (not just within its own file), matching what arrow sigs
  already had. Still missing: an inferred type for a global with *no*
  declared sig at all (a separate, harder problem — see the Elixir-parity
  gap list's "full type inference" item; not blocking this design).
- ~~The reverse-dependency index~~ — **not needed.** ADR-119 Phase 2 already
  ships the equivalent capability (`check-file-deps`/`check-deps-fp`) via a
  pull-based re-fingerprint check instead of a maintained push-based index.
  Nothing left to build here; see the design section above.
- **The trigger — shipped (ADR-125).** Went with file-save via `nest run
  --watch`'s existing watcher: `std/tool/reload.blsp`'s `reload-on-change`
  now takes an optional `on-reload` callback, invoked after every successful
  reload with its own errors caught (never takes the watcher down). `nest
  run --watch` supplies `(fn (_p) (project/check-project-sources))` inside a
  project. A REPL-level hook on bare `def` and a purely LSP-driven trigger
  are both still open for later, but neither is needed now that the
  file-save path covers the common `nest run --watch` dev loop. Verified
  end-to-end: a live edit introducing a real type mismatch surfaced the
  warning in the running session's output with no restart, and fixing it
  cleared the warning on the next reload.
- **Surfacing — resolved for this trigger.** Warnings print to the
  `nest run --watch` session's own stderr, the same place its startup
  pre-flight already prints them. LSP push diagnostics and a REPL message on
  `def` remain open for whoever picks up those triggers.
- **Invalidation precision, if it turns out to matter.** Phase 2's
  fingerprint is coarse (a referenced global's *defining file's mtime* plus
  its declared-sig hash — file-level, not per-refinement). This is
  sufficient for "should this file be re-checked at all," which is all a
  trigger needs; it does not distinguish *which* refinement of a global's
  type a given call site relied on. Not a gap unless a future consumer needs
  finer-than-file granularity.

## Relation to sibling designs

[`incremental-check.md`](incremental-check.md) (ADR-119) isn't just a sibling
design that happens to share a fingerprint mechanism — it **is** the
mechanism this design needs. ADR-123's originally-planned "reverse-dependency
index" (Step 2, above) is fully superseded by Phase 2's `check-file-deps`/
`check-deps-fp` pair, built for an unrelated reason (skipping re-check of
unchanged files in the batch CLI) but structurally identical to what
reload-soundness needs. There is nothing independent left to design or build
on that front; the only open work is choosing and wiring a trigger for a live
session, per above.

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
