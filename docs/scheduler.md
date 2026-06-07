# Step 4b — green M:N scheduler

> Status: **implemented**, now **preemptive**. Stages 1–2 (processes are
> `corosensei` coroutines on an ≈`nproc` worker pool, suspending at `receive`) and
> Stage 4 (reduction-counted preemption — ADR-027) are done. Stage 3
> (work-stealing) remains a deferred optimisation. The *rationale* lives in
> [`concurrency.md`](concurrency.md); this is the build plan and how it landed
> (ADR-018, ADR-027).

## Goal & what changes

Today (4a): every `spawn` is its own OS thread, and `receive` **blocks** that
thread. That oversubscribes cores and can't scale to many processes; it also
needs the `Gate` cap to avoid spawning unbounded threads, and risks deadlock when
more processes block on `receive` than the cap allows.

4b: a `spawn` creates a cheap **green process**; a fixed pool of **worker
threads** (≈ core count) runs them; `receive` on an empty mailbox **suspends** the
green process (freeing its worker) instead of blocking. Result: millions of
processes possible, OS threads bounded at the pool size, and the `Gate`/deadlock
problem disappears (a process waiting on `receive` yields its worker, so whatever
it's waiting for can run there).

**Unchanged:** the language surface — `spawn` / `send` / `receive` / `self` and
message semantics. **Changed:** `process.rs` internals; `-j N` becomes the worker
count (default ≈ `nproc`, capped); the `spawn-count` / `peak-threads`
introspection (now processes ≫ OS threads — the test summary line updates).

## Configuration (pool size is a setting, not a magic number)

The worker-pool size must not be hardcoded. Resolution order:

1. built-in default = **`nproc`** (use the cores),
2. a **settings file**,
3. env var,
4. CLI flag (`-j N`) — wins.

The settings file is **Brood**, mirroring Elixir (`config/config.exs` is Elixir,
not TOML) and our "write the language in the language" rule: a `config.blsp`
(project-local, with a user/global fallback) evaluated at startup into a settings
table. Scheduler thread-count is needed *before* the scheduler exists, so a tiny
single-threaded bootstrap eval reads the config first, then the pool starts.

**Decoupled from this build:** the scheduler reads its thread count through a
settings accessor (default `nproc`, overridable via `-j`); wiring the *config
file* is a small, separate follow-up and does not block Stages 1–2.

## Approach: stackful coroutines (Path A)

Each green process runs inside a **`corosensei` coroutine** (v0.3.4) — its own
stack that can be parked and resumed on any worker. The native recursive `eval`
runs unchanged inside it; suspension is a stack switch, **no evaluator rewrite**.
(The explicit-value-stack VM — Path B — is deferred; it's only needed for precise
mid-eval GC, which is a separate effort. See `memory-model.md`.)

### The crux: how `receive`, deep inside `eval`, suspends

`receive` is a builtin called from within `eval`, which is within the coroutine
body. corosensei hands the *yielder* to the coroutine's top-level closure, but
`receive` is many frames down. Bridge it with a **thread-local**:

```
thread_local CURRENT: { pid, mailbox, yielder } // set by the worker before each resume
```

- The worker, before resuming a process's coroutine, sets `CURRENT` to that
  process's context (pid, mailbox handle, and the coroutine's yielder).
- `receive` reads `CURRENT`: if the mailbox has a message, pop and return it; if
  empty, call `yielder.suspend(Suspend::Receive)` — control returns to the
  worker, the process becomes `Waiting`. On resume it loops and re-checks.
- `self` reads `CURRENT.pid`.

This works because a worker runs exactly one coroutine at a time; `CURRENT` is
re-established on every resume.

## Data model

```
enum ProcState { Ready, Running, Waiting, Done }

struct Process {
    pid: u64,
    coroutine: Coroutine<Resume, Suspend, ()>, // owns its Heap (captured); Send
    mailbox: Arc<Mailbox>,                      // shared with senders
}

struct Mailbox { queue: Mutex<VecDeque<Message>>, /* + Waiting flag */ }

struct Scheduler {
    ready: Mutex<VecDeque<Process>> + Condvar,  // global run queue (stage 1/2)
    parked: Mutex<HashMap<u64, Process>>,        // Waiting processes, by pid
    registry: Mutex<HashMap<u64, Arc<Mailbox>>>, // pid -> mailbox, for send
    workers: Vec<JoinHandle<()>>,
}
```

- **`spawn`**: build the coroutine (body = `apply(f, args)` on a fresh `Heap`
  sharing the runtime `Arc`s — same promotion as today), register its mailbox,
  push `Ready`. Returns the pid. Cheap (no thread).
- **worker loop**: pop a `Ready` process, set `CURRENT`, `resume()`. The coroutine
  runs until it either returns (`Done` → drop, deregister) or suspends at
  `receive` (`Waiting` → move into `parked`).
- **`send`**: lock the target mailbox, push the (copied) `Message`; if the target
  is `Waiting`, move it from `parked` back to `ready` (wake). Send to a dead pid is
  a no-op (Erlang semantics, as today).

### Send-ness & heap migration

A `Heap` is already `Send` (arena slabs, no `Rc`). The coroutine captures its
heap, so a parked `Process` is `Send` iff `corosensei::Coroutine` is `Send` for
our types (it is, when the stack and captured state are `Send`). So a process can
be stolen/run by any worker — one worker touches it at a time, satisfying
share-nothing. The shared `RUNTIME`/`PRELUDE` regions are `Sync` (boxcar +
RwLock), so concurrent workers reading code is already fine.

## Staging (each step keeps `cargo test` green)

1. **Single-worker suspending scheduler.** Add `corosensei`; one worker thread;
   global run queue; `receive` yields, `send` wakes. Proves the suspend/resume +
   mailbox/wakeup machinery and the thread-local yielder, *without* parallelism.
   The `processes` test passes now even on one worker (receive yields rather than
   blocks — no deadlock).
2. **N-worker pool.** Spin up ≈ `nproc` workers sharing the run queue
   (`Mutex<VecDeque>` + `Condvar`). Proves real parallelism and heap migration.
   `-j N` sets the count.
3. **Work-stealing** (optimization). Per-worker deques + steal-on-idle, to cut run-
   queue contention. Optional; only if profiling shows the global queue hurts.
4. ✅ **Reduction-counted preemption** (fairness — ADR-027). Scheduling is no
   longer cooperative-only: `eval`'s `'tail:` loop decrements a per-worker
   *reduction* counter (`process::tick`, budget ≈ 2000) and the process yields its
   worker when it hits zero — a CPU-bound process with no `receive` (e.g. an
   infinite loop) can no longer monopolise a core. The yield carries a `Suspend`
   reason (`Receive` → park on the mailbox; `Preempt` → re-queue Ready), so a
   preempted process goes to the back of the run queue and peers get a turn. The
   root thread has no yielder, so it just refreshes its budget — never preempted.
   This was exactly the **additive** step the model promised (no redesign).

## How this compares to the BEAM (what we copy, what we defer)

The target shape is Erlang's, lean:

| BEAM | This plan |
|---|---|
| one scheduler thread per core, **per-scheduler run queues** | worker pool ≈ core count; **single shared run queue** to start (per-worker + stealing = stage 3) |
| **reduction-counted preemption** (yield every ~2000 calls) | ✅ implemented (ADR-027): `eval` decrements a per-worker budget (~2000) and yields at zero |
| `receive` suspends until a message arrives | same |
| process migration / work-stealing across schedulers | deferred (stage 3); `Heap` is `Send`, so migration is *possible* from day one |
| per-process generational copying GC | per-process arena + top-level reset (ADR-016); tracing GC deferred (Path B) |
| dirty schedulers for long native calls | not needed yet (our builtins are short) |

So we're "BEAM-minus-preemption-minus-migration" at first — both are additive later, not redesigns.

## Risks & open questions

- **`unsafe` via the crate.** corosensei does the stack-switching `unsafe`; we
  audit the integration, not the mechanism (ADR-014 allows the crate).
- **Panic in a process.** A process that panics (Rust panic, not a Lisp error —
  Lisp errors are `Result`) must not take down its worker. Resuming a panicked
  coroutine: catch/propagate so the worker survives and the process dies cleanly.
- ~~**Cooperative starvation** until preemption lands.~~ Resolved — stage 4
  (reduction-counted preemption, ADR-027) landed; a CPU-bound process now yields
  its worker every ≈2000 reductions.
- **Introspection semantics.** `spawn-count` = green processes; `peak-threads`
  becomes "peak busy workers" (≤ pool size) — update `std/tool/test.blsp`'s summary
  and the wording we just fixed.
- **Stack size.** corosensei stacks are configurable; pick a small default
  (processes should be cheap) with growth/guard pages, and revisit under load.
- **Determinism.** Parallel scheduling makes interleavings nondeterministic; the
  test framework already tolerates this (results aggregate by message).

## Out of scope (explicitly deferred)

Precise mid-eval GC (needs Path B / scannable roots), supervision/links,
work-stealing (stage 3), and cross-node distribution. None block 4b.
(Reduction preemption was deferred here originally; it has since landed — ADR-027.)

**Work-stealing note:** stage-3 work-stealing was *deliberately removed* (the
current scheduler pins each process to one worker — `2abf05e`) because
cross-thread coroutine resume was the last slice of the KI-1 race. The
root-cause analysis and the invariants a reintroduction must honour are in
[`concurrency-v2.md`](concurrency-v2.md).

**Placement (since pinning is for life):** because a process never migrates, the
*only* load-balancing lever is **where it's first pinned**, decided once at spawn
by `assign_worker` (`scheduler.rs`). The policy is **least-loaded with a rotating
start**: scan the per-worker queues from a round-robin offset (`NEXT_WORKER`) and
pick the lightest, breaking ties toward the rotation. A worker's load is its
runnable-queue length **plus 1 if it's currently inside `resume`** (the
`WORKER_BUSY` gauge) — so a worker draining one CPU-bound process reads as loaded
even with an empty queue, instead of looking idle. When the pool is idle this
degrades to plain round-robin (so N spawns onto N idle cores land one-per-core);
when one worker is backed up, fresh processes steer to idle cores. Two caveats
follow from pinning: it balances *process count*, not *CPU load* (a count-balanced
placement can still be load-skewed and can't self-correct — that needs migration,
the deferred work-stealing above), and under heavy concurrent spawning the relaxed
`NEXT_WORKER` rotation + `try_lock` queue sampling make it *approximately*, not
exactly, round-robin.
