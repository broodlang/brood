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
   **2026-08-02 — five more found and fixed, all three-point confirmed, all with a
   regression row in `scale_sweep.blsp`:** `template/render` 318→24 ms (it re-sliced the
   rest of the template *and* the output per marker — fixing one of the two changed
   nothing), `last-index-of` 540→1 ms (a Brood loop calling `index-of` per match; on an
   editor hot path both ways), `strip-ansi` 1583→109 ms (`char-at` O(i) *and*
   `string-length` O(n) per character — two stacked quadratics), `stream-lines` 303→39 ms
   (quadratic *inside* one chunk, which is the normal case for a 64 KB socket read), and
   `format-source` 3593→1988 ms — **that one I had cleared as linear on 3.80/4.12/4.64 and
   was wrong**; pushing the base up gave 4.46 then 6.40. The lesson is in the harness
   header now: a ratio near 4 that RISES across bases is not linear, only a falling one
   (warm-up) clears a row.
   Then the string kernel underneath them: a **cached char count** on the heap string slot
   (so `string-length` is O(1) and `chars == bytes` is an O(1) pure-ASCII test) and
   **`expect_string_ref`** — `expect_string` returns an *owned* `String`, i.e. every string
   builtin copied its whole argument per call. `inc-scan` 1114→15 ms; `(char-at s 3)` on a
   216 KB string 23 ms→0 ms for 2000 calls and now flat in string size.
   **Swept and clean:** `proc/agent update`, `buffer insert`, `buffer forward-line`,
   `proc/gen gen-call`. **Every row in the sweep is now linear.**
   **Still open:** `std/net/*` beyond the framed reads, `editor/*` beyond buffer, and the
   rest of `std/tool/*` are unswept; ~115 `expect_string` call sites still copy.
   **Temper the expectations, though** — the string work barely moved real workloads:
   `nest format --check` over the repo is unchanged, one 25 600-form file gained 14%, and
   fuzzy ranking 116→106 ms. Three predictions in a row about where a string fix would pay
   off were contradicted by measurement. These copies only bite when ONE string is large.
   Two lessons for whoever continues: **a comment asserting linearity is not evidence** (both
   net findings sat under one, and one of them was an ADR's claim), and the shape to grep for
   is a `concat`/`append`/`filter` whose argument is the **accumulator**. Also, a `receive`-based
   combinator needs no socket to test at scale: it consumes `[:tcp sock data]` from the mailbox
   and only pins `sock` by equality, so the drip is fabricated with `send` to self.
2. **`latency` — RE-DIAGNOSED AND LARGELY FIXED 2026-08-02. It was never per-message cost.**
   This thread used to read "per-message cost — the last real gap, p50 27 µs vs Elixir's 8 µs".
   Profiling (by ablation; `perf` needs root here, `perf_event_paranoid=4`) says otherwise:
   `spawn` alone 0.9 µs, send+receive **1.1 µs**, a full round trip **4.3 µs**. Nothing near
   27. Three hypotheses died on the way — queueing (lowering the arrival rate made p50
   *worse*: 27/45/42/41 µs at 20k/10k/5k/2k rps), wake latency on a parked worker (worth
   0.3–2 µs), and the dispatcher missing deadlines (its pacing overshoot is p50/p90/p99 = 0).
   With the handler doing **no work at all** p50 was still 26 µs, so the whole of it was the
   interval between `spawn` and the handler's first instruction.
   **Cause: spawn placement.** A child goes on the spawner's own worker; a spawner that keeps
   running (this row's dispatcher busy-waits to each scheduled instant) holds it there for a
   full quantum, and an idle peer only re-probed for stealable work every `STEAL_BACKOFF` =
   **10 ms**, which made stealing a background rebalancer rather than a latency mechanism.
   Fixed: an idle peer is told immediately when a peer queues a child, the owner keeps
   `STEAL_GRACE_NS` (5 µs) of first refusal so a spawn-then-block parent still runs its own
   child warm, and the wake is gated on `Process::spawns_since_park` so the spawn-then-block
   shape issues no wake at all. **p50 27 → 19 µs, p99 124 → 78 µs, p99.9 ~800 → 658 µs**, with
   `supervisor`/`pingpong`/`ring`/`spawn`/`pfib` all at baseline. Levers:
   `BROOD_NO_STEAL_WAKE=1`, `BROOD_STEAL_GRACE_NS=<n>`.
   **What is left**, and it should be re-derived rather than assumed: 19 µs against Elixir's
   8 µs, which is still spawn-to-first-instruction, not messaging. `pingpong` (202 vs 59 ms)
   and `ring` (794 vs 263 ms) remain genuinely message-bound — the receive-mark only helps
   receives that *pin a ref*, and a server loop matching several tags still walks its backlog
   (BEAM uses a saved scan position per receive **site**, a different mechanism).
3. **Allocator policy — and thread 6 turned out NOT to be about the allocator.**
   `MIMALLOC_PURGE_DELAY=0` returns 17% of RSS on a light workload and ~2.3× on heavy churn,
   for ~4% throughput; the default is the deliberate "spend memory for speed" call from
   2026-06-15. This is back to being an ordinary decision: the churn decay was traced to the
   RUNTIME code region (thread 6), not to mimalloc, and `alloc`/`cons` are flat over 50–74 M
   ops. Purge-0 does not fix the decay because the decay was never the allocator's.
4. **Per-process memory floor — MEASURED AND ATTRIBUTED 2026-08-03.** The idle floor is
   **4.19 KB**, not 5.9 KB: measure the *slope* of RSS against process count (4389 / 4186 /
   4195 bytes at N = 10k / 40k / 80k), because RSS/N folds in the ~24 MB runtime base. The old
   5.9 KB came from `spawn-live`, where each process *also* holds a copied message payload — so
   two different quantities were being compared against the BEAM's 3.1 KB.
   **Not allocator retention:** `MIMALLOC_PURGE_DELAY=0` leaves the slope at 4230 / 4120 (base
   RSS does drop 23.9 → 20.6 MB, which is why the slope and not the ratio is the measurement).
   Attribution — `Process` 1304 B (of which **`Heap` 1200 B, embedded by value**), `Mailbox`
   184 B, `Suspended` 136 B = **1624 B structural**, leaving ~2566 B. That remainder is not
   data (`process-info :memory` for an idle process is **64 bytes**) but per-allocation
   overhead across the ~25–30 distinct blocks a process owns: touched `Vec` capacities plus
   size-class rounding, with `Box<Process>` at 1304 B landing in a class well above it.
   Printed and ceiling-asserted by `per_process_floor_is_attributed`, because `Heap` being
   inline means **any new `Heap` field costs one per live process**.
   **Next, in order:** (a) box the `Heap` inside `Process` — `Process` drops to ~112 B and
   `Heap` becomes its own block; if rounding dominates the floor falls, and if the indirection
   costs more than it saves that is also worth knowing. (b) Only then build the allocator
   size-class histogram, which settles the block count directly. (a) is mechanical and tells
   you whether (b) is worth the effort.
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

6. **Throughput decay — FIXED 2026-08-03 (ADR-208). Kept below for the diagnosis trail.**
   `rt_closures` on the churn harness is now **66 and constant** (was 143,752 and climbing);
   throughput flat at ~24,000 ops/s against a decaying ~13,800; RSS 213 MB against 502 MB.
   A serialised same-runtime send now hands an already-shared closure over **by handle**
   (`Message::FnShared`), with a `GenPin` holding its generation while queued and a drain-ack
   re-arm when it lands. Lever: `BROOD_NO_SHARE_FN_MSG=1`. Validated by a 1.78 M-iteration
   paired soak, 941/941, and a test that fails with the mechanism off.
   **Thread 6b (adaptive RUNTIME reclamation threshold) is closed with it, not deferred.**
   Its premise was reclamation pressure from a region growing ~0.87 closures/op. With the
   region flat, `BROOD_RT_GC_FLOOR` is inert: 24038 / 24154 / 24390 ops/s across a 128x range
   (512 / 4096 / 65536). There is no policy left to tune — re-measure before reopening it.
   **Note where the win does NOT appear:** the published `supervisor` row is neutral (820 vs
   831 ms, 1.013x) because it runs ~25,000 operations and the decay needs ~175,000 to show.
   This is a sustained-load win; no burst benchmark will see it.

   *Original diagnosis, retained:* **Throughput decay — SOLVED (diagnosis), not yet fixed.** A busy receiver turns a shared
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
   **NEW 2026-08-03 — the lifetime problem is bigger than "root the queued messages".**
   Before writing any of this, read `report_gen_liveness` in `core/heap/gc_runtime.rs`. The
   drain protocol caches a process's "clean" ack for the whole epoch, and its stated
   justification is precisely the property `Message::FnShared` would remove:

   > *an old-gen handle can never arrive by message (messages deep-copy, promoting closures
   > into the receiver's current generation). So a clean ack needn't be re-earned each
   > safepoint*

   With shared handles in messages that is false: a process can ack clean, then be delivered a
   queued handle into the draining generation, and the collector frees it underneath. So the
   work is not just "seed the liveness probe from mailboxes" — it is also **re-arming the ack**
   when such a message is delivered during an active drain. That ack caching is not
   incidental: the same comment records it as the fix for a fan-out drain regression, where a
   contended `drain_acks` read lock on every safepoint of every pinning process dominated the
   run. Undoing it casually re-introduces that.
   Realistic scope: `to_message` (+ a destination/locality signal), `from_message`, the `send`
   path, mailbox delivery, and the drain ack + reachability probe — five places, in the two
   riskiest subsystems here. Budget for it accordingly; it is not an afternoon.
   **A narrower alternative, unmeasured:** intern the *body* on `promote`. The per-call
   promotion appends the whole closure graph; the body AST is identical every time (only the
   captured env differs), so content-addressing just the body would cut region growth to one
   env frame + one closure cell per call. That mitigates rather than fixes — growth stays
   unbounded, just far slower — and it needs the intern table invalidated on compaction, for
   which `shared_closures_clear` is the precedent. Measure what fraction of the 2× it buys
   before choosing between the two routes.

   **The fix, and the decision it needs.** ADR-194's share-by-handle currently exists only on
   the L1 parked path. It should also apply when the message is serialised: an
   already-shared RUNTIME closure on a **same-runtime local** send should cross as a handle,
   not as a `ClosureMsg`. The obstacle is that `to_message(heap, v)` takes **no destination**
   — it is the same serialiser used for the cross-node wire, where a handle is meaningless —
   so either a destination/locality flag gets threaded in, or the local-send call site
   substitutes handles before serialising. That is a messaging-core change and wants a
   deliberate pass.
   **The cheaper mitigation was investigated on 2026-08-02 and does not exist. Both candidate
   shortcuts are blocked at code level; the messaging change is the only real fix.**
   - *"Let a copied closure VM-compile."* `compile/mod.rs:1821` — `cache_key` keys a LOCAL
     closure on its first body form and requires that form to be a **non-LOCAL** `Pair`
     (code in RUNTIME, only captures LOCAL). A cross-heap copy copies the body code into
     LOCAL too, so the key is `None`: never cached, never compiled, deferred every call. The
     comment there gives the reason it cannot simply accept a LOCAL body — LOCAL handles are
     recycled by the collector, so the key would alias an unrelated closure and run the wrong
     code. Content-addressing it (hash the body AST) would work but costs an O(body) hash on
     every call, on the hot path.
   - *"Stop `promote` re-copying the body."* Already the case: `promote_in`
     (`heap.rs:3533`) guards every arm with `if id.region() == LOCAL`, so an already-shared
     subgraph passes through untouched. The body is re-promoted **because the copy made it
     LOCAL**, not because promote is wasteful. Nothing to fix here.
   Both roads lead back to the same place: do not copy the code.
   **Fresh baseline (2026-08-02, `MODE=sup WINDOW=5000`)** so any fix is measurable:
   19 685 → 9 523 ops/s (**2.07× decay**) across 98 windows, RSS 97 MB → 929 MB,
   `rt_closures` 2 698 → 412 668 — linear at **~0.87 per op** — while `sup_heap` and
   `my_live_bytes` bounce around freely, i.e. both LOCAL heaps GC normally and only the
   append-only region grows. A longer run reached 454 113 closures / 1.02 GB at 534 k ops.
   **What the real fix needs, concretely.** `Message::StrShared(Arc<SharedBlob>)` is the
   precedent for a same-runtime by-handle payload — but it is kept alive by an **Arc
   refcount**, and RUNTIME closures are not refcounted; they are freed per *generation*
   (handles carry one: `ClosureId::runtime_gen(idx, gen)`). So a `Message::FnShared(bits)`
   needs (a) same-runtime detection at the `send` site — available there, it already tests
   `dist::is_local(node)` — and (b) a lifetime guarantee: either the RUNTIME collector treats
   queued mailbox messages as roots, or a per-generation in-flight counter blocks freeing a
   generation with outstanding references. (b) is the whole risk, and it is why this wants a
   deliberate pass rather than a squeeze into another session.
   **Tools:** `BROOD_L1_STATS=1` (parked-hit rate), `BROOD_PERF_STATS=1` + `--features
   perf-stats` (`tw_defer`), `BROOD_TRACE_PROMOTE=1` (what enters the region, with capture
   state). Do **not** instrument with a backtrace — it perturbs the race by ~30×.

7. **Leaf inlining is all-or-nothing per arm** (found 2026-08-02, not attempted).
   `leaf_inline_derive` bails if any non-tail `Call` survives splicing — sound, because an
   inlined native has no deopt checkpoint — so **one un-spliceable callee blocks inlining of
   every small callee beside it**. Measured: an arm whose only non-tail call is a
   three-instruction leaf inlines and runs 196 ms/1M; add a recursive callee and the probe
   vanishes, tiny leaf included, at 588 ms. This is where `mandelbrot`'s `->float` cost lives
   (`row-sum` calls it *and* the recursive `esc`), at ~85 ns a conversion where every other
   language in the suite emits a machine cast. The lever is partial splicing while keeping the
   arm's checkpoint — trades the checkpoint-free fast path for coverage, so measure it.
   Full write-up in `docs/jit-optimizing-tier.md`.

8. **A published benchmark port drifts silently when language semantics change under it**
   (2026-08-02). `mandelbrot` looked like a 3.5× runtime regression bisected to kernel exact
   rationals; it was not a runtime regression at all — with identical source the pre-rationals
   and current binaries measure 201 ms and 200 ms. `(/ px n)` on two ints had simply stopped
   being a float divide (ADR-196, working as designed) and started building a rational per
   pixel. Nothing failed and the checksum never moved; the row just quietly stopped measuring
   what it claimed. `supervisor.blsp` had the same class still latent (`(/ n 4)` feeding an
   iteration count — exact at N=20,000, a rational otherwise). **When a numeric primitive's
   semantics change, grep the benchmark ports.**

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
