# Handoff: blocking work must never pin a worker

**Status:** Phase 1 implemented (GUI observer). **The reusable seam now exists**:
`process::spawn_io_source(subscriber, name, |sink| …)` + `MailboxSink`
(`process/io_source.rs`) is the one place the thread-plus-`deliver` pattern lives.
**TCP sockets are its first consumer** (ADR-062, `crate::net` + `std/tcp.blsp`):
each socket reads on a dedicated non-worker thread and delivers `[:tcp …]` /
`[:tcp-closed …]` / `[:tcp-accept …]` to the owning process's mailbox. Still
planned: migrate `gui`/`dist`/terminal onto `spawn_io_source`; a `mio` reactor to
fold per-socket reader threads into one (Phase 2 scale); the Phase-3 offload pool.
See ADR-058/059/062.

## The problem

The green-process scheduler has a small **worker pool** (≈`nproc` OS threads;
`scheduler.rs`). Green processes are cheap — tens of thousands are fine — but a
worker is scarce. A green process that makes a **native blocking call** (a
`recv_timeout`, a blocking `read`, a synchronous FFI call) holds its worker for
the whole call: the green scheduler can't preempt a thread parked in a syscall.
So if as many processes as there are workers block natively at once, *every*
worker is stuck and the other thousands of processes starve.

A process parked in `(receive)` on an empty mailbox is the opposite: it is
**descheduled** (stored as the mailbox `waiter`), holding **no** worker, until
`mailbox::deliver` wakes it. That is the model everything blocking should use.

## The principle

> Anything that blocks runs on a **non-worker thread** and **delivers a message
> to the owning process's mailbox**; the process parks in `(receive)` (zero
> workers held) until woken.

This is already the runtime's *network* model: `dist` reads each `TcpStream` on a
dedicated `std::thread` and injects inbound messages via `mailbox::deliver`
(`dist/heartbeat.rs`, `process/mailbox.rs`). Phase 1 makes GUI input follow the
same pattern; Phases 2–3 generalize it.

## What the kernel already provides

- **`mailbox::deliver(pid, msg)`** (`process/mailbox.rs`) — push a message into a
  local process's mailbox and wake it (`enqueue` a parked process, or notify the
  root thread's condvar). Callable from *any* thread. Already the shared tail of
  `send`, monitor `[:down …]`, and inbound node-link delivery.
- **`receive` with `(after ms …)`** — the macro (`std/prelude.blsp`) and the
  `%receive` primitive (`process/mailbox.rs`) already support a timeout, so a
  periodic tick (e.g. the observer's live refresh) needs no extra machinery.
- **`Message` is a plain Rust enum** (`process/message.rs`: `Str`/`Keyword`/`Int`/
  `Vector`/…) and symbols are a global interner, so a non-Brood thread can build a
  message **without a heap**. `from_message` turns it into a `Value` when the
  process receives it.
- **`self_pid()`** (`process/scheduler.rs`) — a primitive (`(self)`) the GUI
  `open` reads to learn which process to deliver input to.

Note: the scheduler **pins each process to one worker for life — no migration**
(`scheduler.rs`). That's *why* deliver-to-mailbox is the right shape: a BEAM-style
"migrate to a dirty scheduler" design would be far more invasive, while
deliver-to-mailbox needs no migration at all.

## Phase 1 — GUI observer via the mailbox (done)

The observer loop is **unchanged**: the GUI display's `:poll` simply becomes a
`receive` instead of a blocking channel read, so it returns the same key/mouse
shapes `term-poll` does — but parks the process instead of pinning a worker.

- `gui-open` reads `self_pid()` and registers it as the window's **subscriber**.
- The GUI thread, on a key/mouse event, builds a `Message` shaped exactly like the
  poll return (`"a"`, `:up`, `[:mouse :press :left r c]`) and `mailbox::deliver`s
  it to the subscriber. (No per-window input channel; no `gui-poll`.)
- `(gui-display)`'s `:poll` is `(fn (ms) (receive (m m) (after ms nil)))` — park
  for the next input message, or time out for the live-refresh tick.
- `(observe)` spawns a process per window (ADR-056); each parks in `receive`, so
  an idle observer window holds **no worker** and hundreds can run at once.

## Phase 2 — generalize the input seam (planned)

- **Terminal input**: a reader thread delivers key messages the same way, lifting
  even the root-thread block — the "async input feeding a mailbox" ADR-046
  predicted. Then both frontends are mailbox-driven and the observer loop is
  uniform across them.
- **Sockets / dist**: replace the thread-per-connection blocking reads with a
  single `mio`/`epoll` **reactor** thread delivering read-ready/data messages.
  Same `deliver` primitive; scales connection count off the worker pool.

## Phase 3 — blocking offload pool (planned)

For calls that genuinely cannot be event-driven (blocking FFI, a synchronous C
library, blocking DNS): a small dedicated thread pool plus a primitive like
`(blocking (fn () …))` that runs the thunk off the worker pool and delivers its
result to the caller, which parks in `receive`. BEAM's "dirty scheduler," but
expressed through the same mailbox mechanism (no process migration required).

## Risks / decisions

- **Backpressure**: `deliver` is unbounded. Fine for keys/scroll (low rate; mouse
  *move* is already dropped). Sockets will want flow control in Phase 2.
- **Selective-receive cost**: `%receive` scans the mailbox per match — fine at
  input rates; note it for high-volume sources.
- **Dead-process cleanup**: `deliver` no-ops on a dead pid; a window whose
  subscriber has exited should be reaped (a monitor, or a liveness sweep). Minor
  follow-up.
- **`gui-poll` removed**: the observer uses `receive`; scripts/tests that want raw
  input open a window and `receive` in their own process (the root counts).
- **The display's `:poll` is process-bound.** `(gui-display)`'s `:draw`/`:size`/
  `:leave` act on the captured window id, but `:poll` is a `receive` on the
  *calling* process's mailbox (where `gui-open` registered delivery) — it ignores
  the window id. So a display must be created and polled by the **same** process;
  passing it to another process to run would read the wrong mailbox. (The terminal
  display has no such constraint — it's a constant over the single terminal.)
- **Input messages carry no window id.** A delivered key/mouse message is the bare
  `term-poll`-shaped value (`"a"`, `:up`, `[:mouse …]`) so the observer loop stays
  frontend-agnostic — but it means one process driving two windows can't tell their
  input apart. Hence one process per window (`(observe)` spawns accordingly). If a
  future multi-window-single-process app needs it, tag deliveries with the window
  id and unwrap in that app's loop (keeping the bare shape for the observer).
