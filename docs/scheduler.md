# Step 4b — green M:N scheduler

> Status: **implemented — and since superseded in substrate.** This doc is the
> 4b build plan as it landed on the **`corosensei` coroutine** substrate
> (ADR-018, ADR-027). On **2026-06-08 the stepping-VM endgame shipped**
> (ADR-100, [`decisions.md`](decisions.md)): corosensei is
> **deleted**, a paused process is captured as relocatable heap data
> (`Suspended` — bytecode frames + operand stack), work-stealing is **general**
> (any queued process, not fresh-only), and **live cross-worker migration
> works**. Reduction-counted preemption (ADR-027) carries over unchanged; a
> native-nested `receive` blocks its worker instead (the dirty carve-out,
> concurrency-v2 §7.4). Sections below that narrate coroutines/pinning are the
> as-built history of the corosensei era — read `concurrency-v2.md` §8 and
> `crates/lisp/src/process/scheduler.rs` for the current engine. The
> *rationale* lives in [`concurrency.md`](concurrency.md).

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
3. ✅ **Work-stealing.** Per-worker queues + steal-on-idle — landed fresh-only
   first (2026-06-07), generalised to any process by the state-capture cutover
   (ADR-100, 2026-06-08).
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

| BEAM | Brood (as shipped) |
|---|---|
| one scheduler thread per core, **per-scheduler run queues** | ✅ worker pool ≈ core count with **per-worker run queues** + steal-on-idle |
| **reduction-counted preemption** (yield every ~2000 calls) | ✅ implemented (ADR-027): a per-worker budget (~2000, `BROOD_REDUCTIONS`); the JIT batches the countdown at loop back-edges |
| `receive` suspends until a message arrives | same (selective receive + `after` timeouts) |
| process migration / work-stealing across schedulers | ✅ general since ADR-100 (2026-06-08): a paused process is heap data — any queued process can be stolen, a woken process resumes on the least-loaded worker |
| per-process generational copying GC | ✅ shipped (ADR-055/061/072) — per-process nursery + tenured old gen |
| dirty schedulers for long native calls | the dirty-block carve-out (concurrency-v2 §7.4): a native-nested `receive` blocks its worker, which is excluded from placement and drains its backlog; an all-dirty pool grows an overflow drainer |

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

## Out of scope at 4b time (all have since landed)

Everything this plan deferred has since shipped: precise mid-eval GC (the
generational collector + any-depth safepoint, ADR-055/061), supervision/links
(`std/proc/supervisor.blsp`, ADR-044/063/067), general work-stealing + live
migration (ADR-100), and cross-node distribution (ADR-033/034/088/089).
(Reduction preemption was deferred here originally too; it landed as ADR-027.)

**Work-stealing note (history → current):** stage-3 work-stealing was first
*deliberately removed* (the scheduler pinned each process to one worker —
`2abf05e`) because cross-thread coroutine resume was the last slice of the KI-1
race, then reintroduced in its **fresh-only** safe form (steal only
never-resumed processes). The stepping-VM cutover (ADR-100, 2026-06-08) removed
the constraint entirely: with no native stack to pin, `try_steal` takes **any**
queued process from a backed-up peer, and the `fresh` flag is gone. Root-cause
analysis, invariants, and the full design are in ADR-100
([`decisions.md`](decisions.md)).

**Placement + stealing (the load-balancing levers today):** spawn placement is
scan-free — a process spawned from a worker lands on that worker's queue, else
round-robin (`pick_spawn_worker`); an idle worker steals from the back of a
backed-up peer's queue (`try_steal`, rotating-start `try_lock` scan, gated by a
relaxed `STEALABLE` counter so a truly-idle pool re-parks cheaply). A *woken*
process is re-routed to the least-loaded worker (`assign_worker`: queue length
plus 1 if the worker is inside a quantum — the `WORKER_BUSY` gauge — with a
direct-handoff optimisation that elides cross-thread futex wakes on
send-then-receive ping-pong); a *preempted* process re-enqueues on its own
worker for cache locality. Dirty-blocked workers (a native-nested `receive`)
are excluded from placement and drain their stranded backlog onto peers.
Observable via `(steal-count)`.
