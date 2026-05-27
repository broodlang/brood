# Concurrency — green processes on all cores

> Status: **design, for review.** Not implemented.

## Goal

Erlang-*style* concurrency, kept lean:

- **Use all cores** — N scheduler threads (one per core), true parallelism.
- **Green processes** — lightweight, cheap (spawn lots of them), not OS threads.
- **Share-nothing** — each process owns its memory; no shared mutable state, no
  locks on the hot path. Because they share nothing, **any ready process can run
  on any scheduler** (work-stealing).
- **Message passing** — processes talk by sending values to each other's mailbox.
- **Not** a single event loop. Node's model — one thread where a slow call
  stalls everything — is exactly what we're avoiding. Many schedulers; a hot
  process ties up at most one core, never the whole runtime.

This is the BEAM's shape (N schedulers × many green processes), minus the fancy
parts (see "Out of scope" below).

## The model

```
   core 0            core 1            core 2          (N = #cores OS threads)
 ┌─────────┐       ┌─────────┐       ┌─────────┐
 │scheduler│       │scheduler│       │scheduler│
 │ ready-q │       │ ready-q │       │ ready-q │   ← run next ready process;
 └────┬────┘       └────┬────┘       └────┬────┘      idle schedulers steal
      └──────────── work-stealing ───────────┘        from busy ones

 process = own heap + mailbox; runs until it blocks on `receive` or finishes
```

Surface (in mylisp):

| Form | Meaning |
|---|---|
| `(spawn (fn () ...))` | start a new process running the thunk; returns a process handle/id |
| `(send pid msg)` | copy `msg` into `pid`'s mailbox (non-blocking) |
| `(receive ...)` | take the next matching message from own mailbox; **blocks** (yields) until one arrives |
| `(self)` | this process's id |

`spawn`/`send`/`receive`/`self` are the whole user-facing API for v1.

## The two design knobs that need a prototype

These are the only genuinely hard choices; everything else is plumbing.

### 1. How a process suspends (for `receive`)

A process that blocks on `receive` must pause and free its scheduler. Our
evaluator is a recursive tree-walker whose state is on the native stack, so it
can't pause as-is. Two ways:

- **Stackful coroutines** (Go-style; e.g. a Rust coroutine lib) — give each
  process a small growable stack and **keep the recursive evaluator unchanged**.
  Least rewrite — the "nothing as fancy" option.
- **Stepping VM** — rewrite `eval` into an explicit-stack machine. More work,
  but process state becomes plain data (trivially movable) and enables precise
  preemption later.

**Lean recommendation:** start with **stackful coroutines** to avoid the eval
rewrite; revisit a stepping VM only if we later want fine-grained preemption.

### 2. What must be `Send` (the one unavoidable constraint)

Handing a process or a message to another core means that data crosses OS
threads, so it must be `Send`. Today values are `Rc` (`!Send`). Options:

- **Pin processes to their spawning scheduler** (no live migration); balance only
  at `spawn`; steal only *not-yet-started* work. Keeps per-scheduler `Rc` heaps.
  Simplest; some load imbalance possible. Messages crossing threads are
  **deep-copied** into a `Send` form and rebuilt on the other side (Erlang copies
  on send anyway).
- **`Send` per-process heaps** (a `gc-arena` arena per process, or `Arc` values)
  — lets a running process migrate to any scheduler. This is **the same GC work
  we already planned**, so concurrency and the GC migration become one effort.

**Lean recommendation:** prototype the **pinned-process + copy-on-send** model
first (works with today's `Rc`); move to `Send` per-process heaps when we do the
GC migration, which unlocks full migration.

### Shared code, isolated data

Like Erlang: function/macro **definitions are shared** (read-mostly, and
hot-swappable — your "edit on the fly"), while each process's **data is private**
to its heap. We need a clear split between the shared code table and per-process
data. Messages are data, so they copy cleanly.

## Scheduling

Start **cooperative**: a process runs until it blocks on `receive` (or finishes),
then yields. With N cores, a CPU-bound process only occupies one scheduler, so
the lack of preemption is far less harmful than in a single event loop.
Reduction-counted preemption (the BEAM's fairness mechanism) can be added later
if a workload needs it.

## Out of scope for v1 (the "fancy" we're skipping)

- Supervision trees, `link`/`monitor`, restart strategies
- Distribution across machines/nodes
- Live migration of *running* processes (we start pinned)
- Reduction-counted preemption (we start cooperative)
- Selective-receive performance tuning

These are all additive later.

## Impact on the roadmap

This is the largest *core* undertaking in the project. Two consequences:

1. It pulls the **GC migration earlier** — `Send` per-process heaps (`gc-arena`)
   are the path to full work-stealing, so the GC and concurrency are one effort.
2. Until then, prefer adding language features **in mylisp** (string/math/seq
   libraries, maps) that don't deepen the recursive evaluator, so the eventual
   suspension work (coroutines or stepping VM) stays small.

## Phasing

1. `spawn`/`send`/`receive`/`self` with **one** scheduler, cooperative, stackful
   coroutines — proves the API and the suspend/resume mechanism.
2. **N schedulers + work-stealing** of ready processes — uses all cores
   (pinned processes, copy-on-send messages).
3. **`Send` per-process heaps** (with the GC migration) — enables migrating
   running processes; removes the pinning limitation.
4. Later, if needed: preemption, then supervision/links.
