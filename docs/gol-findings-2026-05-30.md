# Game of Life — performance & concurrency findings (2026-05-30)

Findings from an AI assistant (Claude Code) building and then trying to
parallelise a Game of Life in Brood (`foobar/src/life.blsp`, 80×40 toroidal,
set-of-live-cells representation `{[x y] true}`). Measured against the live image
via the `nest mcp` server. Ranked by leverage.

## 1. `contains?` on a map is O(n) — ~100× slower than `get` (headline)

The whole "the app is very slow" symptom traced to one builtin. A single `step`
on ~700 live cells took **~550 ms**; profiling the pieces:

| part of `step` | time |
| --- | --- |
| `(mapcat neighbours (keys live))` (~5600 coords) | 36 ms |
| `(frequencies …)` (→ 2750-entry map) | 17 ms |
| `survivors` (apply the rule) | **531 ms** |

`survivors` was the entire cost, and inside it, the membership test:

| 700 calls on the same map | time |
| --- | --- |
| `(contains? m k)` | **222 ms** (~0.3 ms/call) |
| `(get m k)` | **2 ms** (~3 µs/call) |

Map *writes* are fast (`frequencies` built a 2750-entry map in 17 ms; `into {}`
of 700 pairs in 3 ms), so the map itself is fine — **`contains?` is doing a
linear key scan instead of the CHAMP hash lookup that `get` uses.** Swapping the
rule's `(contains? live cell)` → `(get live cell)` (values are `true`, so it's
directly truthy) cut `step` from ~550 ms to **67 ms**, identical output. The live
animation went from "gen 0 in 3 s" to "gen 18 in 3 s"; the test suite from
700 ms / 570 MB to 39 ms / 22 MB.

This is **silent-wrong performance**: the idiomatic "set of coords as
`{[x y] true}`, test with `contains?`" pattern is the natural thing to write and
is quietly ~100× too slow. (Note: vector keys vs integer keys made no
difference — both ~500 ms before the fix — so key hashing was *not* the cause;
`contains?` was.)

**Asks:** make `contains?` go through the same hash path as `get` (it should be
O(log), not O(n)); until then, document that `get` is the fast membership test.

## 2. The runtime spams `[DBG]` lines on `spawn`

Every `spawn` printed lines like `[DBG] child 2 coroutine body entered` /
`[DBG] child 2 heap built, about to apply` / `[DBG] child N apply Ok, result =
"nil"` to stderr/stdout. Looks like a leftover `eprintln!` on a non-debug path —
it corrupts any TUI/animation output and any `nest run` that spawns. Should be
behind a debug flag or removed.

## 3. Naïve concurrency does not pay for this workload (and isn't "visible")

Attempted a parallel `step`: fan the live cells out to N worker processes, each
computing a partial neighbour-count map; fan the partial maps back in (key-wise
sum) and apply the rule. It was **correct** (a test asserted `pstep ≡ step` on
random boards, which also exercised maps with vector keys round-tripping across
per-process heaps via `send`), but:

- On 100 generations of an 80×40 board: **serial 26.9 s vs parallel (8 workers)
  27.7 s** — parallel was *slower*. (This was *before* the §1 fix; both are far
  faster now, but the ratio is the point.)
- Why: the **fan-in merge is serial and ~as costly as the work parallelised**
  (summing 8 overlapping count-maps is O(total) map churn); `send` **deep-copies**
  each partial map across heaps; and we `spawn` N processes *per generation*.
  Classic Amdahl + coordination overhead swamping a small parallel region.
- After the §1 fix `step` is ~67 ms and dominated by the frame `sleep` — so there
  is simply no speedup to *see*. A benchmark that prints two timings doesn't show
  concurrency either.

A decomposition that *would* win is spatial **tiling with a halo exchange**
(disjoint regions → partial maps barely overlap → merge ≈ free union). Not worth
it here, but it's the honest "how to actually parallelise a CA" answer.

**Takeaway for teaching material:** the reflex "make it concurrent to make it
fast" is wrong for fine-grained immutable workloads with a serial reduce; the
first move is to find the O(n)-where-it-should-be-O(1) builtin (§1), not to add
processes.

## 4. A supervised/spawned render loop has a different memory profile

The animator runs as a child of a `std/supervisor` one-for-one supervisor
(`start-supervisor [{:id :life :start (fn () (spawn (life-proc)))}]`, with `main`
parked on `receive`). Sampling `/proc/<pid>/VmRSS` once/sec over
`nest run --for 22s` (output to `/dev/null`):

```
570 1140 114 657 1180 257 803 252 533 1078
234 770 215 512 1041 237 772 266 605 948   (MB)
```

This is a **bounded sawtooth, not a leak**: RSS climbs, the per-process GC fires,
drops to ~115–265 MB, repeats; peaks do **not** trend up across the run; the
process exits cleanly at the cap and RAM recovers. **But** the high-water is
~1.1 GB — far spikier than the entry-depth-fixed *top-level* `nest run` loop,
which runs nearly flat (~5 MB). The difference is **where the loop runs**: the
depth-1 entry path collects at the eval safepoint, but a loop inside a **spawned**
green process (supervisor → `spawn` → `life-proc`) reclaims at a much looser
threshold. So **moving a render loop under a supervisor (or any `spawn`) silently
changes its memory profile from flat to a ~1.1 GB sawtooth.** Bounded and correct,
but the spawned-process GC threshold and the depth-1 path's should probably
converge. (Measured before the §1 fix, which also reduces per-gen allocation.)

## 5. Process notes (what helped / what gave false confidence)

- **The MCP `eval` loop is the right way to validate before shipping.** This
  session, profiling `step` piece-by-piece through `eval` is what localised the
  `contains?` cost in minutes — guessing would not have. Earlier the set-based
  `step` was validated through `eval` *before* writing the file, which caught that
  `into {}` over a filtered `frequencies` keeps the *counts* as values (needed an
  extra `map … [cell true]`).
- **`nest test` gives false confidence for anything loop/perf/output-shaped.** The
  pure-function tests (`step`/`render`/wrapping) were green the whole time while
  the app was unusably slow and, in an earlier pass, printing `#<fn …>` instead
  of escapes. A green suite says nothing about render-loop output or per-step
  cost.
- **`require` hygiene:** `life.blsp` carried a dead `(require 'supervisor)` (the
  supervisor wiring lives in `main.blsp`); nothing flagged the unused require.
