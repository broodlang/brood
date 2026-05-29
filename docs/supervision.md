# Process supervision

> **Status: reverted (2026-05-29).** A kernel-level supervisor (resume slots,
> mode-gated automatic retry, hot-reload-on-retry) was implemented under
> ADR-039 on 2026-05-28 and **stripped on 2026-05-29** (commit `e3d3a0d`). The
> kernel is back to Erlang-style let-it-crash: an uncaught error kills the
> process, monitors fire `[:down …]`, no automatic retry. See
> [`decisions.md`](decisions.md) ADR-039 for the rationale; the design that
> *was* tried is preserved in git history (`git show e3d3a0d -- docs/supervision.md`).

## Why it was reverted

The kernel supervisor was the largest contributor to the multi-thread
scheduler race (KI-1) — its RESUME_SLOT thread-local + safepoint rooting +
mid-iteration retry created a wide window of shared mutable scheduler state.
Stripping it cut the `recurse.blsp` failure rate from ~24 worker deaths per
run (0/n clean) to ~0–1 per run (5/10 clean) before the heap rewrite, and the
follow-on Phase-1 bump-only allocator (`f90f0de`) brought it to 10/10 clean
in debug-assertions release.

The decisive trade was: a kernel feature that was load-bearing for *only* the
hot-reload-on-retry story, versus the race that blocked **every** fan-out
program. Keep the race fix; let supervision move to userland.

## What's possible today

The Erlang-style **building blocks** are still here — they were never the
supervisor itself, just the substrate it was built over:

- `(spawn expr)` — start a green process; an uncaught error kills it.
- `(monitor pid)` — watch a pid; receive `[:down ref pid reason]` when it dies.
- `(demonitor ref)` — drop a monitor.
- `(send pid msg)` / `(receive …)` — communicate.

A user wanting *recover-on-throw* writes a supervisor process in Brood:

```clojure
(defn supervise (worker-fn)
  (let (pid (spawn (worker-fn))
        ref (monitor pid))
    (receive
      ([:down ~ref _ :normal] :ok)
      ([:down ~ref _ reason]
        (println "child died: " (pr-str reason) " — restarting")
        (supervise worker-fn)))))
```

Add restart limits, backoff, or a structured policy on top — all in Brood,
no kernel surface to maintain.

## The supervisor library (`std/supervisor.blsp`)

The structured version of that pattern ships as a require-able module
(`(require 'supervisor)`, ADR-044). A supervisor is an ordinary green process
that starts a set of children, `monitor`s each, and restarts them per a strategy
and restart type, bounded by a restart-intensity limit. It is **pure Brood policy
over `spawn` / `monitor` / `receive`** — zero new kernel surface, the
mechanism-in-Rust / policy-in-Brood rule (ADR-006).

```clojure
(require 'supervisor)
(def sup (start-supervisor
           (list {:id :a :start (fn () (spawn (worker-a)))}
                 {:id :b :start (fn () (spawn (worker-b))) :restart :transient})
           {:max-restarts 3 :max-seconds 5}))
(which-children sup)    ; => list of {:id :pid :restart}
(stop-supervisor sup)   ; stop supervising (children keep running — see below)
```

A **child spec** is a map: `:start` (a 0-arg fn that spawns the child and returns
its pid), an optional `:id`, and a `:restart` type — `:permanent` (always
restart), `:transient` (restart only after an *abnormal* exit, reason ≠
`:normal`), or `:temporary` (never). The intensity window (`:max-restarts` in
`:max-seconds`) caps a crash loop: when exceeded, the supervisor exits abnormally
so a watcher's monitor fires.

### What it does *not* do (and why)

Both limits below are the same missing kernel capability — **there is no
kill/exit primitive** (no links, no `exit`), so a supervisor cannot terminate a
process it didn't crash:

- **Only `:one-for-one`.** `:one-for-all` and `:rest-for-one` must terminate
  *healthy* siblings when one dies; without a kill primitive that's impossible in
  userland. `start-supervisor` rejects them rather than silently degrading.
- **`stop-supervisor` doesn't stop the children.** It ends the supervisor's loop;
  the children keep running (orphaned). Likewise an intensity-exceeded shutdown
  orphans the surviving children instead of terminating them (Erlang terminates
  them).

These are the natural trigger for the *one* kernel hook supervision might
justify later — a minimal `exit`/link primitive — at which point the deferred
strategies become a pure-Brood addition. See
[`concurrency-v2.md`](concurrency-v2.md) §4.

## What's gone (vs. ADR-039 as proposed)

- **Kernel-driven automatic resume.** A throw inside an iteration no longer
  re-invokes `(callee, argv)` of the current call. The process dies.
- **Resume slots.** The runtime no longer captures `(callee, argv)` at every
  function call. (This was the per-call overhead the design's mode gate
  existed to avoid in release.)
- **Hot-reload-on-retry.** A `(def my-loop …)` between a throw and a retry
  no longer takes effect on the very next attempt — because there is no
  next attempt. Plain hot reload (next *call* sees the new binding) is
  unaffected — that's ADR-013, separate.
- **`%spawn-supervised` / `%spawn-supervised-named` primitives.** Gone.
- **`(supervise …)` macro in the prelude.** Gone (the *name* may be
  reused later for a userland supervisor helper; today it's not bound).
- **`*supervise-max-restarts*` / `*supervise-max-window-ms*` dyns.** Gone.
- **`BROOD_SUPERVISE=1` env / `(set-supervision! true)`.** Gone.
- **`nest run --watch` supervised re-entry.** A throw in the watched
  program now kills the session; editing the file re-spawns from scratch
  (which is also a cleaner model — no surprising state retention across edits).

## See also

- [`decisions.md`](decisions.md) — ADR-039 (the accept→revert record).
- [`docs/devlog.md`](devlog.md) — the strip is commit `e3d3a0d` (2026-05-28
  evening); the Phase-1 follow-on is `f90f0de` (2026-05-29 morning).
- [`scheduler.md`](scheduler.md) / [`memory-model.md`](memory-model.md) —
  the substrate the race lived on, now substantially simplified by the
  bump-only allocator.
- [`concurrency-v2.md`](concurrency-v2.md) — design for bringing supervisor
  trees (and work-stealing) back without reopening the race; favours a
  **userland Brood supervisor library** over a new kernel hook.
