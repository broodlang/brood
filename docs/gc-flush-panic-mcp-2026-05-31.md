# GC `flush_oob` panic in a long-running `nest mcp` server (2026-05-31)

**Status: CONFIRMED — stale-binary recurrence (KI-1), NOT a live regression** (confirmed on HEAD `00f06ce`, 2026-05-31).

---

## Recurrence 2026-06-02 — same cause (stale MCP server), re-confirmed

Hit again while driving the `brood-life` project through `nest mcp` (profiling
`life/step` with `eval`/`bench`). **Same signature, same root cause: the attached
MCP server was running a pre-rebuild binary.**

**Evidence it's the stale-binary case, not a live regression:**

| Fact | Value |
|---|---|
| Running `nest mcp` server | PID 1934575, **started Tue Jun 2 08:05:42** |
| `~/.local/bin/nest` (what the server execs) mtime | **2026-06-02 08:45:45** — rebuilt ~40 min *after* the server started |
| brood HEAD | `30ec33a "test: GC rooting-across-collect bench repro (scratch)"` (active GC-rooting work) |

So the server is serving a binary from before the 08:45 rebuild — exactly the
`StalenessGuard` condition (binary mtime newer than server start). The guard's
stderr warning isn't visible through the MCP/stdio tool channel, so the stale
server kept serving the pre-fix runtime unnoticed.

**Verbatim panics this session (all `flush_oob`, `index ≫ slab_len`):**
```
GC flush: env handle ... region=0 age=old   epoch=20   index=1133 slab_len=23, collecting old-gen (major)
GC flush: env handle ... region=0 age=old   epoch=16   index=199  slab_len=23, collecting old-gen (major)
GC flush: vector handle ... region=0 age=young epoch=948  index=167  slab_len=13, collecting nursery (minor)
GC flush: map handle ... region=0 age=young epoch=1075 index=1753 slab_len=69, collecting nursery (minor)
```
Triggered by: `bench (reduce + 0 (range 100000)) :iterations 10` (the
`mcp-bench-tool` shape — first result held live across the bench loop), and a
single heavy `eval` against an image already populated with several ~3500-entry
maps.

**Could NOT reproduce on the current (08:44/08:45) build**, matching the
2026-05-31 finding. All of these run clean on a fresh debug-assertions binary,
including under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`:
- the faithful `mcp-bench-tool` shape: `read-string` a form, `(eval form)` once
  into a held `value`, loop `(eval form)` N more times, deref `value` at the end
  — with the image pre-populated by three retained ~3500-entry global maps;
- the same under forced-frequent majors (`BROOD_GC_MAJOR=1500 BROOD_GC_TENURE=400
  BROOD_GC_FLOOR=300`);
- 20× `(reduce + 0 (range 100000))` back-to-back via `nest run`.

The synthetic repros top out at low epochs; the live crashes show epoch 948–1075
(hundreds of prior collections), i.e. they need a genuinely long-lived server's
accumulated old-gen history — which only the stale, hours-old server had.

**Resolution: operational — restart the `nest mcp` server onto the current
binary.** No code change indicated; the fresh runtime does not exhibit the
defect. If a future session reproduces this on a server **demonstrably started
after the last rebuild**, that flips it to a live regression — capture it under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (the verifier names the root→cell path
before the cold `flush_oob`) and reopen.

---

### Original 2026-05-31 report follows
**Severity:** medium (crashes the MCP eval image mid-session; no data loss, but kills in-flight work)
**Relates to:** [`known-issues.md`](known-issues.md) **KI-1** (GC slab-OOB / `flush_oob`), `concurrency-v2.md` §6
**Reporter:** surfaced while driving the `brood-life` (foobar GoL) project through `nest mcp` (`eval`/`bench`/`doc-search`).

## Confirmation (2026-05-31, HEAD `00f06ce`)

**Hypothesis A confirmed; B ruled out.** All three triggers were re-run on a fresh
debug-assertions build under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` (collect + live-
graph verify at every safepoint) — **none panics**:

| # | Trigger (re-run) | Result |
|---|---|---|
| 1 | `(first (shuffle 42 coords))`, `coords` = the 4800 `[x y]` cells | ✓ clean |
| 2 | `(doc-search "append to file")` | ✓ 179 results, no use-after-GC |
| 3 | `(reduce (fn (m i) (assoc m [(mod i 120) (mod i 40)] i)) {} (range 20000))` — heavy nursery map-churn | ✓ clean |

Triggers **1 and 3 are single-threaded** — exactly the path hypothesis B feared the
2026-05-31 KI-1 re-confirmation (concurrent `pstep` fan-out) might not have covered.
They pass on current HEAD under the verifier, so the moving collector has **no
rooting/liveness defect on these paths**. The original panics therefore came from
the two `nest mcp` servers **pinned to pre-17:57 binaries** (started 15:27 / 16:28,
before the rebuild) — the documented KI-1 stale-binary cause.

**Action taken (operational, not a code bug): guardrail implemented.** `nest mcp`
now checks per request whether its executable's mtime is newer than the server's
start time and, if so, prints a loud one-shot stderr warning to restart (the
`StalenessGuard` in `crates/nest/src/mcp.rs`, unit-tested). A stale server can no
longer silently serve a pre-fix runtime for hours unnoticed — which is the only
thing that actually let this happen.

## Summary

A long-lived `nest mcp` server panics in the moving collector's copy phase
(`heap.rs::flush_oob`) under heavy allocation from `eval`/tool calls. Three
independent triggers in one session, each a clean reproduction of the KI-1
`flush_oob` signature: a LOCAL-tagged handle reachable from the GC roots whose
`index()` is far past the live source slab.

The two `nest mcp` servers in play **predate the current binary**, which is the
documented KI-1 §"2026-05-31" cause (panic came from a server *pinned to a
pre-fix binary*, did not reproduce on fresh HEAD). So this is most likely the
same stale-binary recurrence — but two of the three triggers here are
**single-threaded heavy allocation**, not the concurrent `pstep` fan-out KI-1
focused on, so a genuine regression in the current generational collector can't
be ruled out without re-running on HEAD.

## Environment

```
nest mcp (PID 1902420)  started Sun 2026-05-31 15:27:06   (~2h31m elapsed, 7+ min CPU)
nest mcp (PID 2050938)  started Sun 2026-05-31 16:28:42
~/.local/bin/nest       mtime  2026-05-31 17:57:31         <- REBUILT AFTER both servers started
~/.local/bin/brood      mtime  2026-05-31 17:57:21
brood HEAD              4c94071 (VM match/pattern-fns work)
```

Both running servers are executing a binary from **before** the 17:57 rebuild →
stale, matching KI-1's confirmed-stale-binary case.

## Observed panics (verbatim)

All three share the `flush_oob` shape from `heap.rs:4699`. Note `index ≫ slab_len`
(100×+ in two cases) — the hallmark of a badly stale / foreign handle, not an
off-by-one.

**1. `shuffle` of a 4800-element list (old-gen / major):**
```
GC flush: env handle indexes the source slab out of bounds —
region=0 age=old epoch=119 index=620 slab_len=181, collecting old-gen (major).
A handle reachable from the GC roots is not a live this-pass object
(missed rooting / use-after-GC / foreign handle).
Re-run with BROOD_GC_VERIFY=1 for the root→cell path.
```
Trigger: `(def *shuf* (first (shuffle 42 *all-coords*)))` where
`*all-coords*` is the 4800 `[x y]` cells of a 120×40 grid
(`(mapcat (fn (x) (map (fn (y) [x y]) (range 40))) (range 120))`).

**2. `doc-search` tool handler (nursery / minor):**
```
panic in tool handler: GC flush: map handle indexes the source slab out of bounds —
region=0 age=young epoch=12778 index=5958 slab_len=52, collecting nursery (minor).
A handle reachable from the GC roots is not a live this-pass object …
```
Trigger: `doc-search` MCP tool with query `"append to file"`.

**3. Heavy serial `reduce` building/discarding maps (nursery / minor):**
```
GC flush: map handle indexes the source slab out of bounds —
region=0 age=young epoch=14880 index=3405 slab_len=51, collecting nursery (minor).
A handle reachable from the GC roots is not a live this-pass object …
```
Trigger: `(reduce (fn (b _) (life/step b)) board (range 200))` — 200 serial
Game-of-Life generations on a ~1500-cell board. Each `step` builds an ~8N-entry
neighbour-count map and discards it, so the loop churns ~hundreds of short-lived
maps through the nursery.

## Signature analysis

`flush_oob` (`crates/lisp/src/core/heap.rs:4699`) is the self-diagnosing guard
added in the KI-1 follow-up. Its own comment names the mechanism:

> `FlushForward::copies()` admits a handle by region + generation-age but **not**
> by slab bound, so a stale (use-after-GC), foreign, or mis-tagged handle that
> slips into the root set indexes the source slab out of bounds here.

So a handle that is (a) LOCAL (`region=0`), (b) of the generation being
collected, and (c) reachable from the roots, but (d) **not a live object of this
pass**, reaches the copy step and indexes a relocated/compacted source slab past
its end. Both `env` and `map` handle kinds, and both nursery (minor) and old-gen
(major) collections, are affected — consistent with a general rooting/liveness
defect rather than one handle kind.

## Hypotheses

**A — Stale MCP binary (most likely).** Both servers predate the 17:57 rebuild;
KI-1 §2026-05-31 already confirmed this exact signature came from a pinned
pre-fix server and **does not reproduce on current HEAD**. If the 15:27/16:28
binaries predate the current generational-GC rooting fixes, that fully explains
it.

**B — Genuine regression in the current moving collector under heavy
single-threaded allocation.** KI-1's re-confirmation exercised the *concurrent*
`pstep` fan-out (16 workers × 4000-entry maps). Triggers **1** and **3** here are
**single-threaded** — a plain `shuffle` and a plain `reduce` over `step`. If the
rooting defect can be hit without concurrency, the 2026-05-31 "does not
reproduce" result may simply not have covered this path. The `index ≫ slab_len`
magnitudes argue for a real stale handle either way.

## Next steps to fix / confirm

1. **Restart both `nest mcp` servers** onto the current (`17:57`) binary, then
   re-run the three triggers above. Per KI-1 this *should* go clean. If so → it
   was the stale binary; close as KI-1 recurrence and consider having `nest mcp`
   warn (or refuse) when its binary mtime is newer than the server's start time.
2. **If any trigger still panics on a current build**, it's a live regression.
   Capture under the verifier:
   ```
   BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 nest mcp     # collect + live-graph verify every safepoint
   ```
   The verifier (`heap.rs:4059`, "walk every LOCAL handle reachable…") should
   trip at the **root→cell** path that introduces the bad handle, naming it
   before the copy phase — turning the cold `flush_oob` into an actionable site.
3. **Minimal single-threaded repro** to add to the suite if B holds (no
   concurrency, no `pstep`):
   ```lisp
   (def coords (mapcat (fn (x) (map (fn (y) [x y]) (range 40))) (range 120)))
   (first (shuffle 42 coords))           ; (1)
   ;; or a churn loop:
   (reduce (fn (m i) (assoc m [(mod i 120) (mod i 40)] i)) {} (range 100000))
   ```
   Run under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` on a debug-assertions build.
4. Regardless of A/B: the existing concurrency regression test
   (`crates/lisp/tests/concurrency_race.rs`) covers the fan-out path; if B holds,
   add a **single-threaded heavy-nursery-churn** case so this path is guarded too.

## Note for `nest mcp` operators

Because the panic kills the eval image, a long-running MCP server should be
restarted after every `make install` / binary rebuild. **This is now guarded:**
`nest mcp` compares its executable's mtime to its start time on each request and
logs a loud one-shot "rebuilt — restart me" warning to stderr (`StalenessGuard`,
`crates/nest/src/mcp.rs`), so a stale server can't silently serve a pre-fix runtime
for hours unnoticed (which is exactly how this was hit).
