# Concurrency v2 — re-enabling work-stealing and supervisor trees

> **Status: design / root-cause analysis (2026-05-29). No code yet.**
> This is the groundwork for the next concurrency track. Both features it
> targets — **work-stealing scheduling** and **kernel supervisor trees** — are
> *exactly* the two pieces whose removal fixed the multi-thread scheduler race
> ([`known-issues.md`](known-issues.md) KI-1). Re-adding either must clear one
> bar: **do not reopen KI-1.** This doc establishes precisely why they raced,
> what invariants now make the scheduler safe, and the constraints a
> reintroduction must honour — so the build starts from solid ground rather
> than re-discovering the race empirically.

See also: [`scheduler.md`](scheduler.md) (current M:N design),
[`supervision.md`](supervision.md) (userland pattern + ADR-039 revert),
[`memory-model.md`](memory-model.md) (bump allocator + automatic copying GC),
[`known-issues.md`](known-issues.md) KI-1/KI-2, [`decisions.md`](decisions.md)
ADR-018/027/039.

---

## 1. What the race actually was

KI-1 was fixed by **three** changes in series, each removing a distinct slice of
the failure surface. It's important to keep them distinct, because they map onto
different constraints for the two features we want back.

| # | Commit | What it removed | Failure mode it closed |
|---|--------|-----------------|------------------------|
| 1 | `e3d3a0d` | Kernel supervisor: `RESUME_SLOT` thread-local, safepoint rooting walk (`for_each_resume_root`), the `supervise()` retry loop, `%spawn-supervised*` | Wide window of **shared mutable scheduler state** + extra rooting surface — the bulk of the race. ~24 worker deaths/run → ~0–1/run. |
| 2 | `f90f0de` | Free-list slot **reuse** in the allocator (→ bump-only, monotonic per process) | **Use-after-free type confusion**: a freed heap slot reallocated to a new value while another thread held a stale handle. Closed it in debug-assertions release (10/10 clean). |
| 3 | `2abf05e` | The single shared `RUN` queue + cross-thread coroutine **migration** (→ per-worker pinned queues) | **corosensei cross-thread resume**: a coroutine resumed on a *different* OS thread than the one it suspended on clobbered return addresses → plain-release segfault. Bisect: disabling preempt fixed it; pinning is the principled fix. |

The root causes are independent: (1) and (2) are about *shared state and stale
handles*; (3) is about *the coroutine substrate's thread affinity*. The fan-out
programs that triggered KI-1 hit all three at once.

## 2. The invariants the fix now rests on

These are **load-bearing**. Anything we add must preserve them or replace them
with something equally strong.

- **INV-1 — slots are never reused in place** (`f90f0de`, [`memory-model.md`](memory-model.md)).
  Every `alloc_*` bumps the slab; the automatic copying collector (ADR-055)
  relocates survivors into a *fresh* arena rather than reusing freed slots, and
  bumps a per-handle generation epoch (ADR-054) so a stale handle trips a debug
  tripwire instead of silently aliasing a reused slot. A stale handle can therefore
  never observe a value of the wrong type — the engine of the use-after-free race
  is structurally gone. (The `(hibernate)` arena flip this invariant once leaned on
  is gone, ADR-058 — automatic GC bounds memory on every entry path now.)
- **INV-2 — one process is owned by exactly one thread at any instant**
  (`2abf05e`, `scheduler.rs:58-77`). A `Process` is `Send` only under the
  hand-written `unsafe impl`, justified by "moved once from spawn into its
  worker's queue, then never across threads again." Every `resume` happens on
  the process's pinned worker.
- **INV-3 — no shared mutable scheduler state on the hot path** (`e3d3a0d`).
  The scheduler's per-thread state (`CURRENT`, `REDUCTIONS`, `GC_BLOCK`,
  `STACK_BASE` — all thread-locals in `scheduler.rs:92-123`) is saved/restored
  around each suspend; there is no cross-thread shared rooting structure.

Note INV-3's thread-locals were *already designed to survive migration* — the
comments at `scheduler.rs:80-82` and `116-122` say so explicitly ("re-established
after every suspend … so it survives … migration to another worker"). **So TLS
is not the work-stealing blocker.** The blocker is narrower: INV-2's coroutine
affinity (root cause #3).

## 3. Work-stealing

### 3.1 The single hard question

Work-stealing **is** cross-thread migration — it directly violates INV-2. The
plain-release segfault (`2abf05e`) is the evidence that, as currently used,
resuming a corosensei coroutine on a thread other than the one that suspended it
is unsafe ("clobbered return addresses"). So the whole feature reduces to one
question:

> **Under what conditions, if any, is it safe to resume a `corosensei`
> coroutine on a different OS thread than the one that suspended it?**

We have *not* yet root-caused the clobber itself — only bisected it to
migration. Three hypotheses, in priority order to investigate:

1. **It's a fundamental corosensei property.** Some stackful-coroutine
   implementations bake thread-specific context (return trampoline, signal/trap
   state, a TLS slot for "current coroutine") into the suspend point. If so,
   cross-thread resume is simply unsupported and work-stealing of *live*
   coroutines is off the table without a different substrate.
2. **It's a Brood-side TLS-capture bug.** Something we read deep in `eval` holds
   a pointer that's only valid on the suspending thread. The `CURRENT` ctx
   stores `yielder: *const Yielder0` (`scheduler.rs:89`) — a raw pointer into the
   coroutine's own stack; that's stack-stable, but the *re-establishment timing*
   after a resume on a new worker needs auditing. The `STACK_BASE` byte-guard
   (`scheduler.rs:116`) is another candidate. If the clobber is one of these,
   it's fixable and migration becomes safe.
3. **It was actually residual root-cause #2** (stale-handle), and pinning only
   masked it. *Unlikely* — `2abf05e` landed after `f90f0de`, and the bisect
   pointed at preempt/migration, not the allocator — but worth ruling out, since
   if true, work-stealing might already be safe on today's substrate.

**The decisive experiment (cheap, do first):** reintroduce a shared queue / steal
path behind a `BROOD_STEAL=1` flag, keep everything else as-is, and run the KI-1
repro (40-worker prelude fan-out) in **plain release**. If it segfaults →
hypothesis 1 or 2; if clean → hypothesis 3.

### 3.1a Experiment run — RESULT (2026-05-29, worktree `track-a-workstealing`)

Ran exactly that, plus a discriminator, in an isolated worktree off HEAD
(`b9ebbee`). Both a work-stealing path (`BROOD_STEAL`) and a spawn-time
load-balancer (`BROOD_BALANCE`) were added behind env flags (default path
byte-identical), built in **plain release**, 40-worker KI-1 repro × 10 each:

| Config | result |
|---|---|
| baseline (pinned) | 0/10 fail |
| `BROOD_BALANCE=1` (load balance, no migration) | 0/10 fail (also 0/5 under `BROOD_GC_STRESS`) |
| `BROOD_STEAL=1` (work-stealing) | **10/10 segfault** |
| `BROOD_STEAL=1` + preempt disabled (huge reduction budget) | 0/10 fail |

**Conclusion: hypothesis 1 (corosensei substrate limit), hypothesis 2 ruled out.**
- The segfault is **preempt-induced cross-thread migration** specifically:
  disabling preemption makes stealing clean, so the hazard is resuming a coroutine
  that suspended *mid-computation* (deep native stack) on a different OS thread —
  the exact wall `2abf05e` hit.
- Every crash backtrace (gdb, 3/3) is in `scheduler::preempt` at the
  `(*yptr).suspend(…)` call, with a **smashed return address** (`0x7`) in frame
  #1 — *not* in corosensei's switch assembly. The Brood-side fix hypothesis 2
  proposed (re-establish `CURRENT`/yielder after resume) is **already present**
  (`scheduler.rs:283`) and does **not** prevent it. So a deep saved coroutine
  stack is not safely resumable cross-thread in corosensei 0.3.4 as we use it;
  this is a substrate constraint, not a cheap TLS bug.
- **Discovered safe partial:** stealing only **fresh, never-resumed** processes is
  safe (the no-preempt run only ever stole fresh procs → clean). A fresh
  coroutine's first `resume` happens on the thief with no saved state to migrate.

**Load balancing is the safe, shippable win.** `BROOD_BALANCE` (assign a fresh
process to the shortest-queue worker; no migration, INV-2 preserved) was clean
across every test, and ~neutral-to-slightly-faster on a 5000-process burst
(1811 ms vs 1911 ms baseline; the per-spawn `try_lock` scan adds no measurable
overhead). Caveat: queue-length balancing sees only *queued* processes, not a
long-running one currently *occupying* a worker — it improves burst distribution,
not uneven long-task occupancy.

The experiment patch lives on branch `track-a-workstealing` (worktree
`../brood-track-a`); it is **not** merged. See §3.2 for what each result implies.

### 3.2 Design directions (gated on 3.1's outcome)

§3.1a settled this: **migration of a suspended coroutine is unsafe in corosensei
0.3.4**, so the directions that relied on it are closed and the viable ones are:

- ✅ **Spawn-time load balancing — LANDED in main (default-on, 2026-05-29).**
  `assign_worker` pins each fresh process to the least-loaded worker (shortest
  queue, rotating-start scan, ties toward the rotation), replacing pure
  round-robin. No migration, INV-2 preserved, proven clean in §3.1a and
  re-validated default-on (plain-release KI-1 0/8, full in-language suites green).
  Degrades to round-robin when load is even.
- ✅ **Per-worker "busy" flag — LANDED in main (2026-06-07).** The refinement the
  bullet above anticipated: `assign_worker`'s load metric now adds 1 when a worker
  is inside `resume` (the `WORKER_BUSY` gauge, set/cleared in `run_one`), so a
  worker *occupied* by one long-running process no longer reads as idle just
  because its queue is empty. Closes the "queue-length doesn't see the running
  process" limitation for placement; still no migration (INV-2 preserved). The
  remaining uneven-occupancy case (a process that turns long-running *after*
  placement) is unfixable without migration — see the fresh-steal note below.
- ⬜ **Limited work-stealing of *fresh* processes only.** §3.1a found stealing
  never-yet-resumed processes is safe (their first `resume` is on the thief, no
  saved native stack to migrate); only *suspended mid-computation* coroutines are
  unmovable. An idle worker could steal a fresh, runnable process from a backed-up
  peer's queue without tripping the substrate constraint — the one migration-shaped
  win available without changing the coroutine substrate. Deferred (no consumer
  yet); full anything-anytime stealing needs a reified/heap stack like BEAM's.
- 🟡 **Fresh-only stealing (optional, additive).** An idle worker may steal
  processes that have **never been resumed** (a backlog of unstarted spawns on
  one worker) — proven safe in §3.1a, since the first resume then happens on the
  thief with no saved stack to migrate. This is real, migration-free
  work-stealing for the spawn-burst case; it does **not** rebalance already-
  running processes. Worth it only if a workload shows spawn bursts piling onto
  one worker that balancing-at-spawn didn't already spread.
- ❌ **Stealing live (suspended) coroutines** — needs a coroutine substrate that
  supports cross-thread resume of a deep saved stack (evaluate alternatives to
  corosensei, or a custom suspend mechanism). Large effort, gated on a
  demonstrated need that the two options above can't meet. Not recommended now.

### 3.3 Non-negotiables for any work-stealing design

- Preserve INV-1 (don't reintroduce slot reuse to "help" sharing).
- A stolen process's heap travels with it (heaps are per-process and `Send`;
  this already holds — see `scheduler.md` §"Send-ness & heap migration").
- The steal handshake must guarantee INV-2's "one owner at any instant" — a
  process is either in exactly one worker's queue, running on exactly one
  worker, or parked in exactly one mailbox. Never two.

## 4. Supervisor trees

### 4.1 Why the kernel version raced

ADR-039's supervisor wasn't racy because *supervision* is hard — it was racy
because of *how it rooted retry state*. It needed to re-apply a worker with the
same args after a crash, so it parked a resume value in the `RESUME_SLOT`
thread-local and taught the eval safepoint to root it (`for_each_resume_root`).
That added shared mutable scheduler state and widened the rooting surface
(root cause #1) — under the *old, slot-reusing* allocator, that surface raced.

### 4.2 What INV-1 changes

The original motivation for kernel-side retry rooting was partly defensive
against stale handles — which **INV-1 has now eliminated.** This reopens a
question worth settling before building: *does a redesigned supervisor need
`RESUME_SLOT`-style kernel rooting at all,* now that slots are never reused?
Plausibly the retry state can live entirely in the supervisor **process's own
heap** (it's just the worker thunk + args, ordinary `Send` values), needing zero
new scheduler-global state. If so, a kernel supervisor could be far thinner than
ADR-039's and stay clear of root cause #1.

### 4.3 Design directions

- **Userland-first (lowest risk, available today).** `spawn` + `monitor` already
  give `[:down …]` and a respawn loop in ~10 lines ([`supervision.md`](supervision.md)).
  ✅ **Done (2026-05-29, ADR-044):** `std/proc/supervisor.blsp` — `start-supervisor`
  over child specs, `:permanent`/`:transient`/`:temporary` restart types,
  restart-intensity windows, `which-children` — all Brood policy over the
  existing primitives, **zero** scheduler surface. **All three strategies now
  ship** (`:one-for-one`, `:one-for-all`, `:rest-for-one`): the `(exit pid :kill)`
  primitive (ADR-063) closed the one gap — terminating healthy siblings — so the
  group strategies are still pure-Brood policy (hard-kill the siblings to restart,
  selectively drain their `[:down]`). Notably this kernel hook is *not* the
  supervision-specific one §4.2 warned about: `exit/2` is a general Erlang
  primitive (any process can signal any other), keeps no scheduler-global retry
  state, and is independently useful — so it adds no ADR-039-style race surface.
- **Kernel-assisted (only if userland proves insufficient).** If a specific need
  appears that userland can't serve (e.g. hot-reload-on-retry, the one thing
  ADR-039 uniquely did), reintroduce the *minimum* kernel hook — and per 4.2,
  design it to keep retry state in the supervisor's heap, not in a new
  scheduler-global slot. Any such hook must be argued against root cause #1
  before it lands.

### 4.4 Interaction with work-stealing

Supervised children are ordinary processes; a work-stealing scheduler steals
them like any other. No special interaction *if* supervision stays in userland
(§4.3 first bullet) — the supervisor is just a process holding monitors. A
kernel supervisor (§4.3 second bullet) would need its retry/rooting to be
migration-safe under whatever §3 concludes — another reason to prefer userland.

## 5. Suggested sequencing

1. **Run the §3.1 experiment.** It's cheap and decides the work-stealing design
   space. Everything else waits on its result.
2. **Build the userland supervisor library** (`std/`, §4.3) in parallel — it has
   no dependency on the scheduler work and delivers supervisor trees with zero
   race risk. This is the fastest path to "supervisor trees" as a user-visible
   feature.
3. **Then** pick the work-stealing design from §3.2 per the experiment, or take
   the INV-2-preserving **spawn-time load-balancing** partial win (§3.2 option b)
   if migration proves unsafe.
4. Only consider a kernel supervisor (§4.3 second bullet) if a concrete need
   outlives the userland library.

## 6. Acceptance bar

Any change here must, before merge, pass the KI-1 reconstruction in **plain
release** (not just debug-assertions release): the 40-worker prelude fan-out,
≥10 clean runs, plus the 80-worker + `BROOD_GC_STRESS=1` variant. A regression
test in `crates/lisp/tests/` should encode it so the race can't silently return.
</content>
</invoke>
