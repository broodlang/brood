# Concurrency — green processes on all cores

> Status: **implemented** (phases 1–4 below). Green M:N on a worker pool,
> preemptively fair, with selective `receive` + timeouts and process monitors.

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

Surface (in Brood):

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

We started **cooperative** (a process yielded only at `receive`) and have since
added **reduction-counted preemption** (ADR-027): `eval`'s loop decrements a
per-worker budget and the process yields at zero, so a CPU-bound process can't
monopolise a core even on a small pool. This is the BEAM's fairness mechanism,
done as the additive step the original design anticipated.

## Out of scope for v1 (the "fancy" we're skipping)

- Supervision trees, `link`, restart strategies, registered names
  (`monitor`/`demonitor` are now in — see Phasing)
- Distribution across machines/nodes
- Live migration of *running* processes (we start pinned)
- Work-stealing across workers (one shared run queue for now)

These are all additive later.

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
2. ✅ **Green M:N** — processes are now stackful coroutines (`corosensei`) on a
   pool of ≈`nproc` worker threads (a setting; `-j` overrides), suspending at
   `receive` rather than blocking. Cheap spawn, bounded OS threads, no `Gate`
   deadlock. `Send` per-process heaps let a process migrate between workers. See
   `docs/scheduler.md` / ADR-018.
3. ✅ **Reduction-counted preemption** (ADR-027) — `eval`'s loop decrements a
   per-worker budget (≈2000) and the process yields its worker at zero (`Suspend`
   carries `Receive` vs `Preempt`), so a CPU-bound process can't monopolise a
   core. Scheduling is now preemptively fair, not cooperative-only.
4. ✅ **Selective `receive`** (ADR-027) — `receive` takes pattern clauses (the
   `match` grammar) + an optional `(after ms …)` timeout; it scans the mailbox,
   runs the first match, and leaves non-matching messages queued. A green process
   waiting on a timeout is woken by a dedicated timer thread. Timeouts are
   catchable (`throw` from the `after` body → `try`/`catch`).
5. ⬜ **Work-stealing** — per-worker run queues + steal-on-idle (today: one shared
   run queue). An optimisation, not a correctness need.
6. ✅ **Process monitors** — `monitor`/`demonitor`/`ref`: a unidirectional watch
   that delivers `[:down mref pid reason]` to the monitoring process when `pid`
   dies (`:noproc` if already dead), in `process.rs`. The one supervision
   mechanism that needs a primitive; the rest is Brood (the `hatch` library).
7. ⬜ Later: links, supervision trees, registered names; work-stealing.

## Distribution across nodes (slice 1 implemented)

Erlang-style distribution falls out of share-nothing + copy-on-send: **the
network is just a longer copy.** Slice 1 is **done** — two runtimes connect over
TCP and message each other (ADR-034; full reference in
[`distribution.md`](distribution.md)). What landed:

- ✅ **Nodes** — named runtimes (`node-start`/`connect`, `name@host`) that link
  over TCP, authenticated by a shared cookie (Erlang-style; a placeholder for
  real auth/TLS).
- ✅ **Pids carry node identity** — `Value::Pid { node, id }` instead of a bare
  `u64`. `send` is **location-transparent**: local pid → the local registry;
  remote pid → serialize and forward over the link; the peer deserializes into a
  local mailbox. A `{:name :node}` address bootstraps a peer before you hold its
  pid.
- ✅ **A wire codec for `Message`** — reuses the heap-crossing deep-copy.
  **Symbols travel by name** (not by local interned id — each node has its own
  interner) and re-intern on arrival.
- ⬜ **Code distribution** — remote `spawn` needs the function on the far node.
  The closure-as-data path (ADR-033) is the missing piece; the wire codec rejects
  a `Closure` for now.
- ⬜ **Later** — distributed links/monitors and node-down detection.

Caveats Erlang learned the hard way, still to address: security (the cookie is a
placeholder — auth/TLS later), partial failure / net-splits, serialization
versioning, latency. This fits the project's "backend hosted remotely by a
frontend" premise — a remote frontend or second backend is just another node that
links and message-passes.

### Current limitations (to lift later)

Steps 4a→4b and ADR-027 lifted the early limits — processes are now cheap green
coroutines (not OS threads), they share the runtime's live code (ADR-013, no
per-spawn prelude reload), scheduling is preemptively fair, and `receive` is
selective with timeouts. What's still open:

- **Messages are data only** — you can't `send` a function (closures are
  per-heap). Send a *symbol* naming a top-level function instead; code is shared,
  so the receiver resolves it.
- **One shared run queue** — no work-stealing yet (phase 5). Fine until run-queue
  contention shows up in a profile.
- **No `link` and no supervision trees** (phase 6). `monitor` and registered
  names *are* in (see ✅ above; `(register name pid)` / `(spawn :name expr)` /
  `(whereis name)`). The kernel-level supervisor with mid-iteration retry
  (ADR-039) shipped briefly and was **reverted** — see
  [`supervision.md`](supervision.md). A process death prints to stderr and
  fires `[:down …]` to monitors; recover-on-throw is userland (`spawn` +
  `monitor` in ~10 lines).
- **Selective-receive scan cost** — testing a candidate rebuilds it into the
  LOCAL heap; non-matching messages leave short-lived garbage (reclaimed at the
  next top-level arena reset, ADR-016). Negligible when the first message matches;
  optimisable later if a hot skip-heavy receive loop needs it.
