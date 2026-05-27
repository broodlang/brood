# Memory model — toward `Send` heaps and GC

> Status: **design, for review.** Not implemented. This is the approach to pick
> before rewriting the interpreter core.

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

## Staged migration plan (approach B)

1. ✅ **Isolate `Rc` behind the `value.rs` seam.** Every heap construction goes
   through `value.rs` constructors. Safe, behavior-preserving.
2. ✅ **Introduce the per-process arena.** `Value` is now a `Copy` handle into a
   `Heap` (`heap.rs`): per-type slabs for pairs/vectors/strings/closures/natives
   plus env frames. The heap threads through reader/eval/builtins/printer. No
   behavior change (25 tests green); still single-threaded — one heap.
3. ✅ **Reach `Send`** (non-collecting arena — it only grows for now). The `Heap`
   is plain `Vec`s of data, so it's `Send`; a `heap_is_send` test asserts it.
4. 🟡 **Multi-core scheduler** (concurrency doc phase ②): N threads,
   work-stealing, copy-on-send messages, default **2 schedulers** for testing.
   Groundwork started: the symbol interner is now **global** (so symbol ids match
   across threads). Paused here to build the native test library first; uncovered
   that this step also needs cross-heap message serialization and (for true green
   threads) coroutine suspension — see `concurrency.md`.
5. ⬜ **Per-process mark-sweep GC** (see "GC — simplified by isolation"). Roots =
   each process's stack + mailbox; the shared code table is outside it.
6. ⬜ **Suspension** via stackful coroutines for blocking `receive`.

> Today there is exactly **one** heap (the single REPL "process"). Step 2/3 made
> that heap a `Send`, self-contained unit — the prerequisite for *per-process*
> heaps, which arrive with the scheduler (step 4).

## Risks

- Biggest blast radius of any change so far; touches `value.rs`, `env.rs`,
  `eval.rs`, `builtins.rs`, `printer.rs`.
- Handle-threading will be viral through signatures once the arena lands (step 2+).
- No user-visible payoff until step 4 — we're rebuilding the foundation.

Mitigation: keep `cargo test` green at every step (the test suite is the safety
net), and migrate behind the `value.rs` seam so the change is mechanical, not
semantic.
