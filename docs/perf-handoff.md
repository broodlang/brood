# Perf handoff — work that must run on a benchmark box

**Why this file exists.** The primary development machine for this repo does not run
benchmarks: it is a 28-core workstation shared with other work, its thermal and cache state
drifts across a session (a `pingpong` baseline wandered ~10% across one day's runs), and it
has repeatedly produced confident, wrong perf verdicts. So perf verification is *deferred*
rather than skipped, and this file is the queue. Anything here needs a quiet, pinned box.

Read `docs/benchmarking.md` for *how* to measure and the `Commands` section of `CLAUDE.md`
for the traps. This file is only *what* to measure, *why*, and *what a pass looks like*.

---

## Before anything: three ways the measurement lies

These have each cost a wrong verdict in this repo. They are not general advice.

1. **`make doctor` first.** A stale binary fails by *agreeing with the baseline* — the most
   convincing possible way to be wrong. `make ab` aborts if the two binaries come out
   byte-identical, which is what a silently no-op'd build looks like, but it cannot catch a
   binary that is merely old.
2. **`make release-brood` before any timing — never time after `make perf-brood`.** They
   write the *same path* (`target/release-fast/brood`), so the moment you profile anything,
   the binary you go on to time is the counter-armed one and your change is charged ~10% for
   atomics it never introduced. This needs no command-line slip: profile, then time, and the
   bias is there. On 2026-08-27 it showed two behaviourally identical binaries at 958 vs
   1018 ms.
3. **Prove the floor before believing a delta.** `make ab --floor` runs the baseline twice
   and reports that row's own base-vs-base spread; a regression counts only past
   `max(5%, 2 × floor)`. For a suspicious row, keep the `target/ab/<sha>/…/brood` binary and
   run base / base / new pinned best-of-15 — a row that read +5.3% "confirmed" solo turned
   out to be +0.9% against a +0.5% floor once the baseline stopped wandering.

Also: **`make ab` pins compute rows to one core**, which charges the benchmark for background
JIT compilation. Right for judging generated-code quality, wrong for any change that alters
*how much* the compiler does. If the change touches tiering, re-run the row unpinned.
`BROOD_JIT_DUMP_IR=1 … | grep -c '^\[jit-ir\]'` counts the compiles.

---

## What this box CAN answer (and how) — read before deferring anything here

Deferring a question to a benchmark box is right for a *magnitude* claim and wrong for a
*mechanism* one. Most entries that landed in this file were mechanism questions wearing a
number. The order to try:

1. **Turn it into a structural question.** Did the arm lower (`BROOD_JIT_DUMP_IR=1`, count
   `[jit-ir]`)? Was it refused, and for which reason (`BROOD_JIT_BAIL_TRACE=1`)? Did it lower
   and then fall back (`BROOD_DEOPT_TRACE=1`, needs `perf-stats`)? These are binary and
   load-immune, and the counter-armed build's overhead does not matter because nothing is being
   timed — so the `make perf-brood`/`make release-brood` same-path trap is also irrelevant here.
   Task 1 above was answered this way after being queued as unanswerable.
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

What is genuinely left for a quiet, pinned box: absolute cross-process deltas of a few percent,
i.e. `make ab --floor` sweep verdicts. Nothing else in this file needs one.

---

## Task 1 — does KI-114's fix hold KI-109's closure? (the only open question)

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
> | native path carries the work | default vs `BROOD_NO_JIT=1`, 3 runs each | **0.21 s vs ~2.0 s (~9.5x)** |
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

## Task 2 — two lowerings the KI-114 fix changed with no test reaching them

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

---

## Task 3 — re-take KI-100's re-baseline if the runtime has moved

**Priority: low.** KI-100 was resolved as filed on 2026-09-04 by re-baselining against
`8a2aaa01`: `startup` −18.2%, `sort` −7.6%, `fib` −4.2%, `bintree` −2.9%. Those numbers
predate ADR-318 (the tree-walker→VM router, default-on 2026-09-04), the prelude image
becoming default, and everything since. Nothing suggests they have moved; nobody has
checked. This is hygiene, not a suspicion.

---

## Reporting back

Put results in `docs/devlog.md` with the date, the exact `make ab` invocation, N, whether
rows were pinned, and the **floor for every row you quote**. A delta without its floor is not
a result — that is the single most common way this repo has been wrong about performance.

If a task here is settled, say so in this file and in the relevant `docs/known-issues.md`
entry, and delete the task rather than leaving it to be re-derived.
