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
- **Step 2 — the STW compacting collector (its own stage + ADR).** Decomposed:
  - **2a — evacuation core. ✅ done (out-of-place, branch `runtime-gc`).**
    `Heap::runtime_evacuate` traces the live RUNTIME code from globals + roots and
    *copies* it into a fresh `CodeSlabs`, building an old→new forwarding map —
    mirroring the LOCAL GC's `flush_*` but over `boxcar`/`OnceLock` and RUNTIME
    handles (closures/envs use `OnceLock` reserve-then-fill for cycles; pairs/
    vectors/maps are acyclic immutable code, so child-first then push-once).
    `verify_rt_slabs` confirms every handle in the evacuated region is in-bounds (no
    missed rewrite). Validated: after 3000 redefs, evacuate → 2 live closures of
    3001, verifier passes, program unchanged (`tests/runtime_collector.rs`).
    **Installs nothing — cannot corrupt the live region.**
  - **2b — in-place collect. ✅ done (branch `runtime-gc`).** `Heap::runtime_collect`
    + the `(runtime-collect)` builtin. **Gated on `Arc::get_mut`** — runs only when
    this heap uniquely owns the runtime region (no concurrent reader), so it's sound
    *without* stop-the-world; returns `:ran false` when the runtime is shared. One
    pass: every RUNTIME handle in globals + both LOCAL generations + roots/env_roots/
    dynamics is evacuated-and-rewritten; the `vm_cache` + `global_ic` are cleared;
    the compacted region is swapped in (`mem::take` avoids the borrow conflict).
    **Opt-in — never wired into the automatic GC.** Validated: reclaims 2999/3001
    after churn; program correct afterwards incl. a RUNTIME closure held in a LOCAL
    binding *across* a collect (`(let (g f) (runtime-collect) (g 3))`); green under
    `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`; full suite 437/437 both engines.
    **Bounds memory** — 40k redefs: no-collect 34 MB (growing) vs periodic collect
    14.5 MB (flat). The Stage-5 goal, for the single-process case.
  - **2c — runtime-wide stop-the-world (design; the race-prone part, deferred).**
    Lets the collector run when *other processes are live* — today 2b's `Arc::get_mut`
    gate skips (`:ran false`) because parked processes still hold runtime-`Arc`
    clones. Grounded in the scheduler as it is:
    - **No central heap registry.** A *running* `Process` (and its `Heap`) is **not
      reachable from any registry** (`scheduler.rs:519`) — heaps are scattered across
      worker coroutine stacks (running), per-worker run queues, and mailbox waiters
      (parked). So a central collector *cannot* iterate all heaps to rewrite them.
    - **Parked vs running split.** A process parked in `receive` is suspended (its
      coroutine yielded) and won't reach an eval safepoint until woken — so it can't
      *cooperatively* rewrite itself. But its `Heap` **is** reachable, via its
      registry-reachable mailbox (`mailbox.state.waiter`). Queued processes are
      reachable via the per-worker queues. ⇒ a **hybrid**: *running* processes
      rewrite their own heap at the STW safepoint (they hold `&mut heap`); *parked/
      queued* ones are rewritten centrally by the coordinator (reachable + not
      executing → safe to mutate).
    - **Swappable region required.** Under STW the runtime `Arc` is still multiply
      cloned, so the swap can't use `get_mut`; `RuntimeCode.code` must become
      interior-mutable-swappable. `arc_swap::ArcSwap<CodeSlabs>` keeps reads ~lock-
      free (atomic load) — vs `RwLock` which taxes the every-call code-read path
      (the original Stage-5 objection). Read-path cost is the key tradeoff to measure.
    - **Protocol:** request a collection epoch (atomic on `RuntimeCode`) → workers
      bring running processes to the existing eval safepoint + barrier; coordinator
      evacuates the shared region once (→ forwarding map) → running procs rewrite own
      heap, coordinator rewrites parked/queued heaps + globals, all clear `vm_cache`/
      `global_ic`, barrier → atomic-swap the region → resume.
    This is **M4-server-scale concurrency infrastructure** — the single largest,
    most race-prone remaining kernel piece (the project's known races live in the
    scheduler). Deferred to a dedicated effort with the full `BROOD_GC_STRESS` +
    concurrency-fanout rig. **2b already bounds memory for the single-process /
    quiescent case**, which covers the practical near-term need; 2c waits until a
    long-lived *multi-process* server session actually hurts (the Stage-5 principle).
  Build only as far as a real session needs (per the Stage-5 principle).

## Effort / risk

The compacting collector is the **single largest and riskiest** remaining kernel
piece: it introduces runtime-wide STW (new, race-prone) and a moving GC over shared
cross-process state. Step 1 (estimator) is small and safe and worth doing on its
own as a diagnostic; Step 2 should wait for demonstrated need (a long server
session), per ADR-011 / the dogfooding principle — accept a slow, bounded,
non-corrupting leak rather than contort the hot read path for memory reclaimable
later.
