# `nest test --max-parallel 1` deadlocks tests that wait on a spawned process

## Summary

Under `nest test` with a single worker thread (`-j1` / `--max-parallel 1`), a test that
spawns a process and then blocks in `receive` waiting for that process to send it a
message **times out** — the spawned process never makes progress while the test is
parked. The test's own `(after …)` fires and the test fails.

It is **not** specific to any library or to request/reply: the spawned process need only
send *one* message back. It passes under `nest run` at any `-j`, and under `nest test`
with enough workers. The pass/fail count as `-j` increases is **non-monotonic**
(`-j3` can fail where `-j2` partly passed), which points at a worker-dispatch/wakeup bug
rather than a simple "not enough threads" capacity limit.

## Minimal repro (no dependencies)

`j1_repro_test.blsp` (saved at the repo root):

```lisp
(defmodule j1-repro-test (:use test))

(describe "spawned process delivers a message to the test"
  (test "spawn a process that sends me one message, then receive it"
    (let (me (self))
      (spawn (send me :hi))
      (assert= (receive (:hi :ok) (after 1000 :timeout)) :ok))))
```

```
$ nest test j1_repro_test.blsp --max-parallel 1
    actual: :timeout
1 tests, 0 passed, 1 failed
$ nest test j1_repro_test.blsp --max-parallel 2
1 tests, 1 passed, 0 failed
```

The spawned process never runs `(send me :hi)` (or its delivery never wakes the parked
test) while the single worker is parked inside the test's `receive`.

## What works vs fails

| Scenario | Result |
|---|---|
| `nest test … --max-parallel 1` | ❌ test's `receive` times out |
| `nest test … --max-parallel 2` (single test) | ✅ |
| `nest test …` (default parallelism) | ✅ |
| `nest run … --max-parallel 1` (same logic on the **root** thread) | ✅ |
| `nest run … -j1`, call made from a **spawned** process | ✅ |

A `(sleep N)` between the `spawn`/`send` and the `receive` does **not** help — so it is
not scheduling latency; the callee simply never gets the worker.

## Scaling probe — non-monotonic

Three independent tests in one `describe`, each spawning a child that messages it back
(so they run concurrently in one parallel batch):

```lisp
(describe "g"
  (test "a" (let (me (self)) (spawn (send me :x)) (assert= (receive (:x :ok) (after 800 :timeout)) :ok)))
  (test "b" (let (me (self)) (spawn (send me :x)) (assert= (receive (:x :ok) (after 800 :timeout)) :ok)))
  (test "c" (let (me (self)) (spawn (send me :x)) (assert= (receive (:x :ok) (after 800 :timeout)) :ok))))
```

| `-j` | passed / failed |
|---|---|
| 1 | 0 / 3 |
| 2 | 1 / 2 |
| 3 | 0 / 3 |
| 4 | 3 / 3 passed |

`-j1` is deterministic (always all-fail). The middle is erratic (`-j3` worse than `-j2`),
which strongly suggests a **worker fails to pick up a process that became runnable while
the worker was parked/busy**, rather than a clean capacity bound.

## Analysis / likely mechanism

`--max-parallel N` calls the same `brood::process::set_max_parallel(n)` for both `nest
run` and `nest test` (`crates/cli/src/main.rs`, `crates/nest/src/main.rs`), so the cap
itself is identical — the trigger is the **execution shape**:

- `nest run`: the user code runs on the **root thread**, which blocks on a mailbox condvar
  in `receive` (`scheduler.rs`: "The root thread (REPL / file runner) instead blocks on
  its mailbox condvar"). The single worker is then free to run the spawned process. ✅
- `nest test`: each test body runs in a **spawned green process**. Per the scheduler docs,
  a green process doing `receive` on an empty mailbox "Suspended … and returns the worker
  to the pool." The bug appears to be that **when the only worker's current green process
  parks in `receive`, a *runnable* green process (the freshly-spawned child, or one woken
  by a `send`) is not dispatched onto that now-free worker** — so with one worker, nothing
  ever runs the child, and the test's `(after …)` fires.

The non-monotonic scaling suggests the same defect at the margin: a process made runnable
while all workers are parked/busy isn't reliably picked up when a worker frees up.

### Pointers
- `crates/lisp/src/process/scheduler.rs` — the worker loop, park-on-idle, and run-queue
  dispatch; check what a worker does after its current process suspends in `receive`
  (does it re-poll the global/own run queue, or park without redispatch?).
- `crates/lisp/src/process/mailbox.rs` — `wake_parked` / `wake_enqueue` / the
  `ST_RUNNABLE`/`ST_RUNNING` transitions; check that enqueueing a runnable process unparks
  an idle worker (and that a worker that just parked its own process is itself eligible).
- `crates/lisp/src/process/scheduler.rs` work-stealing path (and the `work_stealing.rs`
  test) — whether the single-worker case has a steal/redispatch gap.

## Impact

Any `nest test` suite containing tests that talk to spawned processes cannot run at
`-j1`. Default parallelism and `-j ≥ 2` (enough for the suite's peak concurrent
inter-process tests) are fine, so practical impact is low — but it's a footgun (`-j1`
silently turns inter-process tests into timeouts) and a real scheduler correctness gap.

## Notes

- Reproduced on the current `brood` working tree via `nest test`.
- Discovered while adding supervised-process tests to the `hatch` framework; the pre-existing `hatch` registry tests fail under `-j1` for the same reason (verified against the committed tree), confirming it predates and is independent of that work.
