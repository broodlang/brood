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
- ✅ **Fresh-only work-stealing — LANDED in main (2026-06-07).** An idle worker
  steals a process that has **never been resumed** from a backed-up peer's queue
  and runs it itself — proven safe in §3.1a, since the first resume then happens
  on the thief with no saved native stack to migrate. Implementation
  (`scheduler.rs`): a `Process.fresh` flag (cleared at the first `resume` in
  `run_one`); `try_steal(thief)` scans peers from a rotating start under
  `try_lock`, pulls the first fresh process from a victim's **back** (the owner
  pops the front), and re-pins its `worker_id` to the thief (owner-for-life from
  then on, so INV-2 holds); `worker_loop` does own-queue → `try_steal` →
  park-with-`STEAL_BACKOFF`-backstop (an idle worker isn't notified when a *peer's*
  queue grows, so it re-probes on a 10 ms timeout, gated by a relaxed `STEALABLE`
  counter so a truly-idle pool re-parks on one atomic load). Observable via the
  new `(steal-count)` builtin. This rebalances the **spawn-burst backlog** (fresh,
  unstarted processes piled onto one worker that placement-at-spawn didn't
  spread); it does **not** rebalance already-running processes — see §7. Verified:
  `tests/work_stealing.rs` (20/20 release, 5/5 debug), KI-1 guard
  (`concurrency_race.rs`) clean 13/13 plain-release incl. `BROOD_GC_STRESS`, full
  suite green.
  - Implementation note (cost me a debug cycle, recorded so it isn't re-hit):
    `worker_loop`'s own-queue pop must bind the `pop_front()` result to a `let` so
    the queue `MutexGuard` drops **before** `run_one`. In edition 2021 a guard
    held in an `if let` scrutinee lives to the end of the whole block, so
    `if let Some(p) = lock(..).pop_front() { run_one(p) }` holds the queue lock
    across the resume — and the running coroutine's first preempt re-enqueues onto
    this same worker, re-locking the non-reentrant mutex → the worker deadlocks.
- ❌ **Stealing live (suspended) processes — needs the stepping-VM evaluator, not
  a corosensei replacement.** This is the BEAM-style full rebalancing (migrate a
  *running* process off a hot worker). It is blocked not by corosensei
  specifically but by where a process's **call continuation** lives — the native
  Rust stack — which is true of the tree-walker *and* today's VM (non-tail calls
  still recurse natively). Swapping the stackful substrate doesn't fix it; the
  fix is to reify the call stack as relocatable heap data (the "stepping VM" arm
  of `memory-model.md`'s coupling diagram). The full design — what it requires,
  what's already migration-ready, the staged plan and acceptance bar — is **§7**.
  Large effort; gated on a workload that fresh-only stealing + placement can't
  serve. Recorded as the committed long-term direction (ADR-100).

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
(Encoded: `tests/concurrency_race.rs`. Fresh-only stealing adds
`tests/work_stealing.rs`, which must clear the same plain-release bar.)

## 7. Full process migration — the stepping-VM endgame

> **Status: committed direction, deferred build (2026-06-07). ADR-100.**
> This is the design for BEAM-style rebalancing: moving an *already-running*
> process off a hot worker onto an idle one. Fresh-only stealing (§3.2) handles
> the unstarted backlog; this section is the rest. The conclusion of the analysis
> is that it is **not** a "replace corosensei" task — it is an evaluator-core
> change, and corosensei falls away as a side effect.

### 7.1 Why it's blocked: the call continuation lives on the native stack

A green process's *continuation* — the chain of pending non-tail calls (return
points, locals, "where do I go when this returns") — is encoded in **native Rust
stack frames**. This is true of both execution engines:

- the **tree-walker** (`eval/mod.rs`) recurses one Rust frame per combination;
- **today's VM** (`eval/compile.rs`, ADR-076) reified the **operand stack** onto
  the heap (so the moving GC can relocate transients at a safepoint) and
  trampolines **tail** calls (`dispatch`'s `'apply: loop`) — but **non-tail calls
  still recurse on the native Rust stack** (`exec_call` → `vm_apply`).

corosensei's only role is to freeze that native stack at a `receive`/preempt
point and resume it later. §3.1a proved that resuming a frozen native stack on a
*different* OS thread segfaults (smashed return addresses) — so the suspended
continuation is **thread-pinned because it is a native stack**, not because of
anything Brood-specific. Replacing corosensei with another stackful library
inherits the same hazard: any saved native stack can hold thread-affine state
(cached TLS addresses, pointers into per-thread structures) and is fragile to
resume cross-thread. That route is the ❌ in §3.2 for good reason.

### 7.2 What migration actually requires: reify the call stack

Migration becomes trivial once a paused process's **entire** state is relocatable
heap data rather than a native stack. That is the "stepping VM" arm of the
coupling in [`memory-model.md`](memory-model.md) ("state as data → suspension is
*stop stepping*, migration is *move the data*"). Concretely, finish what the VM
started — reify the **call/frame stack** the way the operand stack already is:

- A heap **frame stack** — `Vec<Frame>` (or a per-process arena region), each
  `Frame` carrying its instruction pointer / continuation node, its locals/slot
  window, and its return target.
- A **flat dispatch loop** — `loop { step the top frame }` with `Call` pushing a
  frame and `Return` popping one, replacing the `exec_call → vm_apply` native
  recursion. (Tail calls already work this way; this generalises it to *all*
  calls.)
- A paused process is then exactly `(frame_stack, operand_stack, ip)` — plain
  `Send` data. **No coroutine.** Suspension is "return out of the loop"; resume is
  "re-enter the loop with the saved state," on **any** worker.

This single change delivers three things at once — which is why it is the right
endgame rather than a point fix:

1. **Migration of any started process** + **anytime work-stealing** (not just
   fresh): the steal handshake §3.3 already built for fresh processes generalises
   directly once the continuation is movable.
2. **Fully precise mid-eval GC** — the original, separately-motivated reason
   `memory-model.md` wanted the stepping VM (no native frames to scan for roots).
3. **corosensei is removed** — along with the 16 MiB-per-process native stacks
   (`CORO_STACK_BYTES`) and the `unsafe impl Send for Process`; processes get
   genuinely cheap (heap frames grow on demand), closer to BEAM's millions.

### 7.3 What's already migration-ready (the substrate is the *only* gap)

Everything around the evaluator was already built to migrate, so this is a
contained (if large) change, not a system-wide one:

- **Per-process heaps are `Send`** and travel with the process; messages cross as
  deep copies (share-nothing already holds — §3.3, `memory-model.md`). ✅
- **Scheduler thread-locals** (`CURRENT`, `REDUCTIONS`, `GC_BLOCK`, `STACK_BASE`,
  `MACRO_BLOCK`) are saved/restored around every suspend and were *explicitly
  designed to survive migration* (§2 note, `scheduler.rs` comments). ✅
- **The one-owner handshake** — a process is in exactly one queue / running on one
  worker / parked in one mailbox, never two (INV-2) — is implemented and now
  exercised by the fresh-steal path; it generalises to any process. ✅
- **INV-1** (no slot reuse; moving collector + generation epochs) is independent
  of the engine and stays as-is. ✅

### 7.4 The one carve-out (same as BEAM's dirty schedulers)

A process blocked **inside a long native builtin** (e.g. a blocking socket read)
still can't be migrated or preempted mid-call — there is no Brood-level safepoint
in the middle of Rust code to capture a continuation at. This is exactly Erlang's
*dirty scheduler* carve-out. Brood's builtins are nearly all short, so it's
minor; the blocking-IO offload pool (`handoff-blocking-io.md`, M4) is the place
that already plans to push the genuinely-blocking ones off the worker threads.

### 7.5 Staging (keep the suite green at each step)

The VM is the default engine, so this is staged *inside* it, behind a flag until
parity, not a parallel rewrite:

1. **Reify the call stack for the VM** — convert `exec_call`/`vm_apply` native
   recursion into the explicit frame stack + flat loop. No scheduler change yet;
   prove parity (full suite, the differential test vs the tree-walker) and
   benchmark — the flat loop should be neutral-to-faster (no per-call Rust frame).
2. **Replace coroutine suspension with state capture** — `receive`/preempt return
   out of the loop with `(frames, operands, ip)` instead of `yielder.suspend`.
   The scheduler stores that struct in place of a `Coroutine`. Drop corosensei and
   the per-process native stack. Re-run the **§6 acceptance bar** (this is the
   point the KI-1 surface could in principle reopen — it must not).
3. **Generalise stealing/migration** — `try_steal` (and a periodic rebalancer, if
   a workload wants it) may now move *any* runnable process, not only `fresh` ones.
   The `fresh` flag and its special-case stealing become redundant and are removed.
4. **(Optional) BEAM-style periodic migration** — compute migration paths from
   per-worker queue lengths and rebalance proactively, not only on steal-when-idle.
   Additive; gated on a demonstrated long-task-occupancy skew.

### 7.6 Acceptance bar (in addition to §6)

- The §6 KI-1 reconstruction stays green in **plain release** through stage 2
  (the migration cutover) — the original race must not reopen.
- A new regression test that **migrates a process mid-computation** under load
  (deep non-tail recursion suspended at `receive`, resumed on a different worker)
  and asserts the correct result over many trials — the live-migration analogue
  of `work_stealing.rs`.
- Tail-call O(1)-stack and deep non-tail-recursion behaviour preserved
  (`tail_calls_do_not_overflow`; the `BROOD_STACK_BUDGET` guard becomes a
  frame-count/heap-bytes guard instead of a native-stack-bytes guard).
- VM parity + no perf regression on the bench suite (the flat loop is the bet that
  it's faster).

## 8. Implementation plan — corosensei removal (architecture **B**, accepted 2026-06-07)

> Migration steps 1–2 are landed (`scan_mailbox` split; the `Control::Suspend`
> signal that `%try` re-raises, dormant). This section is the concrete plan for the
> rest, after deciding **B** — *remove corosensei outright* rather than keep it as a
> fallback. Built **flag-gated alongside corosensei** (`BROOD_STATE_CAPTURE`,
> default off) until proven, then corosensei is deleted — the bytecode-engine
> playbook. Every step holds the §6 / §7.6 acceptance bar.

### 8.1 The enabler and the scope

When a **clean** `receive` (the suspending point is reached directly through
bytecode + the `%receive` native, no *stateful* native in between) suspends, the
`Control::Suspend` signal propagates up to `vm_run_bc`, unwinding only the
transient native frames (`dispatch`/`exec_chunk`-call/`receive_match`) — which hold
no durable state — while `vm_run_bc`'s `frames` Vec and the operand stack
(`Heap::roots`) stay intact. So `vm_run_bc` captures the full continuation as
`(frames, cur_*, ip)` and returns it; re-entry replays from there. **Survey finding
(2026-06-07):** across the stdlib (supervisors, gen_servers, `task`, `sse`, `test`,
`serve`) the suspending `receive` is *always* clean — a tail position in a loop,
never nested in `try`/`binding`/`%isolate`. So state-capture covers the real
workloads; the rare native-nested case is handled by re-run (below).

Removing corosensei means **every** yielder use migrates, not just `receive`:
- **`preempt`** (reduction tick): at `vm_run_bc`'s loop top — a clean frame
  boundary — capture state and return `Preempted`; `run_one` re-enqueues; resume
  re-enters. (No native stack to freeze; the loop top is already the safepoint.)
- **`:kill`** (`Suspend::Kill`): becomes a capture-and-discard (retire with reason);
  no resume.
- **Tree-walked processes** (`BROOD_VM=0`, or an arm that defers to `eval::eval`):
  have **no bytecode frame stack** to capture. Options: (a) keep a *minimal*
  coroutine only for tree-walked process bodies (corosensei not fully gone, but
  off the common path), or (b) require process bodies to be VM-eligible (they
  almost always are post-Stage-5; a deferred arm is the macro/`def` edge). Decide
  during stage 8.3 from how often a real process body defers.
- **Native-nested `receive`** (`receive` inside `try`/`binding`/`%isolate`): the
  stateful native re-raises the `Control::Suspend` (no cleanup), and on resume
  `vm_run_bc` re-executes the native's `Inst::Call` — **re-running** the native +
  its thunk from the start. Correct when the thunk has no irreversible side effect
  *before* the `receive` (the only shape that occurs); a documented footgun
  otherwise. `binding` re-installs its dynamic value on re-run (idempotent);
  `%try` re-enters its body (the receive re-scans).

### 8.2 The capture/resume machinery (`compile.rs`, `mailbox.rs`)

- `Suspended { frames: Vec<BcFrame>, cur: BcFrame, /* + receive deadline */ }` — the
  reified continuation. Promote `vm_run_bc`'s local `Frame` to a module `BcFrame`.
- `vm_run_bc(heap, arm, args, env, resume: Option<Suspended>)` — start fresh or
  resume; on a `Control::Suspend` returned through `exec_chunk`, rewind the
  `Inst::Call`'s `ip` (so re-entry re-runs `%receive`), capture `(frames, cur_*,
  ip)`, and return a `Suspended` outcome (a new return enum, *not* `LispResult`).
- `scan_mailbox` no-match + green + `BROOD_STATE_CAPTURE` → `Err(LispError::suspend(deadline))`
  instead of `wait_for_message`. `exec_chunk`'s `Inst::Call` intercepts a control
  signal (rewind ip, return `ChunkExit::Suspend`); `vm_run_bc` turns it into the
  `Suspended` outcome.
- `binding`/`%isolate` join `%try` in re-raising a control signal untouched.

### 8.3 The scheduler cutover (`scheduler.rs`)

- `Process` holds `Run::Suspended(Box<Suspended>)` instead of (or alongside, while
  flagged) the `Coroutine`. `run_one` calls `vm_run_bc(.., resume)` directly; a
  `Suspended` outcome parks it (mailbox wait + timer, the work `wait_for_message`
  did); a `Preempted` outcome re-enqueues; `Done`/`Err` retires.
- **Migration falls out:** a parked/runnable process is now plain `Send` data, so
  `try_steal` (and a periodic rebalancer) move *any* process — the `fresh`-only
  restriction and KI-1b pin are gone. Generalise §3's steal; delete the `fresh`
  flag.
- Delete corosensei: `Coroutine`/`Yielder`/`Suspend`/`CORO_STACK_BYTES`/the
  `unsafe impl Send for Process`. The `BROOD_STACK_BUDGET` native-byte guard
  becomes the `MAX_BC_FRAMES`-style frame guard already in `vm_run_bc`.

### 8.4 Rollout

1. ✅ **(2026-06-08)** Machinery (8.2) behind `BROOD_STATE_CAPTURE`, corosensei still
   default. `vm_run_bc` takes `resume: Option<Suspended>` and returns
   `VmOutcome::{Done,Suspended}`; `exec_chunk`'s `Inst::Call` intercepts the
   `Control::Suspend` from `%receive` (rewinds `ip`) into `ChunkExit::Suspend`, and
   the driver captures `(frames, cur_*, ip, entry-marks, deadline)` without unwinding.
   `scan_mailbox` no-match + green + flag → `Err(LispError::suspend)`; a nested-native
   suspend re-raises (8.1 re-run). Capture→resume unit test + green-receive signal
   test; suite + differential green at the default; §6 plain-release KI-1 bar
   re-cleared (10/10 + `BROOD_GC_STRESS`).
2. `run_one` dual-mode (coroutine default; state-capture under the flag); the new
   live-migration regression test (§7.6) passes flag-on.
3. Flip the default; full `make test` + the §6 plain-release KI-1 bar green.
4. Delete corosensei (8.3 last bullet); re-run the bar. Generalise stealing.

This is the scheduler core (the KI-1 subsystem) — run it as a focused effort, not
folded into unrelated work.
</content>
</invoke>
