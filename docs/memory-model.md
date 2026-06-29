# Memory model — `Send` heaps and per-process GC

> **2026-05-30 status (current) — the copying collector is now GENERATIONAL
> (ADR-072), with Tier-1 observability.** The LOCAL heap is split into a
> **nursery** (every `alloc_*` bumps into it) and a tenured **old** generation. A
> *minor* collection copies only the nursery's survivors — tenuring them into old
> once the nursery crosses `min_tenure`, else a young semi-space flip — and **never
> recopies the old generation**; a rarer *major* compacts old when it doubles past
> `major_floor`. This is sound with **almost no write barrier** because immutability
> (ADR-026) forbids old→young pointers — the **sole** exception is a frame tenured
> *mid-bind*, tracked in the `remembered` set (`env_define`): a `def`/env-frame
> *binding* rebind (ADR-013, the one binding mutation) can repoint an already-tenured
> frame at a fresh young value. (User-facing transients were tried and removed —
> ADR-026 — so there is **no** `remembered_transients` barrier; data is fully
> immutable, leaving the env-frame rebind as the only old→young edge.) The barrier
> flushes the recorded old object's young refs in place on the next minor and remaps
> the set through the forwarding table on a major. Result on a
> stateful workload (a process holding a large live set across churn): ~8× faster,
> ~9× lower RSS, ~70× less copy volume than the single-space copy below; everything
> else is the same operand-stack-rooted copy described in the next banner. Handles
> carry an age bit + per-generation epoch (the nursery and old epochs bump
> independently). Observability: `(gc-stats)`, `(gc-collect)` (force a collection),
> `(gc-trace on?)` + `BROOD_GC_TRACE`; thresholds tune via
> `BROOD_GC_FLOOR`/`BROOD_GC_TENURE`/`BROOD_GC_MAJOR`. So where the banners below
> say "semi-space copy of the whole live set," read "minor copy of the nursery
> survivors only, plus a rare major over old." See ADR-072 and the 2026-05-30
> devlog entry.
>
> ---
>
> **2026-05-30 status — the collector fires at ANY eval depth
> (ADR-061).** Supersedes the `gc_block_depth() == 1` gate described throughout the
> rest of this doc. The evaluator now keeps every in-flight LOCAL transient on an
> **operand stack** (`Heap::roots` for values + `Heap::env_roots` for envs, both
> relocated in place by `arena_flip`), so the copying collector can run at any eval
> depth — not just the outermost. A loop in argument position (`(f (loop))`), a
> `try`-wrapped loop, or a deep call no longer leaks; the safepoint gate is now
> `!macro_block_active() && gc_due()`. The one exception is the macro-expansion
> compile pass, which opts out of collection via `MACRO_BLOCK` instead of being
> operand-stack rooted. `GC_BLOCK` now feeds only the stack-overflow guard, and the
> ADR-058 `load` depth-1 trick / `GcBlockReset` are gone. Where the prose below says
> "fires iff `GC_BLOCK == 1`" / "depth-1 safepoint," read "fires at any depth, with
> the compile pass excluded." See ADR-061 and the 2026-05-30 devlog entry.
>
> ---
>
> **2026-05-29 status — automatic copying GC at the eval safepoint
> (ADR-054/055).** Reclamation is now automatic and needs nothing from the
> author. The per-process LOCAL heap is still a bump allocator, but a **semi-space
> copying collector** (`Heap::collect`, sharing the `arena_flip` machinery) fires
> at the `gc_block_depth() == 1` eval safepoint whenever the live set crosses an
> adaptive threshold — so a long-running tail loop or `receive` server runs in
> bounded memory automatically. Handles carry a generation epoch (ADR-054) so a
> stale handle held across a flip trips a precise debug tripwire.
>
> **The one thing a program author must never do is think about GC** — there is no
> `while`, no manual collect, and the old `(hibernate)` primitive (which forced a
> flush) was **removed** once automatic collection landed. The only requirement,
> met by the runtime not the user, is that a program runs at the depth-1 safepoint:
> top-level forms (`brood`/`nest run` via `eval_source`), the bodies of spawned
> processes, and files loaded via `load` (which drops to a depth-1 form loop when
> it is the outermost eval). See [`memory-review.md`](memory-review.md) for the
> staged path (Stage A→B) and the entry-depth analysis. The banners below are the
> earlier (pre-ADR-055) designs, kept for the rationale trail.
>
> ---
>
> **2026-05-29 status — bump + flush + shared blobs (commits `f90f0de`
> Phase 1, evening-of-2026-05-29 Phase 2, late-2026-05-29 ADR-041).**
>
> **Today's memory model is:**
> 1. **Per-process bump allocator.** Each green process has its own LOCAL
>    `Slabs` (pairs, vectors, maps, strings, closures, envs). Allocations
>    grow monotonically; no slot reuse, no sweep, no tracing GC.
>    `Heap::collect` is a no-op. The bump alone bounds memory for
>    short-lived processes because the whole heap drops on process exit.
> 2. **Shared code regions** (PRELUDE + RUNTIME) — immutable / append-only,
>    `Arc`-shared. No GC needed; closures live forever.
> 3. **Arena flip on `(hibernate fn & args)`** — long-running processes
>    opt in: hibernate raises an uncatchable `LispError::Hibernate` that
>    unwinds back to the process's run loop, which calls
>    `Heap::flush(&mut roots)` (deep-copy callee + args into fresh `Slabs`,
>    drop the old) before re-applying. Bounds memory in receive loops
>    indefinitely.
> 4. **Arena reset at top-level boundaries** (ADR-016) — still in play for
>    the REPL/file runner between top-level forms.
> 5. **Shared blob heap for large strings (ADR-041)** — per-runtime
>    `Arc<BlobHeap>` sibling to `Arc<RuntimeCode>`. A LOCAL string at or
>    above 256 B is stored as `LocalString::Shared(Arc<SharedBlob>)`;
>    smaller as `LocalString::Inline(String)`. Cross-process `send` ships
>    the `Arc` (atomic incr, no byte copy); `from_message` installs the
>    cloned `Arc` directly into the receiver's LOCAL slab. The `Arc`'s
>    `Drop` is the free path (process exit, hibernate flush of a
>    non-surviving slot, etc.). Cross-node sends downgrade to inline
>    bytes — the receiving runtime has its own `BlobHeap`.
>
> **What we explicitly *don't* use:** tracing GC (gone), `Rc`/`RefCell`,
> `gc-arena`, write barriers (data is immutable per ADR-026), generational
> or incremental collection. ADR-026's immutability means no cycles, so
> the blob heap's plain `Arc` is sound — no `Weak`, no cycle collector.
>
> See [`devlog.md`](devlog.md) 2026-05-29 (Phase 1 + Phase 2) for the
> narrative and rationale, and `crates/lisp/src/core/heap.rs:Heap::flush`
> for the deep-copy details (per-slab forwarding tables; handles cycles
> via placeholder-allocate-then-recurse).
>
> What follows below is the **pre-2026-05-29 design** (mark-sweep + free
> lists, with the GC-arena alternative survey). Kept as the design path —
> it documents *why* the simpler bump-plus-flush model became the right
> step. The Phase-2 caveat earlier in this banner (now resolved) used to
> warn that long tail-recursive loops grew unboundedly; with hibernate
> shipped, the `gc.rs` `long_tail_loop_stays_bounded` test is un-`#[ignore]`d
> and passes.

> Status (pre-2026-05-29): **implemented.** `Send` heaps shipped first (`Value` is a `Copy` handle
> into arena slabs — see ADR-002 → step 2/3 below). Reclamation arrived in two
> steps: **arena reset at top-level boundaries** (ADR-016, cheap O(1)
> truncation), then a **per-process tracing mark-sweep** (ADR-035) that handles
> the mid-evaluation / never-returning-loop case the reset can't reach. The two
> coexist — reset still bounds a long file/REPL session at top-level
> boundaries, and the GC kicks in inside long-running loops.

## Implemented: arena-reset reclamation (ADR-016)

The cheap, safe O(1) reclamation at top-level boundaries:

- The per-process LOCAL heap only grows during evaluation (the arena never moves
  or frees mid-eval). A spawned process frees its whole `Heap` on thread exit, so
  the leak is specifically *long-lived* processes (the REPL, a server).
- **Globals live in PRELUDE/RUNTIME and never point into LOCAL** (a top-level
  `def` *promotes* its value out — see shared-code.md). So at a top-level
  boundary — eval fully returned, stack empty — the only live LOCAL value is the
  form's result. We snapshot the LOCAL slab lengths (`Heap::checkpoint`) and
  truncate back to them (`Heap::reset_local_to`) after consuming the result.
  `eval_str` does this between forms; the REPL after each command. O(1), no
  tracing. (Demo: a file of heavy forms went from ~712 MB growing to ~78 MB flat.)
- **What it does *not* solve:** a single never-returning loop (no top-level
  boundary) keeps accumulating. That's the niche the tracing GC below fills.

## Implemented: per-process tracing GC (ADR-035)

A precise, non-moving mark-sweep that fires at the **outermost-`eval`
`'tail:` safepoint** — exactly when the rooting surface is minimal and
statically knowable.

### Roots

A complete root set at the safepoint, by construction (see ADR-035 for the
correctness sketch):

- `expr` and `env` — passed to `Heap::collect` by the eval safepoint.
- `Heap::dynamics` — the `binding`-form's per-process stack.
- `Heap::roots` — an explicit `Vec<Value>` used by the two depth-0 callers
  (`eval_str` / `eval_source`) for their unevaluated forms.

That's the whole surface. No handle-scopes thread through `eval`'s helpers, no
rooting in builtins.

### The `GC_BLOCK` invariant

A thread-local depth counter, incremented by RAII guards at every `eval()` and
`macroexpand_all()` entry. The safepoint fires GC iff `GC_BLOCK == 1` — *we are
the outermost contributor*. This forces:

- Inner evals (arg evaluation, body forms, nested calls) see `GC_BLOCK >= 2`
  and short-circuit. Cost on the hot path: one TLS read + compare.
- Macroexpansion's internal evals see `>= 2` (the `macroexpand_all` guard is
  also a contributor) — its partially-built forms never get swept.
- A builtin running between an outer eval and an inner eval doesn't fire GC
  on its own; if it calls `eval`/`apply`, *that* eval is `>= 2` and inner
  evals don't GC. GC and builtin-mid-execution are mutually exclusive.

Saved/restored around coroutine suspend (`process::preempt`,
`process::wait_for_message`) and reset to 0 at coroutine entry — so workers
multiplexing several green processes don't leak each other's depths.

### Mark + sweep mechanics

- **Mark** is iterative (an explicit `Vec<TraceItem>` worklist), so a deep
  cons chain or env-frame chain can't overflow the native stack. Per-slab
  `Vec<bool>` mark bits are allocated per collection (no persistent cost).
  PRELUDE/RUNTIME handles are filtered at the worklist-push site — the trace
  never leaves LOCAL.
- **Sweep** rebuilds the free lists from scratch (`(0..len) \ marked`),
  clears dead vector/map/string/closure/env slots so their inner allocations
  drop, and purges `form_pos` entries whose pair slot was freed.
- **Allocators** (`alloc_pair`, …) pop the free list before extending. The
  slab's `len()` is the high-water live count + the largest peak free list,
  not the lifetime allocation total.
- **Adaptive threshold:** after each collect, `gc_threshold = max(GC_FLOOR,
  2 * live)`. Set `BROOD_GC_STRESS=1` to force GC at every safepoint
  (debugging — the full test suite is green under it).

### What's deferred (and why it's fine)

- **GC doesn't fire if a program stays at `GC_BLOCK > 1` forever** — e.g. a
  server loop wrapped in `(try (loop) (catch …))` keeps the outer eval
  blocked in `%try` and never reaches a safepoint until it unwinds.
  Idiomatic Erlang-style loops `try` *within* an iteration, returning to the
  outermost between iterations; that case GCs every iteration. The
  pathological case is recoverable by adding explicit rooting to the few
  builtins that hold transients across eval — incremental, no architectural
  shift.
- **Slab `Vec` capacity doesn't shrink.** The copying collector relocates
  survivors into fresh slabs (dropping the old allocations wholesale), but a
  heap that once peaked high may keep its high-water capacity until the next
  collection's fresh slabs right-size it.
- **The interner and the shared RUNTIME code slabs** (hot-reloaded code via
  `def`) are still append-only and grow with redefinitions. Orthogonal to
  per-process *data* GC. Two process-lifetime growth vectors to keep in mind
  for the long-lived daemon (kernel audit 2026-06-03, perf #4):
  - *Arbitrary strings must not reach `intern`.* The interner never frees, so
    code paths fed by user-typed text use `value::intern_existing` (a lookup,
    no insert) — e.g. the LSP's `resolve_in_source`. The reader itself interns
    every token it scans; that's bounded by actual source content.
  - *`gensym` grows the interner by one entry per call* (`value::gensym` — a
    global counter, so every `<prefix>__<n>` spelling is new): each macro
    recompile mints fresh names. Negligible per reload, but it is the steady
    drip a hot-reload daemon should know about.

## Why now

We chose to do the memory work *before* the multi-core scheduler (so "use all
cores" is real, not faked). Two things are driving it:

1. **`Send` heaps.** To run a green process on any scheduler thread — and to
   migrate it — its heap must be `Send`. Today everything is `Rc`, which is
   `!Send` by design. This is the hard blocker for multi-core.
2. **Real GC.** `Rc` leaks reference cycles (a closure capturing an env that
   reaches it). Fine for a REPL, not for a long-running editor/process.

Constraint from the concurrency model: **share-nothing**. Each process owns an
isolated heap; messages are copied between heaps. So we don't need *shared*
thread-safe values — we need each heap to be **`Send` as a unit** (one thread
touches it at a time), which is different from (and cheaper than) making every
value atomically shared.

## The coupling to understand

Three things are entangled, and the heap choice constrains the other two:

```
   heap model  ──►  evaluator architecture  ──►  how a process suspends
   (Send?)          (recursive vs stepping)      (coroutine vs VM steps)
```

- A **recursive tree-walker** keeps process state on the native stack →
  suspending/migrating it needs **stackful coroutines**, and those coroutines
  must themselves be `Send` to migrate (so they can only hold `Send` data).
- A **stepping VM** keeps process state as plain data → suspension is just
  "stop stepping," and migration is trivial (move the data). This pairs
  naturally with an arena/GC heap.

> We shipped the **recursive** arm (option B below: hand-rolled `Send` arena +
> `corosensei` stackful coroutines), and the VM (ADR-076) later reified the
> *operand* stack but kept native recursion for the *call* stack. That is exactly
> why a *running* process can't migrate across workers today — its call
> continuation is a native stack. The stepping-VM arm (reify the call/frame stack
> too) is the committed way to unblock both **live-process migration** and the
> **fully precise mid-eval GC** this section wants; the full design is in
> [`concurrency-v2.md`](concurrency-v2.md) §7 (ADR-100).

## Options

### A. gc-arena + stepping VM (the Piccolo architecture)

Adopt [`gc-arena`](https://github.com/kyren/gc-arena) (real incremental GC, `Send`
arenas) and rewrite `eval` into a **stepping VM** that runs N reductions per
`arena.mutate()` call, then yields. This is exactly how **Piccolo** (GC'd,
green-threaded Lua) is built — i.e. the *entire stack we want* already exists as
a proven design.

- **+** Best end state: real incremental GC, `Send` per-process arenas,
  suspizable/migratable processes, reduction-counted preemption — all coherent.
- **+** Not a patchwork; follows a battle-tested template.
- **−** Largest rewrite: the `'gc` lifetime brand is invasive (all value-touching
  code runs inside `mutate` closures), **and** it forces the stepping-VM rewrite
  of the evaluator at the same time. Two big rewrites, coupled.

### B. Hand-rolled arena (handles) + stackful coroutines

`Value` becomes a small `Copy` **handle** (an index) into a per-process `Heap`
(a slab/`Vec`). The `Heap` is `Send` if its cells are. Keep the recursive
evaluator; use stackful coroutines for `receive`.

- **+** Conceptually simpler than gc-arena's lifetimes; keeps the evaluator we
  have; can be **staged** (see below).
- **+** GC can come *after* `Send` — a non-collecting arena unblocks multi-core
  first, mark-sweep added later.
- **−** Pervasive mechanical change: every `car`/`cdr`/field access goes through
  the heap; we hand-write the GC; coroutines holding handles across yields need
  care (the heap must travel with the process, not be borrowed across a yield).
- **−** Risk of an incoherent patchwork vs A's proven template.

### C. `Arc` + locks

Replace `Rc` with `Arc`, and `RefCell` with `Mutex`/`RwLock` so values are
`Send + Sync`.

- **+** Smallest diff to *reach* `Send`; keeps the recursive evaluator.
- **−** Wrong model: `Arc` is for *sharing*, but we want *isolation*; atomic
  refcounts + a lock on every variable access cost us on the hottest path, for a
  guarantee (concurrent sharing) we explicitly don't want. Doesn't give GC.
- Verdict: a tempting shortcut that fights the share-nothing design. Not
  recommended.

## Decision (locked in)

- **Approach B** — a hand-rolled per-process **arena** (handles into a slab),
  keeping the recursive evaluator; staged: reach `Send` first → multi-core
  scheduler → add GC. Chosen over A (the full gc-arena + stepping-VM rewrite) to
  fit the "start simple, refine in parallel" approach; both reach the same end
  state and we can converge on a stepping VM later if we want.
- **Code shared read-only, data isolated.** One immutable copy of function/macro
  definitions is shared across processes (enables hot-reload everywhere);
  *data* is never shared between processes.
- **Per-process, single-threaded mark-sweep GC** (see below). The isolation makes
  this simple, which removes B's main drawback.

## GC — simplified by isolation

Because processes share nothing and messages are **copied** between heaps, there
are **no cross-heap pointers**. That collapses the GC problem:

- Each process collects **only its own heap**, independently — no coordination,
  no global stop-the-world (collecting one process pauses only that process).
- The collector is **single-threaded**: no atomics, locks, concurrent marking,
  or barriers. A plain **mark-sweep** suffices.
- **Roots** = that process's evaluator stack + its mailbox. Tracing never leaves
  the local heap.
- The **shared code table** is immutable and lives outside the per-process heaps,
  so per-process GC ignores it. (Reclaiming code orphaned by hot-reload is a
  separate, rare, deferred concern.)

We can ship `Send` with a **non-collecting** arena first (unblocks multi-core),
then add the per-process mark-sweep — neither step needs the other to land.

## Keep Rust a thin substrate

We deliberately hand-roll the GC and the scheduler rather than adopt
Rust-specific machinery (gc-arena, an async runtime, `Arc`/locks as the model).
Rust stays at the **lowest layer** — the allocator behind the arena, and
`std::thread` workers for the schedulers — while the *model* (heap layout, GC
algorithm, processes, mailboxes, copy-on-send isolation, `spawn`/`send`/`receive`
semantics) is **ours**.

- **Why:** control and comprehensibility; the language's semantics aren't dictated
  by a crate's constraints (gc-arena's `mutate`/`'gc` would have reshaped the
  evaluator); portability and a path toward self-hosting; and isolation keeps the
  hand-rolled GC small, so the cost is low.
- **Isolation is guaranteed by the model** (no cross-heap pointers because
  messages are copied), not by leaning on Rust's type system. We still *use*
  `Send` as a guardrail that a process heap is movable — a check, not the design.
- This is not "avoid Rust" — it's "don't let Rust-specific mechanisms define the
  model." The thin substrate stays swappable.

## Staged migration plan (approach B) — complete

1. ✅ **Isolate `Rc` behind the `core/value.rs` seam.** Every heap construction
   goes through `core/value.rs` constructors. Safe, behavior-preserving.
2. ✅ **Introduce the per-process arena.** `Value` is a `Copy` handle into a
   `Heap` (`heap.rs`): per-type slabs for pairs/vectors/strings/closures/natives
   plus env frames. The heap threads through reader/eval/builtins/printer.
3. ✅ **Reach `Send`.** The `Heap` is plain `Vec`s of data, so it's `Send`; a
   `heap_is_send` test asserts it.
4. ✅ **Multi-core processes.** Each process owns its `Heap`; messages are
   deep-copied; symbols share a global interner. Green M:N on a worker pool
   via [`corosensei`] stackful coroutines, with reduction-counted preemption
   and selective `receive`. See `concurrency.md` / `scheduler.md`.
5. ✅ **Per-process mark-sweep GC** (ADR-035; "Implemented: per-process tracing
   GC" above). Fires at the outermost-eval safepoint, gated by `GC_BLOCK == 1`;
   roots are `expr`/`env`/`heap.roots`/`heap.dynamics`. Free lists per slab,
   adaptive threshold, stress mode for testing.
6. ✅ **Suspension** via stackful coroutines for blocking `receive` (landed
   with step 4b).

> Step 2/3 made the heap a `Send`, self-contained unit. Step 4 made each
> process own one. Step 5 finally bounded a long-lived process's footprint.

## Risks (closed)

The "biggest blast radius" risk for the GC turned out to be much smaller than
the doc originally feared: the trampoline structure of the evaluator + the
`GC_BLOCK == 1` invariant collapsed the rooting surface to two sites
(`eval_str`, `eval_source`) and zero rooting in builtins. Validated by:

- the full suite (158 tests) passing under `BROOD_GC_STRESS=1` (GC at every
  safepoint, maximising free-list churn),
- a dedicated `crates/lisp/tests/gc.rs` asserting bounded live counts across
  200k-iteration tail loops and 20k-message server loops, in both the root
  thread and a spawned green process.
