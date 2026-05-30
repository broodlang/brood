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
- `(exit pid reason)` — **terminate another process** (Erlang `exit/2`, ADR-063).
  `:kill` is the untrappable hard kill (caught at the reduction tick, so it stops
  even a tight CPU loop); any other reason is the soft signal (the target dies at
  its next `receive`). Either way the target's monitors fire `[:down ref pid
  reason]`. This is the primitive that lets a userland supervisor terminate
  *healthy* siblings — the capability whose absence used to cap the library at
  `:one-for-one` (see below).

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
           {:strategy :one-for-all :max-restarts 3 :max-seconds 5}))
(which-children sup)    ; => list of {:id :pid :restart}
(stop-supervisor sup)   ; stop supervising AND terminate the children
```

A **child spec** is a map: `:start` (a 0-arg fn that spawns the child and returns
its pid), an optional `:id`, and a `:restart` type — `:permanent` (always
restart), `:transient` (restart only after an *abnormal* exit, reason ≠
`:normal`), or `:temporary` (never). The intensity window (`:max-restarts` in
`:max-seconds`) caps a crash loop: when exceeded, the supervisor exits abnormally
so a watcher's monitor fires.

### Strategies (all three, since `exit/2` landed)

The `(exit pid reason)` primitive (ADR-063) supplied the one missing capability —
terminating a *healthy* sibling — so the full OTP strategy set is now pure-Brood
policy. `start-supervisor` takes `:strategy`:

- **`:one-for-one`** (default) — restart only the crashed child.
- **`:one-for-all`** — restart every child: terminate the survivors, then restart
  the whole set.
- **`:rest-for-one`** — restart the crashed child and every child started *after*
  it (in start order); earlier-started children are left running.

For the group strategies the supervisor `(exit pid :kill)`s each healthy member it
must restart and **selectively drains that member's `[:down]`** (Erlang `receive`
keeps non-matching messages queued), so a deliberate kill is never mistaken for a
fresh crash. The crashed child's `:restart` type gates whether the procedure runs
at all; within a group restart each member is restarted only if its *own* type
permits — a `:temporary` sibling is terminated and dropped, not revived.

**`stop-supervisor` and intensity-exceeded both terminate the children now** (no
orphans): `stop-supervisor` kills every child as it leaves the loop, and a crash
loop that blows the intensity window terminates the survivors before the
supervisor throws (Erlang's shutdown behaviour).

### Shutdown policy + nested trees (`:shutdown`)

A child spec may carry a `:shutdown` field controlling *how* it's terminated:

- **`:brutal-kill`** (default) — `(exit pid :kill)`, untrappable. Right for an
  ordinary worker, which doesn't understand a graceful-stop message.
- **`:infinity`** — send the child `[:$stop]` and wait (forever) for it to exit.
- an **integer ms** — send `[:$stop]`, wait that long, then fall back to `:kill`.

This is what makes **nested supervision trees** tear down cleanly. A child whose
`:start` calls `start-supervisor` *is* a sub-tree (its pid is a supervisor), and
crash escalation already works through it (a sub-tree that exhausts its restart
budget dies and the parent restarts the whole sub-tree). The missing piece was
*deliberate* teardown: a hard `:kill` of a sub-supervisor bypasses its `[:$stop]`
handler, orphaning the grandchildren. Marking the sub-supervisor child `:shutdown
:infinity` fixes that — the parent sends `[:$stop]`, the sub-supervisor runs its
own `terminate-many` (recursively, depth-first), then exits. **Mark every
supervisor child `:shutdown :infinity`** (Erlang's exact rule); workers keep
`:brutal-kill`.

```clojure
(start-supervisor
  (list {:id :db-sub :restart :permanent :shutdown :infinity     ; a sub-supervisor
         :start (fn () (start-supervisor (list …) {:strategy :rest-for-one}))}
        {:id :worker :restart :permanent                          ; a plain worker
         :start (fn () (spawn (worker-loop)))}))
```

#### Still simplified (ADR-011)

- **No `link` / bidirectional exit propagation.** Termination is one-directional
  and supervisor-driven (`monitor` + `exit`), not Erlang's symmetric links. The
  `:shutdown` cascade covers the shutdown *direction*; what's absent is automatic
  *upward* propagation (a linked peer's crash taking you down without a monitor).
- **No broadcast-`[:$stop]`-to-everyone shutdown.** `:infinity`/ms is opt-in per
  child because sending `[:$stop]` to an arbitrary worker that pattern-matches
  broadly could be consumed as data — so only children that opt in receive it.
- **Intensity counts one event per trigger** (per group restart), not one per
  child restarted.

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
