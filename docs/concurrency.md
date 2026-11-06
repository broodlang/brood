# Concurrency — green processes on all cores

> Status: **implemented — all phases.** Green M:N on a worker pool, preemptively
> fair, with selective `receive` + timeouts, monitors **and** links/`trap-exit`
> (ADR-067), userland supervision (`std/proc/supervisor.blsp`), registered
> names, general work-stealing + live migration (ADR-100, state capture — the
> original coroutine substrate is gone), and full distribution (closure
> shipping, distributed links/monitors, node-down detection). This doc is the
> original design rationale; the current engine is described in
> [`scheduler.md`](scheduler.md) (and ADR-100 in [`decisions.md`](decisions.md)).

## Goal

Actor-model concurrency, kept lean:

- **Use all cores** — N scheduler threads (one per core), true parallelism.
- **Green processes** — lightweight, cheap (spawn lots of them), not OS threads.
- **Share-nothing** — each process owns its memory; no shared mutable state, no
  locks on the hot path. Because they share nothing, **any ready process can run
  on any scheduler** (work-stealing).
- **Message passing** — processes talk by sending values to each other's mailbox.
- **Not** a single event loop. Node's model — one thread where a slow call
  stalls everything — is exactly what we're avoiding. Many schedulers; a hot
  process ties up at most one core, never the whole runtime.

The shape is N schedulers × many green processes, minus the fancy parts (see "Out of scope" below).

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

Surface (in Brood):

| Form | Meaning |
|---|---|
| `(spawn expr)` | start a new process running `expr`; returns a process handle/id |
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
  **deep-copied** into a `Send` form and rebuilt on the other side (share-nothing
  copies on send anyway).
- **`Send` per-process heaps** (a `gc-arena` arena per process, or `Arc` values)
  — lets a running process migrate to any scheduler. This is **the same GC work
  we already planned**, so concurrency and the GC migration become one effort.

**Lean recommendation:** prototype the **pinned-process + copy-on-send** model
first (works with today's `Rc`); move to `Send` per-process heaps when we do the
GC migration, which unlocks full migration.

### Shared code, isolated data

Function/macro **definitions are shared** (read-mostly, and
hot-swappable — your "edit on the fly"), while each process's **data is private**
to its heap. We need a clear split between the shared code table and per-process
data. Messages are data, so they copy cleanly.

## Scheduling

We started **cooperative** (a process yielded only at `receive`) and have since
added **reduction-counted preemption** (ADR-027): `eval`'s loop decrements a
per-worker budget and the process yields at zero, so a CPU-bound process can't
monopolise a core even on a small pool. This is the standard actor-runtime fairness
mechanism, done as the additive step the original design anticipated.

## Out of scope for v1 — all since shipped

Everything v1 deliberately skipped has since landed, additively as predicted:
supervision trees + `link` + restart strategies + registered names
(`std/proc/supervisor.blsp`, ADR-044/063/067), distribution across nodes
(ADR-033/034/088/089), live migration of running processes, and general
work-stealing (both via the state-capture scheduler, ADR-100).

## Impact on the roadmap

This is the largest *core* undertaking in the project. Two consequences:

1. It pulls the **GC migration earlier** — `Send` per-process heaps (`gc-arena`)
   are the path to full work-stealing, so the GC and concurrency are one effort.
2. Until then, prefer adding language features **in Brood** (string/math/seq
   libraries, maps) that don't deepen the recursive evaluator, so the eventual
   suspension work (coroutines or stepping VM) stays small.

## Phasing

1. ✅ **`spawn`/`send`/`receive`/`self` + message passing** — implemented in
   `process.rs` (step 4a). Each process is its own **OS thread** with its own
   `Heap` (real parallelism, real isolation); messages cross as a `Send`
   `Message` (deep copy) rebuilt in the receiver's heap; a global registry maps
   pid → mailbox. The mailbox is registered in the parent before the thread
   starts (so a `send` right after `spawn` can't race).
2. ✅ **Green M:N** — processes run on a pool of ≈`nproc` worker threads (a
   setting; `-j` overrides), suspending at `receive` rather than blocking. Cheap
   spawn, bounded OS threads, no `Gate` deadlock. `Send` per-process heaps let a
   process migrate between workers. (Landed on `corosensei` stackful coroutines,
   ADR-018; the substrate was replaced 2026-06-08 by state capture — a paused
   process is plain heap data — ADR-100.) See `docs/scheduler.md`.
3. ✅ **Reduction-counted preemption** (ADR-027) — `eval`'s loop decrements a
   per-worker budget (≈2000) and the process yields its worker at zero (`Suspend`
   carries `Receive` vs `Preempt`), so a CPU-bound process can't monopolise a
   core. Scheduling is now preemptively fair, not cooperative-only. Native code is
   preempted by unwinding its activations into interpreter frames, and a fold walks in
   spans that end between elements (ADR-401/403). A spent budget is paid only while
   another process is waiting for a worker: with none queued a yield would buy no
   latency, so the quantum goes on as a fresh one. The policy lives in one module,
   `process/scheduler/quantum.rs`.
4. ✅ **Selective `receive`** (ADR-027) — `receive` takes pattern clauses (the
   `match` grammar) + an optional `(after ms …)` timeout; it scans the mailbox,
   runs the first match, and leaves non-matching messages queued. A green process
   waiting on a timeout is woken by a dedicated timer thread. Timeouts are
   catchable (`throw` from the `after` body → `try`/`catch`).
5. ✅ **Work-stealing** — per-worker run queues + steal-on-idle; general since
   the state-capture cutover (any queued process, ADR-100; `(dev/steal-count)`
   observes it).
6. ✅ **Process monitors** — `monitor`/`demonitor`/`ref`: a unidirectional watch
   that delivers `[:down mref pid reason]` to the monitoring process when `pid`
   dies (`:noproc` if already dead), in `process.rs`. The one supervision
   mechanism that needs a primitive; the rest is Brood (`std/proc/`).
7. ✅ Links + `trap-exit` (ADR-067) — including **atomic `spawn-link`**, which links
   the child before it can run so its true exit reason (not a racy `:noproc`) always
   reaches the parent — userland supervision trees
   (`std/proc/supervisor.blsp` — `:one-for-one`/`:one-for-all`/`:rest-for-one`),
   and registered names (`proc/register`/`proc/whereis`/`(spawn :name …)`) all shipped.

## Distribution across nodes (slice 1 implemented)

Distribution falls out of share-nothing + copy-on-send: **the
network is just a longer copy.** Slice 1 is **done** — two runtimes connect over
TCP and message each other (ADR-034; full reference in
[`distribution.md`](distribution.md)). What landed:

- ✅ **Nodes** — named runtimes (`node/start`/`node/connect`, `name@host`) that link
  over TCP or a Unix socket, authenticated by a shared cookie (HMAC handshake, then an
  encrypted session, ADR-034/089).
- ✅ **Pids carry node identity** — `Value::Pid { node, id }` instead of a bare
  `u64`. `send` is **location-transparent**: local pid → the local registry;
  remote pid → serialize and forward over the link; the peer deserializes into a
  local mailbox. A `{:name :node}` address bootstraps a peer before you hold its
  pid.
- ✅ **A wire codec for `Message`** — reuses the heap-crossing deep-copy.
  **Symbols travel by name** (not by local interned id — each node has its own
  interner) and re-intern on arrival.
- ✅ **Code distribution** — closures travel as data (ADR-033): the wire codec
  ships a closure's code + captured locals (`M_CLOSURE`, `dist/wire.rs`), so
  `node/spawn` works.
- ✅ **Distributed links/monitors and node-down detection** — cross-node
  `monitor`/`link` fire `[:down … :noconnection]`/`[:EXIT pid :noconnection]`
  on link teardown; `node/monitor` delivers `[:nodedown name]`
  (see [`distribution.md`](distribution.md)).

Caveats every distributed runtime learns the hard way: partial failure / net-splits and
latency stay an application's concern; links are authenticated and encrypted (ADR-089), and the
handshake negotiates the wire version (ADR-492). This fits the project's "backend hosted remotely by a
frontend" premise — a remote frontend or second backend is just another node that
links and message-passes.

### Current limitations

The early limits are lifted — processes are cheap captured continuations (not
OS threads), they share the runtime's live code (ADR-013, no per-spawn prelude
reload), scheduling is preemptively fair with general work-stealing, `receive`
is selective with timeouts, closures `send` across processes and nodes
(ADR-033), and links + userland supervision ship (`std/proc/supervisor.blsp`;
the kernel-level supervisor, ADR-039, was tried and **reverted**). What's
genuinely still open:

- **A long-running native builtin can't be preempted mid-call** — reductions
  tick only at evaluator/VM safepoints, so a huge regex/map-build holds its
  worker for the call's duration (the dirty carve-out; genuinely-blocking natives
  go to the offload pool, ADR-144, `proc/offload`). The process yields as
  soon as the call returns (an overrun records an *owed* preemption, ADR-403), the
  folds are resumable between elements, and `(proc/system-monitor pid
  {:long-schedule-us n})` reports every quantum that ran long (ADR-404). The same
  holds for a pure numeric recursion in the JIT's unboxed register worker, which
  keeps its whole recursion in registers and cannot be unwound (ADR-401).
- **Mailbox and heap limits are opt-in** — `(proc/flag :max-heap n)` and
  `(proc/flag :max-mailbox n)` (ADR-307) bound one process and raise in that process
  only; senders are never blocked. A process that sets neither is caught only by the
  global ADR-043 memory cap.
- **Selective-receive scan cost** — testing a candidate rebuilds it into the
  LOCAL heap; non-matching messages leave short-lived garbage (reclaimed by the
  per-process GC). Negligible when the first message matches; optimisable later
  if a hot skip-heavy receive loop needs it.
