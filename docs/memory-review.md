# Memory management — a critical review (2026-05-29)

> **Purpose.** Step back from the patch-level history and ask the structural
> question: *what is our memory model, which parts are standard practice, which
> are custom to Brood and why, and what is the principled path to **stable**
> memory* (a flat working set over time) rather than the current **spiky** one
> (unbounded climb punctuated by sharp drops)? The brief is explicitly: *prefer
> slow-and-stable over fast-and-spiky.*
>
> This is a design review, not a decision record. When we commit to a direction
> it becomes an ADR. For the *current* implementation state see
> [`memory-model.md`](memory-model.md); for the disabled mark-sweep see ADR-035.

---

## 1. The symptom

Memory is **spiky**. A process's LOCAL heap is a pure bump allocator: every
allocation appends to a typed `Vec` slab and **nothing is ever freed in place**.
Reclamation happens only at three coarse events:

1. **Top-level form boundary** — `eval_str` `checkpoint`s then `reset_local_to`
   (truncates the slabs). Bounds the REPL/file-runner *between* forms.
2. **`(hibernate fn & args)`** — `Heap::flush` copies the live graph into fresh
   slabs and drops the old. Bounds a long-running *loop*, but only when the loop
   author calls it.
3. **Process exit** — the whole heap drops.

So within any one long-lived computation that isn't a hibernating tail loop,
memory climbs monotonically to a cap. The test runner (≈633 tests on one
long-lived process) is the canonical victim: it climbs to the soft cap and trips
a clean `E0043`. The shape is a sawtooth with enormous amplitude — fast
allocation, rare huge drops. The user wants the amplitude *small*: reclaim often,
keep the working set roughly flat, accept the CPU cost.

The root cause is not a leak. It is that **automatic reclamation is switched
off** (the tracing GC is a no-op, ADR-035) and the only automatic boundary
(form reset) doesn't reach inside a running computation.

---

## 2. The invariants any solution must respect

These are the fixed constraints. A memory design that violates one of them is a
non-starter here, regardless of how standard it is elsewhere.

1. **Per-process heaps + copy-on-send (Erlang/BEAM model).** Each green process
   owns a private LOCAL heap; messages cross as deep copies; heaps are `Send` so
   the work-stealing scheduler can move a process between OS threads. ⇒ collection
   is **per-process and single-threaded** — no global stop-the-world, no
   concurrent-marking barriers, no cross-heap pointers to trace. This is the one
   decision that makes everything else tractable. *(Standard, and load-bearing.)*

2. **Immutability (ADR-026) — but NOT acyclicity.** Data never mutates, so there
   are **no write barriers** and the only mutation is `def` rebinding a global.
   It is tempting (and the folklore in older notes says) that immutability ⇒ no
   cycles ⇒ reference counting suffices. **This is false.** `letrec` and mutually
   recursive `defn`s build genuine `env ↔ closure` cycles (a closure captures the
   frame that holds the closure). `flush` already carries cycle-handling code for
   exactly this. ⇒ **pure reference counting would leak `letrec`/mutual-recursion
   cycles**; we need a *tracing* (mark or copy) collector for the general object
   graph. Cycles are *rare and confined*, but not absent. *(This correction is the
   most important single point in this review.)*

3. **Handles are indices into typed slabs, not raw pointers.** A `Value::Pair(id)`
   is a region tag + an index into `Slabs.pairs: Vec<(Value,Value)>` (similarly
   vectors/maps/strings/closures/envs). Chosen for `Send`-safety and the planned
   `Rc`→arena migration (ADR-002). **Consequence that dominates the whole design:**
   *index reuse is unsafe.* If slot 5 is freed and a later alloc reuses slot 5, a
   stale handle to the old slot 5 now silently addresses a *different, valid*
   object — corruption, not a crash. This is precisely why in-place free-list
   reuse (classic mark-sweep) was abandoned. *(Custom to us; the source of most of
   our constraints.)*

4. **The evaluator is a native recursive tree-walker.** Live `Value`s sit in Rust
   locals on the native call stack, where a precise collector cannot find them as
   roots. ⇒ we cannot collect at an arbitrary instant; we can only collect at a
   point where the set of live LOCAL handles is *known and enumerable*. *(Standard
   problem; see §4.)*

5. **A preemptive multi-thread scheduler.** Processes are coroutines preempted on
   a timer and suspended at `receive`. Any rooting argument must hold **across a
   suspend/resume**, not just within one synchronous eval. The original mark-sweep
   "race" was here: a rooting assumption that held single-threaded broke when a
   process could be parked mid-flight on another worker. *(Custom; the real
   hazard.)*

---

## 3. What we do today, mapped onto standard techniques

Most of our machinery *is* standard — we just haven't assembled it into an
automatic collector. Naming the correspondence makes the gap obvious.

| Our mechanism | Standard name | Standard? |
|---|---|---|
| Bump-append into slabs | **Sequential / pointer-bump allocation** (every nursery does this) | ✅ standard |
| `Heap::flush` (copy live to fresh slabs, drop old) | **Semi-space copying collection** (Cheney) | ✅ standard — *we built a copying GC and only call it manually* |
| `checkpoint`/`reset_local_to` at form boundary | **Region / arena memory management** (stack discipline) | ✅ standard |
| Per-process, single-threaded collection | **Per-actor GC** (BEAM) | ✅ standard |
| `(hibernate)` as the reclamation trigger | *(none — userland-driven GC)* | ⚠️ **custom; a smell** |
| `GC_BLOCK == 1` "collect only at outermost eval" | **Safepoint with precise roots** | ◐ standard idea, custom realization |
| Index-slab handles + *no reuse* | *(none — most GCs reuse freed space)* | ⚠️ **custom; trades memory for crash-safety** |

The two custom items in the right column are the whole story:

- **`(hibernate)` as trigger** means *the programmer decides when to collect.*
  That is why memory is spiky: between a loop's hibernate calls, growth is
  unbounded, and a computation that isn't a hibernating loop never collects at
  all. No production language asks the user to call the collector on the hot path.
  This is a bootstrap expedient, not a destination.

- **No slot reuse** is the safety property bought by going bump-only. It converts
  *use-after-free* (a stale handle into reused memory → silent corruption) into a
  mere *leak* (a missed root just isn't reclaimed). Given a recursive native
  evaluator + a preemptive scheduler, that was a sound trade to *unblock
  multi-core first* (ADR-035's "ship a non-collecting arena, add collection
  later"). But "later" is now.

---

## 4. The rooting problem and our two answers to it

Everything hard about collecting here reduces to **"which LOCAL handles are
live?"** With values on the native Rust stack, there are only three standard
answers, and we have effectively chosen the third:

- **(a) Conservative stack scanning** (Boehm): read the raw Rust stack, treat
  anything pointer-shaped as a root. No evaluator changes, but imprecise (retains
  garbage), fragile under Rust's aliasing model, and incompatible with a *moving*
  (copying) collector — which is exactly what `flush` is. ✗ for us.
- **(b) Explicit operand-stack VM** (BEAM, CPython, Lua): rewrite the evaluator so
  every live value lives in a heap-scannable register file, not a Rust local. The
  gold standard — fully precise, collect anywhere — but a large rewrite of the
  tree-walker. The eventual right answer; too big for "stabilize now".
- **(c) Safepoints with enumerable roots** (handle/shadow stacks; gc-arena): only
  collect at points where the complete live set is explicitly known.

We picked (c), and we actually have **two different flavours of it**, which is
the key insight of this review:

### Model 1 — In-place mark-sweep at the eval safepoint *(disabled)*

`collect` was called from the top of the `'tail:` loop, gated on
`GC_BLOCK == 1`. The claim: at the outermost eval, the only live LOCAL transients
are `expr`/`env` (passed as roots) plus the dynamics/root stacks; no *intervening*
Rust eval frame holds an un-rooted handle. It marks from those roots and sweeps
the slabs **in place**, rebuilding free-lists.

- **Reach:** can run mid-computation, every tail iteration → could keep *any*
  long loop flat, not just hibernating ones.
- **Fatal flaw (×2):** (i) in-place sweep ⇒ **slot reuse** ⇒ a rooting bug is
  silent corruption (invariant #3); (ii) the `GC_BLOCK == 1` "no intervening
  transient" claim is *exactly* what broke under the preemptive scheduler
  (invariant #5). Disabled.

### Model 2 — Copying flush after an unwind *(active, via `hibernate`)*

`flush` requires you to **first unwind the Rust stack** (via the
`LispError::Hibernate` sentinel caught at the coroutine boundary). After the
unwind there are provably *no* intervening eval frames — the entire continuation
has been reified as `fn + args`. So the root set is trivially complete: the
explicit roots + dynamics + heap.roots, nothing hidden on the stack. It then
**copies** live objects to fresh slabs and drops the old.

- **Safe by construction:** the unwind *eliminates* the rooting-completeness
  question that sank Model 1 — there is nothing on the stack to miss. And copying
  **never reuses a slot index**: it relocates and drops, so invariant #3 is
  satisfied without a free-list. A rooting bug here is a debug-catchable
  use-after-free (the poison tripwire), not silent corruption.
- **Reach (the limitation):** flush only works where the continuation = a single
  `fn + args`, i.e. **at a tail call**. A *deep non-tail* computation
  (`(f (g (h …)))` building a large intermediate) has its continuation spread
  across live Rust frames; it cannot be flushed and is bounded only by the cap.

**This is the crux.** We already own a safe, standard copying collector. Its only
real restriction is that it collects at tail boundaries, which is *exactly* where
long-lived Brood programs spend their time (every server loop, the REPL, the test
runner, the editor event loop is a tail-recursive `receive`/iterate loop).

---

## 5. The design space, scored against §2

- **Reference counting.** Smoothest possible reclamation (frees promptly, no
  pauses → naturally *stable*). **Rejected as the sole mechanism:** leaks
  `letrec`/mutual-recursion cycles (invariant #2), and per-slot refcounts imply a
  free-list ⇒ slot reuse ⇒ invariant #3. Could complement a tracing backstop
  (RC + occasional cycle-collector), but that's more machinery than copying.
- **In-place mark-sweep.** Reclaims any garbage including mid-non-tail-computation.
  **Rejected** unless slot-reuse is made safe (see "generational indices" below):
  invariants #3 and #5.
- **Semi-space copying (what `flush` is).** Tracing (handles cycles ✓), moving
  (no slot reuse ✓), single-threaded per-process ✓. Cost: copies *live* data each
  cycle; for a young-death workload (most env frames/temporaries die immediately)
  the live set is tiny, so copies are cheap. **Best fit. We already have it.**
- **Generational copying (BEAM's choice).** A nursery (bump) + an old generation;
  minor GC copies only nursery survivors. The refinement that makes copying
  *fast as well as stable*, because the per-collection cost tracks survivors, not
  total allocation. The natural evolution of Model 2.

---

## 6. Recommendation — a staged path to stable memory

The principle: **make collection automatic and frequent, keep it copying (no slot
reuse), and pay for stability with CPU — exactly the trade requested.**

**Stage A — auto-hibernate long-lived loops (smallest step). ✅ DONE for the test
runner (2026-05-29).** Nothing new in the kernel: make the long-lived loops that
grow (test runner; any `receive` server) recur through `(hibernate loop state)`
like the REPL already does (`std/repl.blsp`). This converts their unbounded climb
into a bounded sawtooth whose amplitude is one iteration's working set. *Cost: a
deep copy per step — slower, flat. Acceptable per the brief.* **Measured:** the
test runner (`std/test.blsp`, the hibernating-driver block) took the full suite
from ~4 GiB (tripping the cap) to **peak 1135 MB, 655/655 pass** — confirming the
growth was *garbage*, not live data. This is explicitly a **temporary smell** (a
userland GC trigger); Stage B removes it.

**Stage B — automatic threshold-triggered collection (the real fix).**
Promote Model 2 from manual to automatic. At the `GC_BLOCK == 1` safepoint, when
`mem` crosses a threshold, instead of the disabled in-place `collect`, perform a
**copying collection** with `expr`/`env` as the mutable roots (reuse `flush`'s
machinery; rewrite the loop's `expr`/`env` to the returned handles; dynamics and
heap.roots are already flushed internally). Because it copies rather than reuses
slots, it does **not** reintroduce the invariant-#3 corruption that killed the old
mark-sweep — a rooting miss degrades to a debug-catchable dangling handle.
- The threshold *is* the slow/stable dial: small ⇒ flatter + more CPU; large ⇒
  spikier + less CPU. `BROOD_GC_STRESS` (collect every safepoint) becomes the
  extreme-stable setting and a correctness fuzzer.
- **The gating risk, stated honestly:** this still relies on the `GC_BLOCK == 1`
  invariant being sound *across scheduler suspend/preempt/receive* (invariant #5)
  — the same property whose violation sank Model 1. The difference is that the
  failure mode is now loud (poison tripwire / use-after-GC panic) instead of
  silent. **Prerequisite work:** audit that every suspend site saves/restores
  `GC_BLOCK` and that no worker can observe a process at depth 1 with a live stack
  transient (the byte-stack-guard work already saves/restores `GC_BLOCK` across
  suspend — verify it covers preemption *and* `receive`). Run the full suite and
  `gc.rs` under `RUSTFLAGS="-C debug-assertions=on" BROOD_GC_STRESS=1` on all
  cores; green there is the evidence the old model never had.

**Stage C — generational, if Stage B's copy cost is too high.** Split the slabs
into nursery + old; minor GC copies only nursery survivors. Standard, and the
point where copying stops being "slow". Gate on real measurement, not now.

**Optional cross-cutting — generational (slotmap) handles.** If we ever *do* want
in-place reuse (to reclaim a deep non-tail computation that copying can't reach),
give each slot a generation counter and carry it in the handle (the Rust
`slotmap`/`generational-arena` pattern). A stale handle's generation mismatches on
deref ⇒ a clean detected error, not silent corruption — making invariant #3
*enforceable* rather than merely *avoided*. This is the principled way to unban
slot reuse, independent of which collector we run.

**Explicitly deferred — the operand-stack VM (Model b).** The only thing that
lets us collect inside a deep non-tail computation precisely. Large; revisit when
the tree-walker is the bottleneck, not before.

---

## 7. What to measure (so "stable" is a number, not a vibe)

- **Suite high-water `mem-peak`** under Stage A, then Stage B at several
  thresholds — plot amplitude vs. wall-clock to pick the dial.
- **Pause distribution:** time per copying collection vs. live-set size — confirms
  the young-death assumption (copies should be cheap) and tells us when Stage C is
  warranted.
- **Correctness:** full suite + `gc.rs` green under
  `RUSTFLAGS="-C debug-assertions=on" BROOD_GC_STRESS=1` across all cores, with
  the poison tripwire armed. This is the gate for trusting Stage B.
- **Never** measure an unbounded run with `BROOD_MEM_LIMIT=0` — an uncapped no-GC
  run once OOM-froze the host.

---

## 8. One-paragraph answer to the brief

We are not missing a collector — we *have* a standard semi-space copying collector
(`flush`); we have wired it to fire only when the programmer says `(hibernate)`.
The spikiness is that manual trigger. Immutability spares us write barriers but —
contrary to the folklore — **not** cycle collection (`letrec`), so pure
refcounting is out; copying is the right tracing scheme because it also sidesteps
our defining hazard, index reuse. The path to *slow-and-stable* is to make the
existing copy run **automatically at a memory threshold** at the eval safepoint
(Stage B), after auditing the `GC_BLOCK`/suspend rooting invariant that the old
in-place mark-sweep got wrong — with the immediate win being to make long-lived
loops hibernate today (Stage A). The threshold is the exact knob the brief asks
for.
