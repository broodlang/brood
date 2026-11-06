# Perf handoff — work that must run on a benchmark box

**Why this file exists.** The primary development machine for this repo does not run
benchmarks well: `whklat` is a **12-core laptop** (i5-11500H, `powersave` governor, clocking
0.8-3.7 GHz per core) shared with other work, its thermal and cache state drifts across a
session (a `pingpong` baseline wandered ~10% across one day's runs), and it has repeatedly
produced confident, wrong perf verdicts. So perf verification is *deferred* rather than
skipped, and this file is the queue.

(Corrected 2026-09-11: this said "28-core workstation", which is not this machine and made
the core-pinning advice below read wrongly — the harness pins compute rows to cores **8-11**
here, not 24-27.)

**What the box CAN do, measured 2026-09-11:** *within* one session it is better than this file
assumed — `make ab --floor` read a **0.7% floor** on `sort`, and the same binary measured twice
interleaved reads 0.0-0.7%. What it cannot do is compare **across** sessions: the same unchanged
binary read 115.7 ms in one session and 121.8-125 ms in another, weeks apart. So a
fixed-baseline A/B measured in one sitting is trustworthy here; a number compared against a
stored one is not.

Read `docs/benchmarking.md` for *how* to measure and the `Commands` section of `CLAUDE.md`
for the traps. This file is only *what* to measure, *why*, and *what a pass looks like*.

---

## Before anything: six ways the measurement lies

These have each cost a wrong verdict in this repo. They are not general advice.

1. **`make doctor` first.** A stale binary fails by *agreeing with the baseline* — the most
   convincing possible way to be wrong. `make ab` aborts if the two binaries come out
   byte-identical, which is what a silently no-op'd build looks like, but it cannot catch a
   binary that is merely old.
2. **Time the `release-fast` binary, never the profiling ones.** `make perf-brood` writes
   `target/release-perf/brood` (counter atomics on the hot path, ~10% slower) and
   `make perf-symbols` writes `target/release-symbols/brood`; neither overwrites
   `target/release-fast/brood`, which `make ab` and every timing compare against.
3. **Prove the floor before believing a delta.** `make ab --floor` runs the baseline twice
   and reports that row's own base-vs-base spread; a regression counts only past
   `max(5%, 2 × floor)`. For a suspicious row, keep the `target/ab/<sha>/…/brood` binary and
   run base / base / new pinned best-of-15 — a row that read +5.3% "confirmed" solo turned
   out to be +0.9% against a +0.5% floor once the baseline stopped wandering.

Also: **`make ab` pins compute rows to one core**, which charges the benchmark for background
JIT compilation. Right for judging generated-code quality, wrong for any change that alters
*how much* the compiler does. If the change touches tiering, re-run the row unpinned.
`BROOD_JIT_DUMP_IR=1 … | grep -c '^\[jit-ir\]'` counts the compiles.

4. **`--floor` bounds error WITHIN one `ab-bench` run, not ACROSS runs.** It measures
   base-vs-base inside a single invocation, so it says nothing about comparing one run's delta
   to another's. A bisect built from one-baseline-per-run deltas (2026-09-11, KI-128)
   manufactured a smooth ramp and "converged" on a commit that changes four lines of markdown.
   To compare several commits, put them all in ONE session, interleaved, with the same binary
   measured twice as a control.
5. **Every arm must be `:state :live`, and you must check.** A hand-rolled interleaved harness
   compared source-booting baselines against an imaged HEAD — ~10 ms on a 146 ms row, in exactly
   the direction that erases a HEAD slowdown — and produced a confident *retraction* of a true
   finding. `ab-bench` refuses this (`stdimage MISMATCH`) and was right three times that day;
   hand-rolling around it reintroduced the bias. Probe each binary with
   `(io/inspect (stdimage/status))` and refuse the run unless every arm says `:live`.
6. **`stdimage`'s `prune` keeps `max-keep 4`** (`std/tool/stdimage.blsp`), so at most **4
   baselines + HEAD** can be compared imaged in one session. Building a fifth image silently
   evicts the earliest arm into a source boot — which is trap 5, arriving without a warning.

---

## What this box CAN answer (and how) — read before deferring anything here

Deferring a question to a benchmark box is right for a *magnitude* claim and wrong for a
*mechanism* one. Most entries that landed in this file were mechanism questions wearing a
number. The order to try:

1. **Turn it into a structural question.** Did the arm lower (`BROOD_JIT_DUMP_IR=1`, count
   `[jit-ir]`)? Was it refused, and for which reason (`BROOD_JIT_BAIL_TRACE=1`)? Did it lower
   and then fall back (`BROOD_DEOPT_TRACE=1`, needs `perf-stats`)? These are binary and
   load-immune, and the counter-armed build's overhead does not matter because nothing is being
   timed. Task 1 above was answered this way after being queued as unanswerable.
2. **Trust large ratios, never small deltas.** Measured on this box 2026-09-09, mandelbrot,
   3 runs each: JIT arm spread **5%**, VM arm spread **12%**. So an order-of-magnitude
   comparison (tier ladder `BROOD_TIER=0|1|2`, a feature's on/off lever, JIT vs VM) is solid;
   a few percent is noise. Prefer a comparison whose expected effect is large.
3. **Within-process adjacency** for anything smaller: the `crates/lisp/benches/eval.rs` engine
   grid runs both arms back-to-back in ONE process, so the ratio survives load even while
   absolutes wander ±10–20%. Add the workload to that grid rather than timing two invocations.
4. **Count the calls before optimising** — a `static AtomicUsize` + one `eprintln!` at the call
   site, one run. Minutes, where a build-then-A/B round trip is hours, and it is the only cheap
   way to tell a hot path from a plausible one (`compute-frontier.md` §7.8's top item died this
   way: 21 calls on `fib`).
5. **Check the box before believing anything**: `cat /proc/loadavg` and
   `pgrep -af 'while :'` — twelve orphaned load spinners once pinned this box for 4d18h and
   inflated every number taken in that window ~2.5x. Then `make doctor` for staleness, because
   a stale binary fails by *agreeing* with the baseline.

**`perf record` is unavailable here** (`/proc/sys/kernel/perf_event_paranoid` is 4; do not change
it without asking). The substitutes are the VM's own counters — `make perf-brood` plus
`(perf/measure thunk)` / `BROOD_PERF_STATS=1` — and the JIT dumps above.

What is genuinely left for a quiet, pinned box is one class only: absolute cross-process deltas
of a few percent, i.e. `make ab --floor` sweep verdicts. That is Task 1's remaining half (the
30-row sweep) and Task 3 (re-taking KI-100's re-baseline, whose smallest rows are 2.9% and
4.2% — under the floor measured above). Every *mechanism* question here is answerable on the
dev box; check that first.

---

## Task 1 — does KI-114's fix hold KI-109's closure? ✅ ANSWERED 2026-09-11

> ### The magnitude sweep is done too — nothing moved
>
> Measured against `8a2aaa01` with `--floor`, box idle, both arms imaged:
> **`mandelbrot` +0.4%** (floor 1.3%) and **`nbody` +1.2%** (floor 0.4%) — the two float-heavy
> rows `as_f64_pair` sits in the middle of, both inside their floors. Together with the
> structural answer below (the arm lowers, nothing thrash-latches), **KI-109's closure holds on
> magnitude as well as mechanism.**
>
> **`matmul`, the third float row, could not be measured against this baseline** and is NOT
> claimed: ADR-302/308's data-first reorder (2026-09-02) means current bench rows call
> `(reduce coll init f)` while brood 0.19.1 expects `(reduce f init coll)`, so `matmul` dies on
> the baseline arm. Twelve of the 31 rows are unreachable from `8a2aaa01` for this reason —
> see the note at the end of this file. Measure `matmul` against a post-09-02 baseline instead.
>
> The full 30-row sweep is therefore a **19-row** sweep here. Of those, one row moved and it was
> real: `sort`, root-caused and fixed as **KI-128** (a top-level value rendered and discarded on
> every native run — unrelated to KI-114). `errors-deep` +17..27% is KI-123's documented
> post-fix residue against a pre-KI-117 baseline, not new.

**Priority: high.** This is a *possible silent regression*, not a suspected one.

> ### ✅ Answered structurally on the dev box, 2026-09-09 — only the magnitude sweep is still queued
>
> The feared regression has a **named mechanism**, not just a number: `->float` deopts on every
> activation, sixteen in a row latch it `BAILED`, and it runs interpreted for the rest of the
> process. That is a *binary, load-immune* observation, so it does not need a quiet box — and
> the instrumented build's overhead is irrelevant because nothing is being timed. On
> `9a9f6a3d`, lean `make release-brood`, `make doctor` clean, box idle (loadavg 0.03):
>
> | check | command | result |
> |---|---|---|
> | `->float` lowers | `BROOD_JIT_DUMP_IR=1` | **lowered** (1 arm) |
> | `esc` lowers | `BROOD_JIT_DUMP_IR=1` | **lowered** (1 arm) |
> | the KI-109 signature | `BROOD_JIT_BAIL_TRACE=1 \| grep deopt-thrash-latched \| sort -u` | **zero arms, anywhere** |
> | native path carries the work | default vs `BROOD_NO_JIT=1` (now `BROOD_TIER=1`), 3 runs each | **0.21 s vs ~2.0 s (~9.5x)** |
>
> So `as_f64_pair` still licenses `->float`'s promotion, the arm stays native, and nothing
> thrash-latches. **KI-109's closure holds.** The 9.5x is quoted because a *large ratio* is
> trustworthy on a noisy box even when a 3% delta is not — the run-to-run spread measured here
> was 5% (JIT arm) and 12% (VM arm), which is exactly why the sweep below is still deferred.
>
> `row-sum` and `grid-sum` do not lower, and that is **not** this change: `row-sum` bails
> `call-mediated-boxed`, a profitability-gate refusal about call plumbing, not a float-gate
> refusal — `as_f64_pair` cannot produce that reason. They are also the per-row/per-grid
> drivers, not the per-pixel path.
>
> **Task 2 below is fully answered by the same run**: it asks for `deopt-thrash-latched` arms the
> pre-KI-114 binary did not have, and the new binary has **none at all**, so there can be no new
> ones — no second binary needed.
>
> **Still queued for a quiet box, and only this:** the ±few-percent claims — `mandelbrot` within
> its floor of the 2026-09-05 number, and no row regressing past `max(5%, 2 × floor)` across the
> 30-row sweep. Those are absolute cross-process deltas smaller than this box's noise floor.

### The situation

KI-109 was `mandelbrot` ~3% slower than the 0.19.1 column at steady state. It was **closed on
2026-09-05 by `62cbe29f`**, and not by the layout work the entry spent its length on:

> `->float` is `(* 1.0 x)`. The `1.0` puts the arm in float context, so `x` was read through
> `as_f64`, whose guard accepted `Float` alone — and every program that converts calls it
> with an int. The arm deopted on every activation, sixteen in a row latched it BAILED, and
> it ran interpreted for the rest of the process: on `mandelbrot` that is one VM call per
> pixel.

`62cbe29f` made an `Int` operand **promote** (`fcvt_from_sint`) instead of deopting. Closing
gate: `make ab BASE=8a2aaa01 --floor ROWS=mandelbrot`, best-of-9, images live on both arms —
**578 → 579 ms, +0.2%** against a 4.3% floor.

**KI-114 (2026-09-07) then found that promotion was unconditional, and unsound.** A
float-*profiled* arm applied to ints promoted them too, so `(- 33)` answered `-33.0` — pong
failed 22 of 101 tests. The fix (`emit::as_f64_pair`) licenses promotion only when some
operand is **proven** float, which is the VM's own rule for when an op is float arithmetic
at all.

### Why this needs measuring rather than reasoning

The argument that KI-109 is unaffected is: `->float`'s `1.0` is an `Op::Float`, i.e. proven,
so its promotion is still licensed and the arm still stays native. That argument is sound as
far as it goes, and it is exactly the kind of argument that KI-109 itself shows to be
insufficient — that entry spent its length on an icache hypothesis that measured true and was
not the cause. **Bring numbers.**

### Run this

    make doctor
    make release-brood
    make ab BASE=8a2aaa01 --floor ROWS=mandelbrot N=9      # the closing gate, repeated
    make ab --floor                                         # full sweep, all 30 rows
    ./scripts/ab-bench.sh --list                            # row names if you need them

**Pass:** `mandelbrot` within its floor of the 2026-09-05 number, and no row regressing past
`max(5%, 2 × floor)`. Watch the float-heavy rows in particular — `mandelbrot`, `nbody`,
`matmul` — since those are the ones `as_f64_pair` sits in the middle of.

**If `mandelbrot` regressed**, the first question is whether the arm still stays native:

    BROOD_JIT_BAIL_TRACE=1 ./target/release-fast/brood <mandelbrot.blsp> 2>&1 \
      | grep -E "arm=(->float|esc|row-sum)"

`->float` appearing as `deopt-thrash-latched` means the gate is rejecting a promotion it
should license — i.e. `as_f64_pair`'s `FOperand::Float` classification is not recognising the
`Op::Float` operand — and the bug is in
`crates/lisp/src/eval/compile/jit_lower/emit.rs`. Before KI-114 that arm latched BAILED and
cost one VM call per pixel, so this is the exact failure to look for.

### Also worth measuring while you are there

`as_f64_pair` is written so the common both-float path keeps the *same two branches* the
unconditional version had (the first operand's tag test, then the second's inside
`float_or_promoted_int`). That is an argument from the code shape, not a measurement. The
sweep above is what would show it wrong.

Measure **short and long** runs, per `CLAUDE.md`: a tiered runtime has two steady states and
a micro-benchmark reports one of them. Sweep the call count across two orders of magnitude
and check whether the *gap between the arms* moves, not just whether each arm got faster.

---

## Task 2 — two lowerings the KI-114 fix changed with no test reaching them ✅ ANSWERED 2026-09-20 — zero latched arms, in-tree and on the published rows

**Priority: low. Correctness is argued, not tested; perf is unmeasured.**

`as_f64_pair` guards three sites in `jit_lower/prim.rs`. Only one — `Prim2SlotInt`, where
unary `-` lowers — is reachable by any program written for it, and that one is
sabotage-verified by `crates/cli/tests/float_profile_int_stays_int.rs`. The other two are
`Prim2SlotSlot` and `Prim2`'s type-erased `Op::Handle` path.

The shapes that *should* reach them do not tier: `(defn add2 (a b) (+ a b))` warmed on floats
is never elected, and the nbody-shaped `(- (nth v 0) (nth v 1))` beside a float slot
thrash-latches on the **int** path first, so both answer via the VM. Sabotaging those two
arms leaves the guard test green — checked, not assumed, and said so in the test's header.

On a benchmark box with real float workloads, the thing to look for is **new**
`deopt-thrash-latched` arms that the pre-KI-114 binary did not have:

    BROOD_JIT_BAIL_TRACE=1 … 2>&1 | grep deopt-thrash-latched | sort -u

A new one on a float-heavy row means the pair gate is refusing a genuinely mixed
float/int operation somewhere the guard test does not reach.

**Half-answered 2026-09-18, on this box, without a benchmark corpus.** The check the task
describes does not need wall-clock — only the bail trace — so it was run over every
float-heavy program in the tree: `unbox_torture_test`, `jit_float_global_test`,
`jit_float_eq_test`, `float_roundtrip_test`, `jit_eq_join_test`. **Zero
`deopt-thrash-latched` arms in all five.** The bails that do occur are the ordinary
profitability and subset ones — `call-mediated-boxed` ×7, `chunk-outside-jit-subset` ×2,
`call-spill-exhausted` ×1 — none of which is the pair gate's signature.

That closes the comparison for this corpus by construction: with zero latched arms on the
current binary there can be no NEW one, whatever the pre-KI-114 binary had, so no second
build is needed to rule it out here.

**✅ ANSWERED 2026-09-20 — on this box, on the published rows.** The remaining half was
recorded as "the part this box genuinely cannot do: `nbody` and friends live in
`brood-benchmarks`, which is not checked out here". It is checked out — as
`../brood-benchmark` (singular), which is why every tool defaulting to the plural reported it
missing (`scripts/bench-dir.sh` resolves either now). Same grep, release-fast binary,
`BROOD_JIT_BAIL_TRACE=1`:

| row | `deopt-thrash-latched` | the bails that do occur |
|---|---|---|
| `nbody` | **0** | `call-mediated-boxed` ×8, `call-spill-exhausted` ×1, `chunk-outside-jit-subset` ×1 |
| `mandelbrot` | **0** | ×7, ×1, ×1 |
| `matmul` | **0** | ×10, ×2, ×3 |
| `sort` | **0** | ×7, ×1, ×1 |
| `primes` | **0** | ×7, ×1, ×1 |

Every one is the profitability gate or the subset rule doing its job; none is the pair gate's
signature. `make tier-audit` says the same thing over all 29 rows in one command, and is green.
**Kept rather than deleted** (the task said to delete it on a zero) because the method is the
reusable part: the question "did this change refuse an operation somewhere no test reaches?"
is answered by the bail trace and not by a clock, which is why it was answerable here all along.

---

## Task 3 — re-take KI-100's re-baseline if the runtime has moved ✅ ANSWERED 2026-09-11

> **They moved, and one of them was a real regression.** Re-taken against `8a2aaa01`,
> `--floor`, both arms imaged, each flagged row re-run solo at best-of-15:
>
> | row | 2026-09-04 | 2026-09-11 | |
> |---|---|---|---|
> | `startup` | −18.2% | **−9.7%** | still a large win |
> | `sort` | −7.6% | **+5.0%** | → root-caused as **KI-128**, now fixed |
> | `fib` | −4.2% | +3.7% | 4x its floor but under the 5% bar; not pursued |
> | `bintree` | −2.9% | −3.8% | unchanged |
>
> `sort`'s swing is the whole finding: it is not drift, it is KI-128 (`7a72135b` rendering every
> top-level value on the native path). **Re-taken 2026-09-12 with the fix in: `sort` vs
> `8a2aaa01` reads −4.2%** (0.0% floor, best-of-15, both arms imaged) — back on the fast side,
> consistent with the 2026-09-04 direction.
>
> `ring` deserves its own note as a methodology datum: it read **+5.5% in the sweep and +3.7%
> solo**, i.e. the sweep-interleaving drift this file warns about, caught by the mandated solo
> re-run rather than reported as a regression.

**Priority: low.** KI-100 was resolved as filed on 2026-09-04 by re-baselining against
`8a2aaa01`: `startup` −18.2%, `sort` −7.6%, `fib` −4.2%, `bintree` −2.9%. Those numbers
predate ADR-318 (the tree-walker→VM router, default-on 2026-09-04), the prelude image
becoming default, and everything since. Nothing suggests they have moved; nobody has
checked. This is hygiene, not a suspicion.

---

## Task 4 — the empty-pool park backoff: confirm `latency` and the message rows are flat ✅ ANSWERED 2026-09-12

> **`latency` is flat on the metrics that matter, and the control agrees.** The p50/p99
> median-over-11 protocol, unpinned (it is a concurrency row), every arm verified `:state
> :live`, three arms interleaved:
>
> | arm | p50 | p99 | raw p99 (11 runs) |
> |---|---|---|---|
> | `99d61222` (pre-backoff) | 20 µs | 87 µs | 83-104 |
> | HEAD (backoff ON) | 20 µs | 88 µs | 83-97 |
> | HEAD `BROOD_NO_IDLE_BACKOFF=1` | 21 µs | 87 µs | 84-110 |
>
> Within 1 µs across all three, spreads overlapping. The +15% WALL reading below (against an
> 11.9% floor) was arrival-schedule noise, exactly as its floor said — the latency metrics
> themselves did not move. With the five message rows below also clean, **the backoff costs
> nothing anywhere this task named.** The p99.9 claim is still not made, per this file's rule.


> **Five of six rows are clean; `latency` is not resolvable here and is NOT counted as a pass.**
> `ab-bench --floor` against `99d61222` (= `c7afd4ec^`, the commit before the change), both arms
> imaged, box settled to 0.69:
>
> | row | base | new | delta | floor | verdict |
> |---|---|---|---|---|---|
> | `pingpong` | 208 ms | 204 ms | −1.9% | 1.4% | pass |
> | `ring` | 808 ms | 775 ms | −4.1% | 0.1% | pass |
> | `spawn` | 75 ms | 78 ms | +4.0% | 4.0% | pass |
> | `spawn-live` | 1503 ms | 1494 ms | −0.6% | 1.4% | pass |
> | `supervisor` | 927 ms | 909 ms | −1.9% | 1.2% | pass |
> | `latency` | 4311 ms | 4956 ms | +15.0% | **11.9%** | **inconclusive** |
>
> `latency`'s delta is inside `max(5%, 2 x floor)` = 23.8% and that is *not* a pass — an 11.9%
> floor means the instrument cannot see a 15% effect on that row, which is what this file
> already predicted. Letting a wide floor launder a number is the failure mode, not the result.
>
> **Still owed for this task:** `latency` p50/p99 as medians over 11 runs (the protocol
> `BROOD_SPAWN_SPILL` was tuned with), and the `BROOD_NO_IDLE_BACKOFF=1` control on the two
> interesting rows. Both were specified below and neither was run.


> ### ✅ The named open shape is answered on the dev box, 2026-09-09 — the row sweep is what is left
>
> The "shape worth probing" below — work made runnable by a **timer** (no spawn, so no
> spawn-time peer wake) while every would-be thief is parked on its grown backstop — is a
> *mechanism* question with a hundreds-of-milliseconds effect size, so it does not need a quiet
> box. Probe (**`stress/idle_backoff_probe.blsp`**, kept there so `make check-corpora` statically
> checks its names and it cannot rot the way the fuzz generator and scaling probe did): 8 sleepers placed on ONE worker with
> `BROOD_SPAWN_SPILL=100000` (always-local), a **1500 ms** idle so the backoff is provably at
> its 500 ms ceiling (the doubling 10+20+40+80+160+320 reaches the cap by 630 ms), then all 8
> timers expire together and each does 150 ms of CPU work. Serial on one worker would be
> ~2700 ms; full parallelism ~1650 ms.
>
> | arm | last finish | spread |
> |---|---|---|
> | backoff ON (default), 3 runs | 1651 / 1652 / 1651 ms | 1 / 0 / 1 ms |
> | `BROOD_NO_IDLE_BACKOFF=1`, 3 runs | 1652 / 1652 / 1651 ms | 1 / 1 / 0 ms |
>
> Indistinguishable, and both at the parallel ideal — the backoff costs this shape nothing.
>
> **Sabotage, because a flat result is only worth having if the probe could show a delay.**
> With `BROOD_NO_STEAL_WAKE=1` (since removed) the spread grows to 8 ms (backoff ON) and 2-10 ms (OFF): the
> probe does resolve steal latency, and even with the spawn-time wake gone a thief finds the
> work on its ordinary 10 ms re-probe, never the 500 ms backstop. So the shape is protected
> **structurally** — the `enqueue` that makes a timer-woken process runnable wakes a parked
> worker — not by luck, which is the thing the paragraph below could not tell from reasoning.
>
> **Still queued for a quiet box:** the row sweep proper — `latency`, `pingpong`, `ring`,
> `spawn`, `spawn-live`, `supervisor` with floors, and `latency` p50/p99 as medians over 11
> runs. This box cannot resolve that row, for the reason recorded below.

**What changed (2026-09-09).** A parked scheduler worker used to re-probe every 10 ms
forever; it now doubles that backstop to 500 ms **while `STEALABLE == 0`** and resets to
10 ms the moment it runs anything. Motivation was not throughput: an idle runtime was
costing 0.25% of a core per worker (6-8% on 28 cores, 10 h of CPU on a five-day-idle `nest
mcp`). See the devlog entry. (Its opt-out, `BROOD_NO_IDLE_BACKOFF`, was removed 2026-10-09;
an A/B control now means building the commit before the change.)

**Why it should be flat, and why that still needs checking.** With work queued anywhere the
cadence is bit-for-bit the old one, and work is discovered by direct condvar wake
(`enqueue`) or the spawn-time peer wake (`wake_a_parked_peer`), never by this timeout. The
dev box measured post-idle fan-out at 7-12 ms with the backoff against 7-29 ms without,
including with `BROOD_SPAWN_SPILL` huge so that only stealing parallelises. But that box
cannot resolve the `latency` row: `BROOD_SPAWN_SPILL`'s own tuning notes record that two
11-run samples of the *same* binaries disagreed by 3× on p99.9.

**Do.** `make ab --floor BASE=<the commit before the change>` over the message/scheduler
rows — `latency`, `pingpong`, `ring`, `spawn`, `spawn-live`, `supervisor` — and read
`latency` p50/p99 as medians over 11 runs, the protocol `BROOD_SPAWN_SPILL` was tuned with.
The base-vs-new comparison is the A/B control (the opt-out flag is gone).

**Pass.** Every row within `max(5%, 2 × floor)`, and `latency` p50/p99 not worse than the
control beyond that band. **Fail.** Any message row outside it — report the row, the floor
and the control numbers; the flag makes a revert a one-line default flip, not a code change.

**A shape worth probing if a row does move.** The one path with a real (if bounded) change
is work becoming runnable *without* a spawn — a timer or I/O wake enqueued onto a **busy**
worker while every other worker is deep-idle. The owner still runs it after its current
quantum; what waits up to 500 ms is a *thief* taking it off that owner instead. Construct it
with a long-idle process holding one busy worker, then a timer fire onto that same worker.

---

## Task 5 — tier-2 monomorphization: is the ability-dispatch win worth a checker→compiler channel? ✅ ANSWERED 2026-09-17 — no: the ceiling exists, the sites do not

**Answer.** Steps (1) and (2) taken the afternoon this was filed; (3) is not worth building.

**(1) The dispatch cost, one process, warm** (release, `scratchpad/mono.blsp`: a
`defrecord circle`, `(defability Shape (area [self]))`, one impl; 2M iterations, best of
several): a direct call `(direct c)` **162 ns**, the dynamic `(area c)` **530 ns** — so ability
dispatch costs **~370 ns per call** on top of the call itself (`%identity-of` + `%dispatch` +
the computed-head impl call). Tier 1 on a constructor receiver, `(area (circle 2))`, reads
**533 ns against 449 for `(direct (circle 2))`**: the constant-id `%dispatch` leaves ~85–110 ns
of the 370. So Tier 1's ceiling is ~27% of a record-allocating loop and, for the
variable-receiver shape Tier 2 would target, up to ~48% of the loop — well past the 10%
"stop" line on its own.

**(2) Whether any real workload has a site.** `BROOD_MONO=1 BROOD_MONO_DBG=1` over every
benchmark row that loads a library (`json regex strings wordcount base64 sort nqueens
pipeline supervisor persistent-map errors`): **0 devirtualizations, every row.** bedit's
whole test suite (1657 cases, 0.4.3 at `0770056`): **4**, all one example's `Shape/area`. No
benchmark row and no real program dispatches an ability op in a hot loop on a literal or a
constructor call — and Tier 2's variable-receiver sites are the same ops in the same
programs. There is no row for a channel to move. **Closed: the number is recorded, the
channel is not built.** Reopen only when a workload appears whose profile shows
`%identity-of`/`%dispatch`; the microbenchmark above is the probe to re-run then (with `BROOD_DEVIRT_DBG=1` to count sites; the rewrite is always on).

**What (2) found on the way — KI-161, fixed.** Tier 1 had NEVER fired on a record constructor
inside a `defmodule`, i.e. in any real code: the KI-90 guard compared the registry's BARE
recorded name (`:mp/circle → circle`) with the module-qualified call head (`mp/circle`) and
refused every one — while in module-less code the same comparison always agreed, so a
constructor REBOUND to a plain fn (`(defn shape (n) {:n n})` after `(defrecord shape …)`) was
devirtualized to the record's impl: `rebound: REC7` against the dynamic path's `DEFAULT`, on the
pushed 0.30.1 binary under `BROOD_MONO=1`. The `a_stale_record_id…` test passed only because
its fixture is a module. The guard is structural now (`closure_constructs_record`: the head's
bound closure must be one arm whose one body form is the map literal carrying this `:__id__`
— the very identity dispatch reads), the test asserts the record call FIRED under both
spellings, and the sabotage (the name comparison back) reds it both ways.

**Priority when filed: low. Not started; a design that needs a number before it earns its surface.**
Filed 2026-09-17 when the type-system review's last two items were decided (item 5,
return-type dispatch, was declined — ADR-361; this is item 7, which is a perf item).

> **Since ADR-398 Tier 1 is always on and `BROOD_MONO` is gone** — the `BROOD_MONO=1`
> measurements below are of the then-opt-in rewrite; `BROOD_DEVIRT_DBG=1` is the trace now,
> and the `BROOD_MONO=2` lever below was never built (Tier 2 stays deferred). The always-on
> rewrite's head is the guarded `%dispatch-proven`, a slightly different call from the one
> measured; its effect on the rows is unmeasured.

**What it is.** ADR-182's Tier 1 (then `BROOD_MONO=1`, off by default) devirtualizes an
ability-op call whose first argument is a LITERAL or a direct record-constructor call —
`(area (circle 2))` — to a direct impl call, the identity proven at compile time and the
resolution left behind the epoch-guarded cache (ADR-294, so a re-registered impl still
wins). Tier 2 would do the same for a receiver whose type the CHECKER inferred — `(area
s)` inside `(defn total (s) …)` under a `(sig total (circle -> float))` or a caller-derived
`circle` — which needs the compiler to ask the checker for a local's type at a call site:
the channel neither side has today (`NoSourceRewrites`: the checker reads the author's
code, the compiler rewrites it, and they never talk).

**Why it is here and not on the build list.** Its only value is the dispatch cost —
`identity-of` + `impl-for` per call — and every ability call site would pay a type query
at compile time. Without a row that moves, the channel is surface for nothing; and
`get` (~4 800 sites, the JIT cannot see through its type dispatch — CLAUDE.md's "single
highest-leverage perf item in the library") is the same cost class with a bigger footprint,
so the number to take first is whether devirtualizing ability calls moves anything
`get` would not move more.

**What to measure, in order.**

1. **The dispatch cost itself**, one process, warm: a hot loop over `(area s)` on a record
   on a *literal* receiver (Tier 1, always on now; the ceiling any Tier 2 can reach)
   against the dynamic call. If Tier 1's ceiling is under ~10% of the loop, stop — Tier 2
   cannot beat its own ceiling and the channel is not worth building.
2. **Whether Tier 1 is even on the path**: `BROOD_DEVIRT_DBG=1` on the rows that use
   abilities (`std/editor/*` under bedit, `format`, `stream`) — count devirtualizations.
   Tier 1 has never been measured on a real workload either (it was turned on for the
   first time on 2026-08-29 and found its own miscompile, ADR-294).
3. Only if (1) shows a ceiling worth having: prototype the channel as a side table the
   checker WRITES (call-site position → proven record id, from `check_file`'s derived
   types) and the compiler READS under `BROOD_MONO=2`, measure `make ab --floor --all`
   plus the bedit-derived rows, and gate it with the Tier 1 differential extended to
   inferred receivers (a wrong inference must deopt to the dynamic call, never miscompile:
   the id is a GUARD, the way a float-global guess is).

**A pass looks like:** a compute row with ability calls in its hot loop moves by more than
its floor under `BROOD_MONO=2`, `make ab --floor --all` reads noise everywhere else, and the
extended differential is green under sabotage (a wrong id in the side table must red it).
**A fail looks like:** step (1) under 10% — record the number here and close the item.

---


## Task 6 — symbols hash by spelling now (KI-166): confirm the rows on a box that benchmarks ✅ ANSWERED 2026-09-20 — neutral on every row; the micro's win survives at the native tier

**Answered on this box, the way this file says to: two fixed baseline binaries, adjacent
commits (`2fc5e13e` → `65598226`, the hash change alone), both stdlib images confirmed
`:live` in the same shell, interleaved best-of-7 pinned to cpu 2, with a base-vs-base
control (`A2`) as each row's floor:**

| row | before | before again (floor) | after | after vs before |
|---|---|---|---|---|
| `startup` | 15.5 ms | 15.1 (−2.7%) | 15.4 | −0.7% |
| `persistent-map` | 76.8 | 77.0 (+0.3%) | 77.2 | +0.5% |
| `json` | 172.1 | 170.8 (−0.8%) | 169.5 | −1.5% |
| `base64` | 78.3 | 77.4 (−1.3%) | 77.4 | −1.2% |
| `wordcount` | 65.9 | 65.3 (−0.9%) | 65.6 | −0.5% |
| `sort` | 111.8 | 111.6 (−0.2%) | 113.1 | +1.2% |
| `strings` | 29.1 | 28.9 (−1.0%) | 29.4 | +0.9% |
| `reduce` | 21.9 | 21.6 (−1.2%) | 22.3 | +1.7% |

Every delta is inside its own row's floor. The +1.0% boot instruction count does not reach
`startup`'s wall, and the map-heavy −15% reaches no row — none is keyword-hash-bound enough.

**Question 2, the micro at the native tier** (200k four-key `assoc` + 400k `get`, default
ceiling, `perf stat -e instructions:u -r 3` and five pinned wall samples): **1 054M → 876M
instructions (−17%), 133 → 123 ms wall (−8%)**. The win survives the `brood_rt_map_assoc` /
`brood_rt_map_get` callback boundary; the boundary halves it in wall terms.

The change is in the `1b9befd0` column refresh in `brood-benchmarks` (with five other
changes from the same day — which is why the isolated A/B above was needed to answer this
task rather than the column).


**Landed 2026-09-20, correctness-motivated, instruction counts only.** `value::symbol_hash`
makes a symbol's hash a function of its SPELLING rather than of its interned id, so a map's
iteration order stops depending on whether the prelude came from the image or from source
(KI-166 — a differential failed a full-suite run accusing the feature under test because of
it). The change is not optional tuning; the question here is only what it cost and what it
bought, in wall-clock, on rows this box cannot run.

**What was measured here** (callgrind, both arms from outside a checkout so the staleness
guard is skipped on each; `2fc5e13e` as the baseline):

| | baseline | after |
|---|---|---|
| boot — `(def x 1)` | 46.44M | 46.90M (+1.0%) |
| map-heavy loop — 200k `assoc` of four keyword keys + 400k `get` | 1414.6M | ~1.20G (−15%) |

The +1.0% is the FNV over each newly interned name, paid once per name at intern time. The
−15% is a keyword skipping `DefaultHasher`: it now takes the same scalar fast path `Int`
already had.

**What to ask on the benchmark box:**

1. `make ab --floor --all` against `2fc5e13e`. The rows to watch are the map-shaped ones
   (`persistent-map`, `json`, `base64`) for the win, and `startup` for the +1.0% — which is
   an instruction count on a boot, so it may well be inside that row's floor.
2. Whether the map-heavy −15% survives at the native tier. It was measured at the default
   ceiling, and the keyword hash reaches native code through `brood_rt_map_assoc` /
   `brood_rt_map_get`, so the callback boundary may dominate what the fast path saves.
3. The map figure is the soft number in that table — readings spread ~3% where boot's spread
   0.1%. If a row disagrees with the sign, trust the row, not the micro-benchmark.

**Not a blocker for anything.** The change is in for correctness and is gated by
`cli::prelude_image_matches_source::an_imaged_boot_and_a_source_boot_render_a_map_the_same_way`
(sabotage-verified). This task is bookkeeping: put a wall-clock number against a change that
only ever had an instruction count.
## How far back the bench corpus can measure (2026-09-11)

**`8a2aaa01` is no longer a fully usable baseline, and neither is any pre-2026-09-02 commit.**
The benchmark programs live in `brood-benchmarks` and track the *current* language; ADR-302/308's
data-first argument reorder means they call `(reduce coll init f)` where brood 0.19.1 expects
`(reduce f init coll)`. The baseline arm then dies with `type error: empty?: expected collection`
and `ab-bench` aborts the whole sweep on the first such row.

Measured by running every row against the `8a2aaa01` binary:

- **Reachable (19):** `ackermann bintree collatz errors errors-deep fib loop mandelbrot nbody
  pfib pingpong primes regex ring sieve sort spawn startup latency`
- **Unreachable (12):** `base64 json matmul nqueens persistent-map pipeline reduce strings
  wordcount spawn-live supervisor` (argument order) and `http` (times out)

Task 3's four rows are all reachable, which is why the 2026-09-04 re-baseline succeeded despite
ADR-302 having already landed — those four happen not to use the reordered combinators.

**The rule this earns, and it cuts both ways.** A bench row may only use names and argument
orders that have existed for as long as the oldest baseline anyone will measure against. A
language change that reorders or renames a std function silently shortens the corpus's reach,
and nothing reports it: brood-benchmarks' own CI builds brood from `main`, so a row that runs
only on `main` looks perfectly healthy. The same trap caught a fix of ours the same day — a
guard written with `failure?` (which arrived with ADR-310 on 2026-09-02) made every row
unrunnable on any older baseline, and only a perf sweep noticed. It was rewritten with `int?`
and `bytes?`, which predate 0.19.1.

---

## Reporting back

Put results in `docs/devlog.md` with the date, the exact `make ab` invocation, N, whether
rows were pinned, and the **floor for every row you quote**. A delta without its floor is not
a result — that is the single most common way this repo has been wrong about performance.

If a task here is settled, say so in this file and in the relevant `docs/known-issues.md`
entry, and delete the task rather than leaving it to be re-derived.

## Instruction counts CAN be taken on this box — `callgrind`, not `perf` (2026-09-18)

> **Reversed on 2026-09-20: `perf stat -e instructions:u` WORKS here now**
> (`/proc/sys/kernel/perf_event_paranoid` reads **1**), and `valgrind` is **not installed**
> (`valgrind: command not found`). Use `perf stat -e instructions:u -x, -r 3 <cmd>`; it is
> what measured ADR-371/372 and KI-167. The traps below still apply to either tool.

> **Read the sysctl; do not remember it. Both readings above have been true.** That
> reversal recorded `perf_event_paranoid` at **1**, and on 2026-09-20 later it reads
> **4** again — `perf stat -e instructions true` refuses, as an ordinary user and outside any
> sandbox — with no reboot in between (uptime from 2026-08-29), so something in userspace
> moves it and `sudo sysctl -w kernel.perf_event_paranoid=1` is the lever that does not stay.
> `valgrind` and `callgrind_annotate` ARE installed, whatever an earlier `command not found`
> reported, which makes `callgrind` the instrument always available here and `perf` the one to
> check for first:
> `cat /proc/sys/kernel/perf_event_paranoid; perf stat -e instructions true; command -v valgrind`.
> Quote a number from this box with the instrument that took it; the measurements above
> (ADR-371/372, KI-167) are not in doubt either way.

`perf` is unusable here: `/proc/sys/kernel/perf_event_paranoid` is **4**, so
`perf stat -e instructions:u` refuses without root. That is why this file says the box
cannot measure, and for wall-clock it still cannot. But **instruction counts do not need
`perf`**:

```sh
valgrind --tool=callgrind --callgrind-out-file=/dev/null <cmd> 2>&1 | grep -oE "refs: *[0-9,]+"
```

`callgrind` counts instructions **deterministically** — no thermal drift, no governor, no
core pinning, and it reproduces to the instruction across runs — which is exactly the
measurement this file's wall-clock warnings are about. It costs ~50× wall time, so it suits
a 100M-instruction `--check` or a short row, not a benchmark sweep. `valgrind 3.26` and
`callgrind_annotate` are installed.

**Two traps that made three contradictory readings of the SAME code in one session** —
read these before quoting any number from this box:

1. **A rebuild colds the prelude image** (ADR-314 keys it on the binary's own mtime), so the
   first runs after `cargo build` boot the prelude from SOURCE. On a one-line file that is
   **1.5 BILLION instructions against 78M** — a 19× difference that looks exactly like a
   catastrophic regression in whatever you just changed. Run the binary two or three times
   after every build before measuring (what `scripts/warm-boot-cache.sh` does for nextest).
2. **A `std/` edit or a new COMMIT moves the stdlib image id**, and the runtime then reads
   std from source until a `nest` invocation rebuilds it. Print `(stdimage/status)` and
   assert `:state :live` in the same shell as the measurement — not once at the start.

Both were hit here while measuring KI-150, and the second only surfaced because reverting
the change did not restore the baseline. **If a revert does not reproduce the number you
started from, the rig is wrong, not the code.**

## Task 7 — three 2026-09-21 runtime changes, measured on micros only: confirm the rows ✅ ANSWERED 2026-09-21 — the list rows are flat; `mandelbrot` −51%; `startup` +6% is real and attributed (KI-182)

**Answered with the benchmark refresh at `422c92a5`.** `make ab BASE=136b14d7 --floor` (the
previous column's commit, which brackets the three commits plus ADR-380/381/382), std image
live on both arms, best-of-7, then `startup` alone at N=15:

| row | base | new | delta | floor | verdict |
|---|---|---|---|---|---|
| reduce | 21 ms | 21 ms | +0.0% | 0.0% | noise |
| pipeline | 25 ms | 25 ms | +0.0% | 0.0% | noise |
| strings | 28 ms | 28 ms | +0.0% | 0.0% | noise |
| persistent-map | 69 ms | 72 ms | +4.3% | 0.0% | noise (rule) |
| wordcount | 60 ms | 62 ms | +3.3% | 1.7% | noise (rule) |
| nqueens | 103 ms | 107 ms | +3.9% | 1.0% | noise (rule) |
| mandelbrot | 175 ms | 86 ms | **−50.9%** | 1.1% | improved (ADR-378) |
| startup | 16 ms | 17 ms | **+6.2%** | 0.0% | regressed → **KI-182** |

The prelude change's fail signal (a list-heavy row past its floor) did not fire: `reduce`,
`pipeline` and `strings` are byte-flat, so `reverse`'s two predicate calls stay as they are.
Three rows read +3–4% in the same direction against sub-2% floors — under the rule, and noted
as a watch: re-check `persistent-map` and `nqueens` unpinned before the next refresh. The
`startup` movement is not the three commits at all; it is attributed on unstripped binaries
in KI-182 (the unarmed contract offer tiers `not` at boot; the prelude image's def sites).

The original brief, kept for the method:

### Task 7 as posed

**The situation.** Three commits on `main` (`62201c68`/`4492d313` the JIT, `d19ec13f` the
prelude, `93b7440d` the table) each earned their place on a microbenchmark and a downstream
game's step; none was run on the rows. Each touches something every row does:

- **JIT (ADR-378 + addendum).** Every `Prim2` `<`/`<=` with a `Handle` operand now lowers as
  `cmp_dispatch` (two tag tests, a branch each way) where it was one `as_int` guard; every
  `Prim2SlotSlot` comparison on two untyped slots likewise. The re-tier itself only fires on
  arms that used to latch BAILED, so it can only help — the dispatch cost on int-only arms is
  the question. Expect flat; `nqueens`/`bintree`/`json` are the comparison-heavy rows.
- **Prelude (`d19ec13f`).** `seq` gained one `type-of` + `%eq` before its old chain (a vector
  returns at once, everything else pays the two calls — a list ~60 ns more per `seq`); `reverse`
  gained a `type-of` and two predicate calls on the list path. `conj`/`into` on a vector
  copy once. `pipeline`, `reduce`, `wordcount`, `strings`, `persistent-map` are the rows.
- **Table (`93b7440d`).** `get` rebuilds under the lock; scalar keys compare without a rebuild.
  `sieve` (dense ints, untouched path) should be flat; anything hashed-path with compound
  values should not be slower.

**Run this.** `make ab BASE=a61d7762 --floor` (the commit before the JIT change), then the
three commits individually only if a row moves past its floor — `make ab BASE=<prev> --floor`
per commit. Unpinned as well for the JIT one (it does not change compile volume, but the
rule is the rule).

**A pass looks like:** every row inside `max(5%, 2 × floor)`; a `map`-heavy row a little
better. **A fail:** a list-heavy row (`reduce`, `pipeline`) past the floor — then the fix is
to fold `reverse`'s two predicate calls into one kind check, not to revert.

---

# Moved from `docs/handoff.md`, 2026-11-06

The handoff's state notes and open threads as they stood when the front moved to language
features, unedited. Dated claims are as of their dates (the state list is headed 2026-10-17 and
names a watch item and a release since superseded). The live perf leads are `Next: performance
items` and `Older threads` below.

## State (2026-10-17)

- **Easy pickings, done 2026-10-16** (devlog "a plain `assert`, the rustfmt 2024 reformat, the
  concurrency teaching note"): `assert` in the prelude, the `style_edition = "2024"` reformat,
  executed examples for 23 bare core names, and the tiling note in `brood-for-claude.md` with
  `examples/life_bands.blsp`. The docstring-example campaign continues module by module — the
  largest gaps are `editor/buffer` (101), `debug` and `test` (~40 each), `project`, `datetime`.

- **Latest work (2026-10-15/16, devlog "the call into native code, 32% cheaper" and "the
  interpreted call, 10% cheaper"):** the interpreter → native call 636 → 434 instructions
  (settled arms skip the tier-up; no shared-arm atomic write per entry), the interpreted call
  771 → 691. Rows: default ceiling `ring` −3.3%, `nbody`/`regex`/`spawn` ~−3%, `json` −1.6%;
  `BROOD_TIER=1` every row ≤ 0 but `sort` (+0.04%), `fib` −3.4%. Code generation itself is unchanged — the one IR
  change tried (`push_room` inlined) moved `json` +2.5% and was reverted. Six call-path
  experiments are recorded as measured-and-dropped; the remaining call costs are structural.
- **Don't build the debug profile while a suite runs:** `cargo build` relinks
  `target/debug/brood` under the running tests (it reddened `cli::stdimage_reporting`
  2026-10-16, reproduced on demand). Release builds (`make release-brood`) are safe.
- **2026-10-15/16, the maintenance review** (devlog entries of 2026-10-15, the last one
  "two bugs (KI-262, KI-263), one formatter walk, the 2024 edition"). Dependencies are on
  latest with MSRV 1.96, and the workspace is on the **2024 edition**, rustfmt's
  `style_edition` too (the reformat landed on its own, `27a24cef`). Two bugs were found
  and fixed:
  - **KI-262:** after `unlink` plus a zero-timeout flush, an `[:EXIT]` could still arrive
    later. Guard: `crates/lisp/tests/unlink_race.rs`.
  - **KI-263:** `nest format` deleted the blank line below a trailing comment.

  Large files were split by job, and the ADR log now lives in `docs/decisions/` by number
  range. `process.rs` documents the lock order in one table. All of this was verified with
  targeted tests on tiers 0–2, GC stress and fuzz. **Run on the other machine 2026-10-16:**
  `make test-both` found four reds, all stale tests from those moves (devlog "the merged
  tree's first full `test-both`"), fixed — now 1897/1897 on both engines. CI's red on
  `fec73f3f` was a test race, KI-267, also fixed.
- **Released:** v0.38.0 (`CHANGELOG.md`). On `main` since, merged 2026-10-14 from a branch
  developed apart from the release: preemption reaches every shape of running code (ADR-400..405,
  KI-230..238), one native entry and one quantum module, folds walked in spans, the gate sweep,
  and handoff items B (the fold span result stops allocating) and A (ADR-406, the computed-call
  memo). That branch's KI and ADR numbers were moved at the merge; commit messages from before
  it carry the old ones — the map is in the devlog entry "merged origin/main".
- `known-issues.md` lists **one watch entry, KI-281** (a JIT fast-link test that never published, once in CI; diagnostic armed) and nothing open (KI-279/280 fixed 2026-10-18). KI-247 (a bystander raising after a
  holder died) and KI-248 (a large allocation passing the hard limit when the live snapshot
  had not seen an earlier large block) were found and fixed 2026-10-14 (night); KI-264 (a native
  tail call by name never hit its head's cache slot) and KI-265 (a memory-pressure test that
  required the selector's stderr line when the culprit had raised in itself) and KI-266 (a stale
  leaf derivation refused for good) 2026-10-15. Those three were KI-250..252 on the other
  machine and were renumbered at the 2026-10-16 merge (this side's KI-250..255 came first).
- **Gates on the merged tree** (2026-10-14): `make test-both` 1859/1859 on both engines,
  `make gcstress`, fuzz (`tier_transition`, `metamorphic`, `pipeline`: 1 350 programs, clean),
  clippy `--all-features`, the `--no-default-features` checks, the JIT/frame/preemption/
  `process_floor`/`doc_refs` Rust tests, and the pre-push steps (`nest check` and `--strict` at
  zero, the 45 test files the merge touches). `nest format --check` no longer reads
  git-ignored files (`make prepush` used to stop on `stress/fuzz_out/fuzz_6.blsp`). On
  `0ba6c797` (2026-10-16): `test-both` 1882/1882 both engines, fuzz (four generators × 300,
  `stale=0`), `gcstress`, `check-stress`, `check-examples`, `check-imaged`, `breakagetests`
  (37/37) and `latency-gate` (worst p99 79 µs) all clean.
- **Memory ceiling:** decided (ADR-407). Inside a memory-capped container `brood`/`nest run`
  default their limits from the cap (hard = 3/4 of it), re-read every few seconds so a
  resized container moves them; elsewhere they stay unlimited. A table holding the memory is
  named in the `E0043`. A table now dies with the process that created it unless a global
  holds it (ADR-418), and a dropped table's slot is reused (KI-283).
- **Probes worth keeping:** every value walker survives data nested a million deep; a
  program in `unshare -U` with `ulimit -u N` gets an exact thread budget
  (`crates/cli/tests/thread_exhaustion.rs`); `systemd-run --user --scope -p MemoryMax=…`
  gives a real container cap locally.
- **The cleanup is finished** (devlog entries 2026-10-09 and both of 2026-10-12): the dead
  `BROOD_*` flags and aliases are gone — `BROOD_TIER=0|1|2` is the only tier knob — `nest
  mcp` is Brood, every Python script is gone, closed known-issue bodies live in
  [`archive/known-issues-closed.md`](known-issues-closed.md). Its last four items
  landed too: `tcp/close` silences the socket at once, the two message-copy walkers are one,
  ability devirtualization is always on (`BROOD_MONO` deleted), and the doc catalogue
  derives its categories. Nothing from it is deferred.
- **Before pushing:** `make prepush`, and `git merge --ff-only origin/main` first if it moved.
- **Is the tree green?** Answer with `make green`, never by reading the run list. On this
  machine run targeted tests only, capped (`( ulimit -v 16000000; … -j1 )`).
- **bedit at CI's pin (`d50442f`) passes 2464/2464** with a grammar-enabled `nest`
  (`make smoke-bedit NEST=…`; without `NEST=` the smoke picks a grammar-less one and the mode
  tests fail on "no grammar", which is not a regression). `make wasm-test` needs
  wasm-bindgen-cli at the playground's pin, 0.2.129. On this machine `~/.cargo/bin` holds
  0.2.100 (2026-10-17); `cargo install wasm-bindgen-cli --version 0.2.129 --locked` fixes it.

### Open threads, in value order

#### DONE (2026-11-06): the language design pass

A–D of ROADMAP's language design pass are done (commit `7252319a`, merged as `490a3641`). The
last two decisions: `()` stays `nil` with a `:list-truthiness` lint (ADR-620), and one grammar
for every `fn` head with in-order clauses and a compile error for a shadowed clause (ADR-619).
Also landed: fields on `failure` (ADR-617), macro errors at the call or the named sub-form
(ADR-618), and folds over vectors and ranges stopping on a failure (ADR-315 finished).

Loose ends from that session, for whoever picks up:
- **ADR numbers collided once:** the UI round took 613–616 upstream while this work was
  uncommitted, so it was renumbered 617–620 at merge. `git fetch` and read the newest ADR file
  on `origin/main` before numbering a new one.
- **pfib at tier 1 (`make ab-vm`) read +3–10% wall** across runs. Instruction counts (one
  worker, `BROOD_TIER=1 BROOD_J=1 BENCH_N=27`) settled it: every code change applied to HEAD is
  0.5% *under* a comment-only control build, and the working tree's +1.3% over that control
  comes from non-code content baked into the binary (std files pfib never runs, docs) — the
  same effect that lets one prelude comment move the count 1.9%. Not a regression; worth
  knowing when the next A/B shows pfib tier 1 moving.
- `target/ab-ctl/` (≈0.7 GB) is that control build — safe to delete.
- Not done: moving more of the prelude's hand-rolled macro checks onto pattern clauses (most
  check a list's contents, which a clause head cannot say) — move them as they are touched.

#### DONE (2026-10-21): a wait for another process's module load parks, at every tier

The last weakness the `bytes_literal_compiles` diagnosis turned up. `bbd6194f` hands a VM
global miss on a module another live process is loading (`derive::module_loading_elsewhere`)
to the prelude's `%lazy-global`/`%lazy-call` as ordinary frames, so the wait parks. The same
day this was carried to the rest: the tier-0 machine makes the same hand-off (and does not
redirect a thin wrapper whose inner head is mid-load), and the JIT's tier-up pre-load declines
rather than waits. Guards: `lazy_load_wait_parks.rs` (VM, stdlib image) and
`autoload_parks.rs` (tiers 0/1/2, and the tier-up); devlog 2026-10-21. **Later the same
day the claim race closed too:** a waiter whose claim is taken between the loading-elsewhere
check and `require-one`'s CAS hands off rather than waiting (`derive::parkable_miss`,
`%load-may-hand-off?`; guard: the six `…loses_the_claim_race…` cases, held open by
`BROOD_FAULT_CLAIM_RACE`). Still blocking, by the general dirty-block carve-out (ADR-100
§7.4) rather than anything load-specific: a load inferred at COMPILE time (a qualified macro
head) and a miss in a nested (non-capturing) run — devlog 2026-10-21 (later) says why.

#### DONE (2026-10-19): the stepping tree-walker, ADR-416 (the user chose option B)

All five stages are in (devlog 2026-10-19, stage 1 through "late night"). At `BROOD_TIER=0`
every process body and top-level form runs on the tree-walker's capturing machine
(`eval/machine.rs`): its `receive` parks and its budget preempts, so tier 0 is the reference
semantics for whole programs and `make test-both`'s tree-walker half finally runs test
bodies on the tree-walker. The escape audit (`BROOD_TIER0_AUDIT=<file>`) over the whole
workspace at tier 0 records only a Rust unit test that drives `vm_run_bc` itself.

Running test bodies on the tree-walker found a real, older `let` divergence (a `fn` lost its
own name after a list-valued binding; fixed) and re-sized three budgets (the tree-walker's
frame cap `1 << 19`; `quantum_tail_panic.rs`'s spinners; the suite wrapper's nextest cap,
1800 s, for 370 s measured). **Measured 2026-10-20** (instruction counts, devlog "what ADR-416 costs at tier 0"): +3–10% at
tier 0 on single-process rows, `pfib` 11× (its bodies now really tree-walk), the default ceiling
flat. Fixed 2026-10-20: a global read no
longer scans the dynamic stack for names not on it, and a tree-walked level costs ~600 B
(was ~1 KB; the VM's ~220 B — the rest is an env frame per call and the root stack).

**Done 2026-10-20, the user's item 1:** the slow gates on this code — `make gcstress` (both
ceilings), the fuzzers, the dist tests looped, `make runtime-soak` — all clean (devlog
2026-10-20).

#### DONE (2026-10-17): the design promises without a guard now have one

An audit mapped the design's promises to tests and closed the eight that had none (devlog
2026-10-17 "every design promise has a guard"). Two were real bugs, both fixed: KI-272 (a
`receive` on a decimal, ratio or bignum literal raised on every message) and KI-273 (a crash
reason dropped what the process threw). The standard suite (`make test`) stays inside its
5-minute budget: the new per-push tests cost a few seconds. The slow ones are separate gates:

- **`make test-contracts`** — the suite with every `sig` armed, as `nest test` users get it.
  CI job `contracts-armed`; `green-all`. Not in `make test`: armed costs twice the CPU.
- **`make gcstress-all`** — every test file under GC stress (~80 s at -P 6). Nightly and
  `green-all`; `make gcstress` (the twelve process files) stays per push.
- **`tier-audit`** — nightly now too (checks out `brood-benchmarks`), with the checkout and a
  native tier asserted first so it cannot pass vacuously. **`latency-gate`** joins `green-all`
  only: it is a benchmark-box gate, and a shared runner's p99 means nothing (benchmarking.md).

**No KI is open; KI-281 is a watch** (see above). The 2026-10-17 stability review found and fixed KI-271 (the run-check cache sweep
held every boot), KI-274 (a native self-loop never saw a `def` of itself), KI-275 (an edited
`include-str` file did not stale the project image) and KI-276 (the project image was not keyed
by contract mode). The nightly is
green again: cargo-deny (a yanked crate), and ASAN, whose job now runs through nextest
(`make asan`, profile `asan`) because `cargo test`'s one shared process broke tests written for
process-per-test.

**Found and then done:** `monitor` on a remote `{:name :node}` address refused ("pass the pid"),
so `gen/call` by name across nodes failed at once. ADR-411 adds `Frame::MonitorName`, resolved by
the peer; guard `gen_call_round_trips_across_nodes_by_pid_and_by_name`.

#### DONE (2026-10-16): `exec_value`, the VM's second executor, is retired

The bytecode loop is the VM's only executor (devlog 2026-10-16 "exec_value retired"). A
top-level form compiles to a chunk and runs through `vm_apply` (a throwaway arm, `BAILED` so
it never tiers); a real `&optional` default is a prologue in its arm's own bytecode —
`push_frame` sets a hidden flag slot per missing defaulted optional (`optional_flags`), the
prologue tests it by truthiness and sets the slot. `exec_call` went with it; the prim helpers
are `eval/compile/prim_exec.rs`. Guard: `tests/optional_defaults_test.blsp`.

#### DONE (2026-10-16): `receive` on the native path (ADR-409, ADR-410)

Two items, taken in order. **ADR-409:** a `receive` whose clauses are all simple shapes hands
the kernel a pattern spec instead of a matcher closure (`process/receive_spec.rs`). **ADR-410:**
an arm that receives at its head, `(defn loop (state) (receive …))`, is compiled to native code.
Its park is captured at ip 0 by the top-level driver, and only an entry that can unwind runs it
natively (`jit_tier_in_frame`). `ring` −19.4% and `pingpong` −7.3% for ADR-410, on top of
ADR-409's −2% and −3.7%. Guards: `tests/receive_spec_test.blsp`, `tests/receive_loop_jit_test.blsp`.
Left on the messaging path: a loop that does work before its `receive` (a `send` and then a
`receive`, the `gen/call` shape) stays on the VM. **Measured and dropped 2026-10-17:** admitting
it by resuming a park at the last deopt checkpoint (`060ed7c0`, undone the next commit) read
`pingpong` +1.9% and `supervisor` +4.8%. Such a loop parks on nearly every iteration, so native
code only adds crossings. Making it pay would need the wake to re-enter native code *at* the
`receive`: a native continuation, not a VM resume.

#### Next: performance items (A and B the user's pick, 2026-10-13; C found doing A)

Measure both by **instruction count** first — `perf stat -e instructions:u` with
`BROOD_NO_CHECK=1` (the run pre-flight walk is noise), each binary's stdlib image live (`nest
stdimage`; `make ab` now writes both arms' images itself), the timing binary from
`make release-brood`. Times on this box are noisy; confirm with `make ab --floor` and, for
anything on the scheduler path, `make latency-gate`. `perf record` is capped at 1 kHz here:
aggregate 20-40 runs, and build `make perf-symbols` for names.

**A. DONE (2026-10-14, ADR-406, devlog "native code memoises calls to a closure value").**
`jit_dispatch_call` memoises a computed-head call's arm, env and IC block per closure instance:
~240 instructions off every such call (anonymous closure 943 → 706; named heads 415). What
remains of the gap to a named call is `fill_captures`, `finish_nested`, `native_feedback` and the
unconditional stack probe — an in-IR fast link for computed heads (a site id per computed call,
the handoff's original design) is the next step there; check `staged` (KI-19) and
`mono_devirtualize` when allocating the site.

**C. DONE (2026-10-14, ADR-408).** Native code runs a thin wrapper's compiled arm instead of
the dispatcher's by-name redirect, so the wrapper tiers and links natively: `(fn (acc x) (+ acc
x))` as a callback 1 892 → 714 instructions a call. The interpreters keep the redirect.

**B. DONE (2026-10-13, devlog "(later) — the fold span result stops allocating").** A finished
span returns the accumulator; a stopped one returns `(:%fold-more acc . next)`. `nqueens` −1.3%,
one-span folds ~145 instructions cheaper each, `reduce` flat. Two leftovers, neither urgent:
`Reducer::resolve` per span (`reduce_prim_op` re-derives `+`'s primitive every span), and the
stopped-short path's `first`/`rest` are runtime callbacks (~150 instructions a span, once per
4 096 elements). **Read the devlog's measurement lesson before trusting a few-percent
instruction delta between two builds:** a comment-only prelude edit moved rows by up to 2.3%.

**D. DONE (2026-10-14, night).** `reduce` takes the counted range loop (`nqueens` 113 → 90
ms); a named thin wrapper links from native code (~2 100 → ~420 instructions a call); the
CLIF verifier is off in release (`base64`/`json` −11% pinned); a fold's native callback
finishes inline (746 → 617 instructions an element). Leads left, measured: the fold callback
is still ~617 instructions an element against ~412 for a native→native call by name — the
remaining Rust entry ceremony (`enter_native`, frame nil-fill, captures), whose amortisation
would reshape ADR-403's per-element preemption; and a computed-head call from native code is
652 against ~410 by name (771 before ADR-406's 2026-10-14 amendment). **The in-IR fast link
for computed heads is measured low-value on the corpus:** computed-head calls from native code
per run are `supervisor` 57k, `persistent-map` 50k, `base64` 48k, `json` 25k, `wordcount` 1k,
every other row ~0 — at ~240 instructions saved each, ≤2% of any row. Worth it for user code
built on higher-order loops, not for the benchmark column. `nqueens`' `solve` stays
interpreted (it builds its loop closure; §7.1).

**E. DONE (2026-10-15) — `json`, picked apart by counters.** KI-264 (a native tail call by
name caches its head), KI-266 (a stale leaf derivation is re-validated, not refused for good),
the unlinkable-call slow path reads the call IC instead of walking the environment, `nth`'s
default is a fixed parameter, and a queued callback compiles synchronously after 2 048 declines.
`json` −5.1% instructions on top of KI-264's rows. What is left on `json` (N=20 000, counter
build): `vm_apply` ~470 000 — the functions the native compiler refuses
(`call-mediated-boxed`: `json-value`, `json-slice`, `enc-key`, `enc-string`, `->string`,
`reverse`; `chunk-outside-jit-subset`: `enc-escapes`, `count`), each called from native code
through the slow path. Followed 2026-10-15 (evening): the gate is right (§7.1), so the
boundary was cut instead — a native → interpreted → native round trip −23% (devlog). Left:
the nested `vm_run_bc` trampoline per crossing and `jit_tier_in_frame`'s entry ceremony
(older thread 2), and `fast_link_from_entry` re-checking a callee that is not native on
every call. A spike (devlog 2026-10-15 night) measured that resuming native code through the
driver instead of nesting would be break-even for compute: the lever is the per-call
constants — 768 instructions an interpreted call, 640 into native code, ~410 native to native.
The middle one is now 434 (devlog 2026-10-15 late: settled arms skip the tier-up, the shared
atomic writes are gone); what is left of it is `exec_chunk`'s call-site share (IC probe + its
`Arc` clone, the frame fill), `enter_native`'s save/restore and the stack stamp. Next: the
768 of an interpreted call, then native to native.
The interpreted call is now 691 (devlog 2026-10-16; the quantum TLS merge, a fast-link base
mirror and an in-IR `push_room` were measured and dropped there — the last moved `json` +2.5%).
Native to native is ~255 once the xcall re-lowering lands; a direct i64-convention call that
skips the callee's boxed wrapper was the structural idea, but the wrapper is 0.0% of every
row (an i64 worker is entered once and recurses in registers) — not worth building for the
corpus.

#### Older threads

1. **Receive loops run interpreted — DONE for the receive-at-head shape (ADR-410,
   2026-10-16).** A loop that does work before its `receive` (send then receive) still runs
   interpreted; see the DONE entry above.
2. **The rest of the VM call** (~790 instructions after `CallInPlace`): `exec_chunk`'s `Call` +
   callee entry, and `vm_run_bc`'s frame push/pop. The cheap forms were built, measured and
   dropped (devlog 2026-09-25 "(later)"); what remains is structural.
3. **Per-message residue:** `receive_match`, `run_one`, allocation, `Ctx` install — each small;
   count the calls before touching any of them. Profiled 2026-10-16 (`ring`, precise
   sampling): `receive_match` is 9.7% of the row with no source line above 0.6% — no
   hotspot to cut; `run_one` 3.8%, `send` 2.3%, the `Ctx` thread-local + its drop ~2.8%.
   The interpreter running the process bodies (`exec_chunk` 30%) is thread 1's lever.
4. **Refresh the benchmark column** once 1 or 2 lands (protocol in brood-benchmarks'
   `CLAUDE.md`). Not on this machine.
5. **Done 2026-10-18:** the tree-walker no longer reports a contract shim's position
   (devlog "the tree-walker stops reporting contract shims").
6. **Done 2026-10-19 — ADR-416 (see "DONE" above).** `BROOD_TIER=0` did not reach process bodies. `run_process_body` runs
   a VM-eligible body on the VM whatever the ceiling, so `make test-both`'s tree-walker half
   and CI's differential job tree-walk only each test file's top-level forms: every test
   body runs on the bytecode VM. Either tree-walk process bodies at tier 0 (a `receive` then
   blocks its worker; the suite's many-process tests may starve) or document that tier 0 is
   top-level only and say what the differential covers. A tree-walker guard has to pin the
   ceiling itself (`set_forced_ceiling`), as `contract_error_position.rs` does.
7. **Perf lead from the 099abd59 column:** `ef80547d` (KI-274) costs `collatz` +6.3% and
   `primes` +3.0% in instructions — per iteration, not per poll; check whether the entry
   epoch held across the self-loop spills.
