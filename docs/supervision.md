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
  `:one-for-one`.
- `(link pid)` / `(unlink pid)` / `(trap-exit on)` / `(spawn-link expr)` —
  **symmetric** failure coupling (Erlang links, ADR-067), **local or cross-node**.
  A linked peer's death takes you down too (abnormal reason) or arrives as a
  trappable `[:EXIT pid reason]` message if you `(trap-exit true)`; a cross-node
  link fires `:noconnection` on net-split. This is what makes a supervisor's *own*
  death tear its children down (propagation) — the orphan fix monitors couldn't
  provide — and lets supervision span nodes (see §Cross-node supervision).

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
that starts a set of children, `link`s + traps each (ADR-067), and restarts them
per a strategy and restart type, bounded by a restart-intensity limit. It is
**pure Brood policy over `spawn` / `link` / `trap-exit` / `receive` / `exit`** —
the mechanism-in-Rust / policy-in-Brood rule (ADR-006); the only kernel surface it
needed was the general Erlang primitives (`monitor`/`exit/2`/`link`/`trap_exit`),
never a supervision-specific hook.

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

### Cross-node supervision (distributed links)

Links span nodes (ADR-067), so a supervisor on one node can supervise a child on
another. `link`/`unlink`/`exit` accept a remote pid and route over the dist link
(`Frame::Link`/`Frame::Unlink`/`Frame::Exit`); a remote child's crash arrives as a
link `[:EXIT]` and restarts, the supervisor's own death tears the remote child
down, and a **net-split** fires `:noconnection` to the local side (the same
semantics a remote monitor has). The supervisor logic is identical to the local
case — it just links pids.

One ergonomic gap: a child `:start` must *return* the (remote) child's pid, but
`remote-spawn` is fire-and-forget (returns `nil`). So a remote-child spec obtains
the pid via a roundtrip today — e.g. ask a remote factory to spawn the worker and
reply its pid:

```clojure
{:id :w :restart :permanent
 :start (fn () (let (me (self))
                 (send {:name :factory :node :a} [:make me])
                 (receive ([:made pid] pid))))}   ; returns the remote worker's pid
```

A synchronous `remote-spawn` that returns the pid (making this turnkey) is the one
deferred follow-up. End-to-end coverage in `crates/cli/tests/distribution.rs`.

#### Still simplified (ADR-011)

- **`link` + `trap_exit` now exist (ADR-067)** — so a supervisor's *own* crash/kill
  propagates down the links and tears the subtree down (workers die by propagation;
  a child sub-supervisor traps and recognises its parent's `[:EXIT]`). The
  `:shutdown :infinity` cascade above still governs a *graceful* `stop-supervisor`
  (a deliberate hard `:kill` is untrappable). See §How it differs for the full
  picture. What's still absent is a `terminate/2`-style cleanup hook on an external
  kill.
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

## How it differs from Erlang/OTP & Elixir

Brood's process substrate is genuinely Erlang-family — per-process heaps,
copy-on-send messages, a preemptive reduction-counting scheduler, let-it-crash,
`monitor`/`[:down]`, `(exit pid reason)` (Erlang `exit/2`, incl. the untrappable
`:kill`), and a registered-name table dropped on death. So the differences below
are deliberate design choices, not missing plumbing.

| Dimension | Erlang/OTP & Elixir | Brood |
|---|---|---|
| What a supervisor *is* | A sealed generic OTP **behaviour** (runtime stdlib) | ~230 lines of **editable Brood**, hot-reloadable |
| Failure-coupling primitive | **Links** (bidirectional) + `trap_exit` | **Links + `trap_exit`** (ADR-067) *and* monitors — `link`/`unlink`/`trap-exit`/`spawn-link` |
| Supervisor dies → its children | Auto-killed via links | **Auto-killed via links** (ADR-067) — propagation, like OTP |
| Graceful child cleanup | `trap_exit` → `terminate/2`, deadline-enforced | Cooperative `[:$stop]` *message*; no forced signal, no cleanup callback |
| Strategies | one/all/rest_for_one + `simple_one_for_one`/DynamicSupervisor | one/all/rest_for_one only |
| Restart types | permanent / transient / temporary | **same** |
| `:shutdown` | brutal_kill / infinity / ms; default by child `type` | brutal-kill (default) / infinity / ms; **no type-derived default** |
| Shutdown order | **reverse** start order | start order |
| Startup | synchronous, ordered, **rollback on failure** | async (returns the spawned pid); a throwing `:start` orphans earlier children |
| Named children | supervisor-managed names; survive restart transparently | `register` exists, but **not supervisor-managed** |
| Runtime child mgmt | start/terminate/restart/delete/count_children | `start-child`/`terminate-child`/`restart-child`/`count-children`/`which-children` (ADR-067) |

### The load-bearing difference (now closed): links + `trap_exit`

This *was* the deepest difference, and it's now resolved (ADR-067). Like Erlang,
the supervisor `link`s its children and `(trap-exit true)`s, so a child death
arrives as a trappable `[:EXIT pid reason]` message **and the supervisor dying
propagates exit signals down the links, killing the children automatically**. The
kernel still also offers `monitor` (the one-way notification) for watchers that
shouldn't be coupled.

What this fixed: **a supervisor's own death now tears its subtree down.** A
*crash* (or external `(exit sup …)`) propagates through the links — workers die by
propagation; a child **sub-supervisor** traps, recognises its parent's `[:EXIT]`,
and tears its own subtree down (it records the caller as `:parent` at
`start-supervisor`). Previously this orphaned the whole subtree; it was the single
biggest gap and was structural — only links closed it. The `:shutdown :infinity`
cascade still governs a *graceful* `stop-supervisor` (a deliberate hard `:kill` is
untrappable, so a sub-supervisor opts into the cooperative `[:$stop]` path).

The one remaining piece of this axis is **`terminate/2`-style cleanup**: a worker
still can't run orderly cleanup on an *external* kill (a `:kill` is untrappable);
a cooperative worker that handles `[:$stop]` can, under `:shutdown :infinity`/ms.

### Faithful

`:one-for-one` / `:one-for-all` / `:rest-for-one` match OTP semantics (incl. the
trigger's restart type gating a group restart, and per-member type within it);
`:permanent` / `:transient` / `:temporary`; the restart-intensity window
(defaults 3-in-5, matching Elixir); the `:shutdown` *vocabulary*; nested-tree
crash **escalation** (a sub-tree that exhausts its budget dies and the parent
restarts the whole sub-tree).

### Divergent (beyond the links gap)

- **Startup is async with no rollback** — `start-supervisor` returns the spawned
  pid immediately; a throwing `:start` thunk crashes the supervisor and orphans
  whatever already started (Erlang's `start_link` is synchronous and rolls back).
- **Intensity counts one event per trigger** (a group restart = 1 tick), not one
  per child restarted.

(Two earlier divergences are now resolved: shutdown is **reverse start order**
like OTP, and a `:name` in a child spec is **supervisor-managed** — re-registered
on each restart so callers address a stable name. See §Shutdown policy and the
parity list below.)
- **No dedicated `simple_one_for_one` mode** — but the **runtime child API**
  (`start-child`/`terminate-child`/`restart-child`/`count-children`) covers the
  DynamicSupervisor use case: a supervisor started with `[]` children and grown at
  runtime *is* a dynamic supervisor, under any strategy.
- **No child `type` / `modules` / `significant` / `auto_shutdown`**, and **no
  `code_change`/release upgrades** (though ADR-013 late binding gives a different
  hot-reload: redefining the module changes a *running* supervisor on its next
  message — but captured `:start` closures keep their old code until restarted).

### Where Brood is arguably nicer

The whole supervisor is ~250 lines of readable, redefinable Brood — no opaque
behaviour, immutable-state-through-the-loop instead of `gen_server` callback
ceremony — and it forced the kernel to grow only **general** Erlang primitives
(`monitor`, `exit/2`, now `link`/`trap_exit`) rather than a supervision-specific
kernel feature (the path ADR-039 took and reverted).

### Path to OTP parity (roughly by value)

1. ✅ **`link` + `trap_exit`** (done 2026-05-30, ADR-067) — automatic subtree
   teardown when a supervisor *crashes*, not just on graceful stop. Added as the
   general Erlang primitives (`link`/`unlink`/`trap-exit`/`spawn-link`), not a
   supervision-specific hook (the ADR-039 lesson); link teardown rides the cold
   `deregister` path, no new scheduler-global state.
2. ✅ **Supervisor-managed registration** (done 2026-05-30) — a `:name` keyword in
   the child spec, registered to the fresh pid on every (re)start, so callers
   address a stable name via `whereis`. Pure Brood over the existing `register`.
3. ✅ **Reverse-order shutdown** (done 2026-05-30) — `terminate-many` tears down
   last-started-first.
4. ✅ **DynamicSupervisor + a runtime child API** (done 2026-05-30, ADR-067) —
   `start-child`/`terminate-child`/`restart-child`/`count-children`; a supervisor
   grown from `[]` at runtime is a dynamic supervisor.
5. **A worker cleanup convention** layered on `[:$stop]` (a `terminate`-style hook)
   — the last remaining item.

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
