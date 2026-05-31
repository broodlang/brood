# RUNTIME-region collection — exploration notes

> Status: **exploration / design assessment** (2026-05-31, branch `runtime-gc`).
> Not an implementation. Builds on `live-editing.md` §"Stage 5 — Bounded RUNTIME
> memory", which decided to *defer* the collector behind a quantified leak +
> dedup. This doc grounds that decision against the current code, **quantifies the
> leak empirically**, inventories the roots a collector must trace (including one
> the Stage-5 note predates — the VM's per-process `vm_cache`), and recommends a
> safe incremental path.

## The problem (recap)

The shared RUNTIME code region (`RuntimeCode.code`, `heap.rs`) is an **append-only**
set of `boxcar::Vec` slabs (pairs/vectors/maps/strings/ropes/closures/envs).
Append-only is *why* reads are lock-free and return stable references — process
threads dereference closure bodies without locking while another process `def`s.
Every `def`/hot-reload `promote`s new code in; the superseded version is **never
freed**. The `closures_structurally_equal` dedup (ADR-042) skips a re-append when
the new code is identical to the current binding (save-without-change / formatter
churn), but any *real* edit accumulates.

## Leak, quantified (this exploration)

Redefining a small fn `(fn (x) (+ (* x i) i) (- x i))` N times, each body
structurally distinct (so dedup can't skip), max RSS:

| redefs | max RSS | wall |
|---|---|---|
| 1,000 | 9.3 MB | 0.01 s |
| 20,000 | 28.2 MB | 0.12 s |

≈ **1 KB per redefinition** of a small fn (bigger bodies / nested closures: a few
KB). So ~19 MB / 20k redefs — matching the Stage-5 estimate. Negligible for a
normal session; **real for a multi-day server** (an editor-as-server hot-reloading
across many connections) — single-day heavy editing is tens of MB, multi-day could
reach hundreds.

## The two shapes (Stage 5), assessed against the code

1. **Free-list slab** — reclaim individual dead cells. Abandons `boxcar`'s
   lock-free stable refs: a global read happens on *every* operator/prelude call,
   so adding locking or epoch-protection to RUNTIME reads regresses the hottest
   path in the system. **Bad trade.** Rejected.

2. **Compacting copy at a runtime-wide safepoint** (favored) — trace live RUNTIME
   code, copy it to a fresh region, rewrite every handle, swap. Preserves lock-free
   reads *between* collections. This is a **moving GC over shared, cross-process
   state**, and needs four things, three of which don't exist yet:
   - **(a) Runtime-wide stop-the-world.** *None exists* — the GC (ADR-035) is
     strictly per-process; processes are pinned to worker threads and coordinate
     only via per-worker queues. A RUNTIME collection must pause **every** process
     of the runtime at a safepoint at once (signal all workers → each brings its
     current coroutine to a safepoint and parks → confirm all parked → collect →
     resume). New concurrency machinery — and concurrency is exactly where this
     codebase has had subtle races (`docs/claude-demo-findings.md`).
   - **(b) Trace from all roots across all processes** (inventory below).
   - **(c) Rewrite every RUNTIME handle everywhere** — globals + every process's
     roots/stack/heap/envs/mailbox/`vm_cache`. (LOCAL handles carry a generation
     epoch for the per-process moving GC; RUNTIME handles do **not** — they'd need
     a forwarding map keyed by old index.)
   - **(d) Swap the `boxcar` under its `Arc`** — only safe inside the STW pause.

## Root inventory — what a trace must cover

The live set = everything reachable from these, transitively through RUNTIME code:

- **`runtime.globals`** — the current bindings; the primary live roots. (Old
  versions are dead *unless* a process still holds a handle to one.)
- **Per process:** the operand stacks (`roots` + `env_roots`), the LOCAL heap
  (a closure/value that captured a RUNTIME handle — e.g. a closure stored in data
  or sitting in a mailbox), env chains, and **in-flight call frames** (a process
  mid-call to a now-superseded version — append-only is what makes that safe
  today; a collector must keep those versions live).
- **⚠ Per-process `vm_cache` (new — the Stage-5 note predates the VM, ADR-076).**
  `Heap::vm_cache` maps `VmCacheKey` → `Arc<CompiledClosure>`. It references RUNTIME
  code **two ways**: the keys are RUNTIME handles (`Runtime(closure.0)` /
  `LocalBody(body_pair.0)`), and the cached `Node` trees hold RUNTIME `Const`
  handles + the `MakeClosure` arms hold RUNTIME body `Value`s. A RUNTIME collector
  must account for it. **Cheapest correct option: clear every process's `vm_cache`
  during the STW pause** — it's a pure compile cache, rebuilt lazily on next call,
  so dropping it loses only warm-up, not correctness. (Rewriting it would be far
  more work for no benefit.)
- **Not code:** `def_sites` holds `SourceLoc` (file + position), not code handles —
  safe to leave (or rewrite trivially).

## Recommended incremental path

- **Step 0 — quantify (done, above).** ~1 KB/redef; deferral remains justified for
  desktop sessions, flagged for long-lived servers.
- **Step 1 — a *mark-only* reclaimable estimator (safe). ✅ prototyped on this
  branch.** `Heap::runtime_live_closure_count` marks RUNTIME closures reachable from
  `globals` + the process roots (walking the shared code graph: closure arms,
  captured RUNTIME envs, pairs/vectors/maps), moving and freeing nothing. The gap to
  `runtime_closure_count` is the reclaimable set. Validated by
  `tests/runtime_collector.rs`: **after 3000 distinct redefs of one fn,
  total=3001, live=2, reclaimable=2999** — i.e. ~100% of the churn is reclaimable
  and the mark correctly finds the 2 live closures (current `f` + `redef`). Next:
  surface it from Brood (`(runtime-code-stats)` / extend `(gc-stats)`) and extend
  the mark to all processes' roots for a multi-process-accurate figure. This makes
  the leak **observable** and de-risks the real collector with none of the
  STW/moving hazard.
- **Step 2 — the STW compacting collector (its own stage + ADR).** Build the
  runtime-wide safepoint first (reusable beyond GC — e.g. consistent snapshots for
  `nest observe`), then the trace + compact + rewrite + `vm_cache` clear. Only when
  a real session actually hurts (per the Stage-5 principle).

## Effort / risk

The compacting collector is the **single largest and riskiest** remaining kernel
piece: it introduces runtime-wide STW (new, race-prone) and a moving GC over shared
cross-process state. Step 1 (estimator) is small and safe and worth doing on its
own as a diagnostic; Step 2 should wait for demonstrated need (a long server
session), per ADR-011 / the dogfooding principle — accept a slow, bounded,
non-corrupting leak rather than contort the hot read path for memory reclaimable
later.
