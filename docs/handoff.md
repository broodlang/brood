# Handoff — current state, open threads, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); the option book lives in
[`runtime-frontier.md`](runtime-frontier.md). Read this to pick the work back up cold.

**As of 2026-07-31**, brood `ead59798`+, brood-benchmarks `10d669e`. Nothing half-finished.
The overnight soak is done (thread 5, closed); it produced thread 6, which is now **diagnosed
down to one code path and one region** — see below. The fix is an open ADR-091 decision.

Since the perf session this document was written for, the tree also gained **automatic macro
hygiene** and **exact rationals as a kernel type** (`Value::Ratio`, ADR-196 — `1/2` is a
reader literal and `(/ 1 2)` is exact), plus the ADR-166 Phase 1 reserved-name plan and
ADR-197. Those are language work, not runtime work, so §§1–5 below still describe the
current runtime picture unchanged.

**Two of the three 1.0 release blockers in [`roadmap-for-v1.md`](roadmap-for-v1.md) moved**
(re-measured 2026-07-30): `nest::registry` is **resolved** — the tests pin
`commit.gpgsign=false`/`tag.gpgsign=false`, so CI no longer tracks whether a desktop signing
agent is unlocked — and `nest format --check` is red on **26 files, not 52**. The formatter
red is a **style verdict, not a bug**: 40% of its diff is documented comment *hoisting*.
Read that entry before touching the formatter; do not run it tree-wide first.

---

## 1. What shipped, and how to turn each piece off

Every mechanism below has an off-switch, because each one is an optimisation whose fallback
is the old behaviour. If something misbehaves, bisect with these before bisecting commits.

| Change | Off-switch | Worth |
|---|---|---|
| Supervisor children: list → pid-keyed map | — (data structure) | `start-child` O(N²) → O(1)-ish |
| Restart-intensity window: list-filter → two-list queue | — | 5000 restarts: 4.5 s → 0.6 s |
| Closure sends share already-shared code (ADR-194) | `BROOD_NO_SHARE_FN=1` | retained closure 436 B → 48 B |
| Spawn placement spills off a backlogged worker (A5) | `BROOD_SPAWN_SPILL=999999` (or `BROOD_SPAWN_RR=1` for the other extreme) | `latency` p50 5×, p99 2.9× |
| Selective-receive scan takes the mailbox lock once (A6) | — | 4× per step against a backlog |
| Receive-mark (ADR-195) | `BROOD_NO_RECV_MARK=1` | backlogged reply O(backlog) → **O(1)**, 653 µs → 4 µs at 32k |
| Framed reads scan a straddle probe, not the accumulator (ADR-142 correction) | — (bug fix) | drip-fed frame O(total²) → **O(total)**; 4000 chunks 1568 → 90 ms |

Net on the supervised path: **15×** (8000 children, 3746 → 244 ms; Elixir 233 ms).

## 2. Where we stand against the field

From the published run (`brood-benchmarks/results/`):

- **`latency`** (open-loop, fixed arrival rate, 5% of requests occupying 500 µs) — Brood
  **2nd of five** on p99: Elixir 54 µs, **Brood 124 µs**, Node 410, Python 459, .NET 654.
  Brood's p50 is 27 µs against Elixir's 8 µs — that residue is per-message cost.
- **`supervisor`** — Brood 833 ms vs Elixir 253 ms (3.3×). Was ~20× before this session.
- **Compute aggregate** — 2.8× the fastest, 3rd of seven, ahead of Elixir (3.3×).
- **Known cost we published rather than hid**: `pingpong` +2.8%, `ring` +2.3% from the
  receive-mark. Those rows run with near-empty mailboxes, so they pay for it and never see
  the benefit.

## 3. Open threads, in the order I'd take them

1. **`std/` scale sweep — started, harness in §4, and the hit rate is real.** The premise held:
   two of the three quadratics fixed the perf session were in **Brood policy code, not the
   kernel**, and `std/net/*` yielded two more on the first look — `tcp--read-until` rebuilding
   and rescanning its whole accumulator per chunk (O(total²), 16× the chunks → 169× the time,
   no size cap, remotely triggerable), and `http--read-until` still O(head²) in *memcpy*
   because ADR-142 had fixed only the scan half. Both fixed with a straddle probe; flat
   ~15 µs/chunk to 64 000 chunks (`net_framed_scale.blsp`).
   **Swept and clean:** `proc/agent update`, `buffer insert`, `buffer forward-line`
   (`scale_sweep.blsp`). **Still open:** `proc/gen gen-call`'s ratio is unstable and needs
   three points + medians before it means anything; `std/net/*` beyond the framed reads,
   `editor/*` beyond buffer, and the rest of `std/tool/*` are unswept.
   Two lessons for whoever continues: **a comment asserting linearity is not evidence** (both
   net findings sat under one, and one of them was an ADR's claim), and the shape to grep for
   is a `concat`/`append`/`filter` whose argument is the **accumulator**. Also, a `receive`-based
   combinator needs no socket to test at scale: it consumes `[:tcp sock data]` from the mailbox
   and only pins `sock` by equality, so the drip is fabricated with `send` to self.
2. **Per-message cost — the last real gap.** Brood's `latency` p50 is 27 µs vs Elixir's 8 µs,
   and it is the same number behind `pingpong`, `ring` and most of what remains in
   `supervisor`. A request here is spawn + send + a collector receive. See frontier A1/A3.
   The receive-mark only helps receives that *pin a ref*; a server loop matching several tags
   still walks its backlog (BEAM solves that with a saved scan position per receive **site**,
   a different mechanism from the per-ref mark).
3. **Allocator policy — and thread 6 turned out NOT to be about the allocator.**
   `MIMALLOC_PURGE_DELAY=0` returns 17% of RSS on a light workload and ~2.3× on heavy churn,
   for ~4% throughput; the default is the deliberate "spend memory for speed" call from
   2026-06-15. This is back to being an ordinary decision: the churn decay was traced to the
   RUNTIME code region (thread 6), not to mimalloc, and `alloc`/`cons` are flat over 50–74 M
   ops. Purge-0 does not fix the decay because the decay was never the allocator's.
4. **Per-process memory floor** — 5.9 KB vs the BEAM's 3.1 KB, roughly half unattributed.
   Frontier section B. Needs an allocator size-class histogram behind a cargo feature; there
   is no heaptrack/valgrind on this box, only `perf`.
5. **Endurance — CLOSED 2026-07-31. 16/16 soak runs OK, 12,671,363 self-checking iterations,
   zero failures** over 8 hours (8 armed ~819k each, 8 control ~765k each), against a pinned
   copy of the `86cd3fb3` binary. Detector verified to trip first. Full write-up, logs and a
   per-minute `rss.csv`: **`~/brood-soak-2026-07-30/README.md`**. The runtime stays *correct*
   under sustained load overnight; what it does not stay is *fast* — see the new thread 6.
   Two measurement notes worth keeping: RSS is **~1.0 KB per iteration and deterministic in
   iteration count, not in time** (run 1 and run 15, 7 h apart, matched to within noise at
   the same four iteration marks), and I published a wrong "a 9-hour run needs ~21 GB"
   projection by extrapolating MB/*minute* when the driver is MB/*iteration* — the long run
   actually reached 3M iterations / 3.06 GB in 7 h. A8's trap, freshly re-stepped-in.

6. **Throughput decay — SOLVED (diagnosis), not yet fixed. A busy receiver turns a shared
   closure into a private LOCAL copy, and everything else follows.** Worth ~2× throughput and
   ~2× RSS (`supnocrash`: 300 k ops / 639 MB decaying, vs 590 k ops / 318 MB flat under
   `BROOD_NO_JIT=1`).
   **The chain, every step measured:**
   1. `start-child` sends the spec — which carries the `:start` closure — to the supervisor.
   2. If the supervisor is **parked**, the ADR-178 L1 fast path runs `copy_cross_heap`, which
      hands an already-shared RUNTIME closure over **by handle** (ADR-194). Correct, free.
   3. If the supervisor is **busy**, the send takes the ordinary `Message` path, which
      **deep-copies** the closure into the receiver's LOCAL heap. `BROOD_L1_STATS=1`: with the
      JIT the sender outruns the receiver and the fast path hits only **73.2%** — 10 716
      not-parked sends; without the JIT it is **100%** (11 not-parked).
   4. A LOCAL closure has no VM-eligible arm, so `dispatch` defers it to the **tree-walker**:
      `tw_defer` **3936** with the JIT vs **16** without, and `BROOD_TRACE_TWDEFER=1` names it —
      `b5/start-fresh [LOCAL] argc=0`, 2678 times.
   5. The tree-walker's `fn` then builds the `spawn-link` thunk from a **LOCAL `fn_rest`**, and
      `make_closure_cached` early-returns for a non-RUNTIME key *before* both the template and
      const-closure caches — so the thunk is never a constant.
   6. `spawn_impl` promotes that fresh LOCAL closure into the **append-only** RUNTIME region.
      Every call. `BROOD_TRACE_PROMOTE=1`: 2660 of 2671 promotions are
      `<anon> [captures-frame arity=0] :: spawn_impl <- spawn_link`.
   That explains all three puzzles at once: **JIT-dependent** (the JIT makes the sender faster,
   so the receiver is parked less often), **timing-sensitive** (slowing the program with a
   backtrace collapsed it ~30×), and **zero deopts** (nothing deopts; the detour is a
   dispatch-time defer).
   **The fix, and the decision it needs.** ADR-194's share-by-handle currently exists only on
   the L1 parked path. It should also apply when the message is serialised: an
   already-shared RUNTIME closure on a **same-runtime local** send should cross as a handle,
   not as a `ClosureMsg`. The obstacle is that `to_message(heap, v)` takes **no destination**
   — it is the same serialiser used for the cross-node wire, where a handle is meaningless —
   so either a destination/locality flag gets threaded in, or the local-send call site
   substitutes handles before serialising. That is a messaging-core change and wants a
   deliberate pass.
   **Cheaper mitigation worth measuring first:** step 4 is also a bug on its own — a LOCAL
   closure tree-walks *forever* (and `vm_cache_put` caches the negative result). If a copied
   closure could still be VM-compiled, the 10× interpreter cost and the per-call promotion
   both go away even when the copy happens.
   **Tools:** `BROOD_L1_STATS=1` (parked-hit rate), `BROOD_PERF_STATS=1` + `--features
   perf-stats` (`tw_defer`), `BROOD_TRACE_PROMOTE=1` (what enters the region, with capture
   state). Do **not** instrument with a backtrace — it perturbs the race by ~30×.

**Explicitly NOT open: a memory leak.** See §5 — it was chased and does not exist. The soak
strengthens this: 12.7 M iterations with `rt-closures` flat and every fresh process starting
from the same performance, which is not what a leak looks like.

## 4. Tools, and how to use them

All three live in `scripts/fuzz/stress/` and carry usage headers:

- **`soak_selfcheck.blsp`** — sustained load, an invariant checked every iteration, prints
  `ERROR at iteration N` and halts on violation. **Always pair it with a control** that
  reverts the mechanisms under test (`BROOD_NO_RECV_MARK=1 BROOD_NO_SHARE_FN=1
  BROOD_SPAWN_SPILL=999999`); without one, an alarming RSS curve cannot be attributed.
- **`receive_backlog.blsp`** — the receive-mark's benchmark. ~4 µs at any backlog; if it ever
  goes linear again, the mark stopped applying.
- **`net_framed_scale.blsp`** — the framed reads at scale. ns/chunk must be FLAT in `CHUNKS`;
  it carries its own two controls (`tcp-read-n`, already O(total), and a receive-and-discard
  floor at ~1.0–1.7 µs/chunk), because an absolute per-chunk number means nothing without
  them. That row has a ~15% run-to-run spread — don't read a small delta off it.
- **`reload_cost.blsp`** — fixed-iteration memory harness (see the trap below).
- **`decay_isolate.blsp`** — thread 6's harness: one operation per `MODE`, throughput per
  fixed-size window plus RSS, the supervisor's heap, and `:runtime-closures`. It is what
  turned "the runtime slows down" into "`start-child` appends to the RUNTIME region". Run
  modes **sequentially** — two in parallel and each one's growth pollutes the other's curve.
- **`scale_sweep.blsp`** — thread #1's harness, added 2026-07-30. Runs a `std/` framework op
  at N and 4N and prints the ratio (linear ~4×, quadratic ~16×). **Its header carries the
  measurement caveat and must be read first:** `proc/agent update`, `buffer insert` and
  `buffer forward-line` measured linear; `proc/gen gen-call` read 10.21× then 7.77× at base
  2000 but 2.93× at base 4000, which is not a quadratic's signature (a quadratic does not
  improve as the base grows). Extend it to three points + medians before believing any row.

Plus the existing `scripts/fuzz/run.sh <generator>` (differential across 4 engine configs)
and `dist_chaos*.sh` (multi-node, closure-shipping).

## 5. Traps — every one of these cost real time this session

- **Never difference time-boxed runs.** A time-boxed run does a different number of
  iterations per configuration, and RSS tracks iterations, so the comparison measures the
  iteration count rather than the change. This produced **two wrong versions of frontier A8**
  ("churn leaks", then "~75 KB per `def`, ~270 MB/hour"). Fix the *work*, repeat, compare
  medians. Correct answer: **hot reload costs nothing measurable**; the growth is allocator
  fragmentation against a 59 KB live set.
- **Establish the noise floor before believing a delta.** Run base-vs-base first. `pingpong`
  measured +5.7%, then +4.5%, and once a config that should have been *slower* came out
  faster — the tell. With a 0.5% floor established, the true figure was +2.8%. `spawn-live`
  is worse: its own floor is **20.6%**, so nothing smaller is resolvable there (it has
  produced phantom results five times now).
- **Report the statistic you computed.** "Median of 5" was true of the *run*, not the
  *metric*: the sweep sorted runs by p99 and printed that run's whole p50/p99/p99.9 triple,
  so the quoted p99.9 was luck. p99.9 on the `latency` workload is not resolvable at that
  sample size at all.
- **`make ab` pins to one core** — right for codegen, meaningless for a *placement* change
  (that is why `spawn-live` reads 6740 ms there against 2469 in the harness). Use unpinned
  A/B for scheduler work.
- **`pkill -f <pattern>` matches your own shell** when the pattern (or the path) appears in
  the same command line. It killed my shell twice and left watcher loops that could never
  exit. Use the bracket trick (`pgrep -f "harness[.]py"`) and kill by PID.
- **Process death reports go to stdout, not stderr** — `2>/dev/null` will not filter them;
  pipe through `grep -v 'died: error'`.
- **Verify a detector before trusting it.** I corrupted an invariant deliberately to confirm
  the soak actually fails; a detector you have never seen trip is not a detector.

## 6. Semantics worth knowing (documented, not bugs)

- **Hot reload does not reach a self-recursive loop.** A tail self-call compiles to
  `Node::SelfCall`, which re-runs the arm *without resolving the callee*, so redefining the
  looping function itself does not affect a process already inside it — only a fresh call
  gets the new body. Redefining any *other* global the loop calls does reach it. Erlang's
  local-vs-remote rule; see `live-editing.md`, and it is why Stage 6 (an upgrade hook) exists.
- **A closure that captures no locals is already shared code**; one that captures a local is
  copied on send (~9× more expensive). That is why supervisor `:start` thunks should avoid
  captures — ADR-194.
- **Duplicate supervisor `:id`s** now resolve to the later-started child (was: the first).
  OTP requires uniqueness anyway.
- **`RSS is not a proxy for live data`** on this runtime: 2.5 M spawns left 4 live processes
  and 59 KB of live heap against hundreds of MB of RSS.
