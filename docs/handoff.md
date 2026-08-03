# Handoff — current state, open threads, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); the option book lives in
[`runtime-frontier.md`](runtime-frontier.md). Read this to pick the work back up cold.

**As of 2026-08-03**, brood `9e0d1f4b`, brood-benchmarks `71c21a9`. Nothing half-finished.
Suite **942/942**, `nest check` clean, `nest format --check` clean, rustfmt clean.

**There are no open correctness bugs.** KI-22, KI-23 and KI-24 all closed. Every `std/` scale
sweep row is linear. The four perf threads this document used to carry are resolved: two were
fixed, one dissolved when another was fixed, and one turned out to be measuring the wrong
thing. **One item is genuinely open** (§3), and it is deliberately gated rather than started.

---

## 1. What shipped, and how to turn each piece off

Every mechanism has an off-switch, because each is an optimisation whose fallback is the old
behaviour. If something misbehaves, bisect with these **before** bisecting commits.

| Change | Off-switch | Worth |
|---|---|---|
| Shared closure crosses a **serialised** send by handle (ADR-208) | `BROOD_NO_SHARE_FN_MSG=1` | `rt_closures` **143,752 → 66 constant**; throughput flat ~24k vs decaying ~13.8k; RSS 213 vs 502 MB |
| Idle peer told at once that a peer queued a child | `BROOD_NO_STEAL_WAKE=1` | `latency` p50 27 → 19 µs, p99 124 → 78 µs |
| Owner keeps first refusal on a fresh child | `BROOD_STEAL_GRACE_NS=<n>` (0 disables) | protects `supervisor`; see the cliff below |
| Closure sends share already-shared code, **parked** path (ADR-194) | `BROOD_NO_SHARE_FN=1` | retained closure 436 B → 48 B |
| Spawn placement spills off a backlogged worker | `BROOD_SPAWN_SPILL=999999` / `BROOD_SPAWN_RR=1` | `latency` p50 5×, p99 2.9× |
| Receive-mark (ADR-195) | `BROOD_NO_RECV_MARK=1` | backlogged reply O(backlog) → **O(1)**, 653 → 4 µs at 32k |
| Registry updates are atomic (KI-22/23, ADR-202) | — (correctness) | ~40% of concurrent registrations were being lost |
| Framed reads use a straddle probe (ADR-142 correction) | — (bug fix) | drip-fed frame O(total²) → O(total) |

**The `STEAL_GRACE_NS` cliff, because it is the one tuned constant here.** Swept on `latency`
p50 / `supervisor`: grace 0 → 10 µs / 2607 ms · 1–2 µs → 10 µs / ~2106 ms · **2.5 µs → 13 µs /
1005 ms** · 5 µs (default) → 19 µs / 1005 ms · 10 µs → 22 µs / 1006 ms. 2.5 µs is the best point
*on this machine* and is deliberately **not** the default: the cliff sits there because that is
how long that benchmark's parent takes to reach its `receive`, which is a property of the
workload. The failure is asymmetric — too small costs 2.3× throughput, too large costs a few µs
of p50 — so the default takes a 2× margin.

**Five `std/` quadratics fixed**, each three-point confirmed with a regression row in
`scale_sweep.blsp`: `template/render` 318→24 ms, `last-index-of` 540→1 ms, `strip-ansi`
1583→109 ms, `stream-lines` 303→39 ms (quadratic *inside one chunk*, which is the normal case
for a 64 KB socket read), `format-source` 3593→1988 ms. Underneath them, the string kernel: a
cached **char count** on every heap string slot (so `string-length` is O(1) and `chars == bytes`
is an O(1) pure-ASCII test) and **`expect_string_ref`** — `expect_string` returns an *owned*
`String`, so every string builtin was copying its whole argument per call.

## 2. Where we stand against the field

From the published run (`brood-benchmarks/results/`, 2026-08-02):

- **`latency`** (open-loop, 5% of requests occupying 500 µs), ranked by p99 — Elixir 58 µs,
  **Brood 78 µs**, Node 461, Python 485, .NET 783. Brood is 2nd of five and **5.9× ahead of
  third**. p50 19 µs against Elixir's 8.
- **`supervisor`** — Brood 876 ms vs Elixir 439 ms. **Neutral to thread 6's fix** (820 vs
  831 ms pinned) because it runs ~25,000 operations where the decay needs ~175,000 to appear.
  Thread 6 is a *sustained-load* win; no burst benchmark will show it. Don't go looking for it.
- **Compute aggregate** — 2.9× the fastest, 3rd of seven, ahead of Elixir.
- **`spawn-live`** remains the worst row: 2.8× slower and 1.9× heavier than the BEAM.

## 3. Open threads, in the order I'd take them

**1. Partial leaf splicing in the JIT — the only genuinely open perf item, and deliberately
not started.** `leaf_inline_derive` bails if *any* non-tail `Call` survives splicing, so **one
un-spliceable callee blocks inlining of every small callee beside it**. Measured with
`BROOD_INLINE_DBG=1`: an arm whose only non-tail call is a three-instruction leaf inlines and
runs 196 ms/1M; add a recursive callee and the probe vanishes entirely, at 588 ms. That is where
`mandelbrot`'s `->float` cost lives (~85 ns a conversion, where every other language in the
suite emits a machine cast).

Why it is gated rather than queued: the bail is *sound*. `jit_ckpt_read` documents that the
inlined layout's `ckpt_slot` **points into the spliced slot range**, where a spliced callee's
Int param would fake a journal and produce a garbage resume ip — so an inlined native must
resume from ip 0, which is only effect-free when no call completed. Allowing partial splicing
means re-laying-out the checkpoint area and enabling journaling in the inlined lowering, i.e.
changing **deopt-resume correctness**. That is the one place in this tree where the failure mode
is a *silent miscompile* rather than a crash or a leak (bug #2, KI-11, KI-18, KI-20), and the
validation story is weaker than elsewhere — no self-checking soak, only the four-config
differential fuzzer. **Make that fuzzer the explicit gate before starting.** Write-up in
[`jit-optimizing-tier.md`](jit-optimizing-tier.md).

**2. Per-process memory floor — measured, two experiments named.** The idle floor is
**4.19 KB** (slope of RSS against process count: 4389 / 4186 / 4195 at N = 10k/40k/80k). It is
**not** allocator retention — `MIMALLOC_PURGE_DELAY=0` leaves the slope at 4230 / 4120.
Attribution: `Process` 1304 B (of which **`Heap` 1200 B, embedded by value**), `Mailbox` 184 B,
`Suspended` 136 B = 1624 B structural; the remaining ~2566 B is not data (`process-info
:memory` for an idle process is **64 bytes**) but per-allocation overhead across the ~25–30
distinct blocks a process owns. Next: **(a)** box the `Heap` inside `Process` — `Process` drops
to ~112 B and `Heap` becomes its own block; if size-class rounding dominates the floor falls,
and if the indirection costs more that is also a result. **(b)** Only then the allocator
size-class histogram. (a) is mechanical and tells you whether (b) is worth building. Guarded by
`per_process_floor_is_attributed`, which exists because `Heap` being inline means **any new
`Heap` field costs one per live process**.

**3. Finish the `std/` sweep.** Unswept: the rest of `std/tool/*`, `editor/*` beyond buffer,
`std/net/*` beyond the framed reads. The method that worked: grep for the shape (an
`append`/`concat`/`str` whose argument is the **accumulator**, or char-indexed access on a UTF-8
string), then confirm on **three points** before believing it. Hit rate was five for five, but
the obvious shapes are now gone. ~115 `expect_string` call sites still copy their argument —
convert only when a workload shows the cost, since it bites only on large strings.

**4. `spawn-live`** — the worst published row, untouched. Its own noise floor is **20.6%**, so
nothing smaller is resolvable there; it has produced phantom results repeatedly.

**Explicitly NOT open:** a memory leak (chased, does not exist), endurance (16/16 soak,
12.7 M iterations), thread 6's throughput decay (fixed, ADR-208), the RUNTIME reclamation
threshold (dissolved — `BROOD_RT_GC_FLOOR` is inert across a 128× range now that the region does
not grow), and per-message cost as an explanation for `latency` (measured: send+receive 1.1 µs,
round trip 4.3 µs — it was spawn placement).

## 4. Tools, and how to use them

All in `scripts/fuzz/stress/`, each with a usage header:

- **`soak_selfcheck.blsp`** — sustained load, an invariant checked every iteration; prints
  `ERROR at iteration N` and halts. **Always pair it with a control** reverting the mechanisms
  under test; without one an alarming RSS curve cannot be attributed. Thread 6 was validated
  this way: 1.78 M iterations across armed and control, zero violations.
- **`decay_isolate.blsp`** — one operation per `MODE`, throughput per fixed-size window plus RSS
  and `:runtime-closures`. The harness that found thread 6 and proved it fixed. Run modes
  **sequentially**.
- **`scale_sweep.blsp`** — a `std/` op at N and 4N, ratio printed (linear ~4×, quadratic ~16×).
  **Read its header first**; it records which rows are cleared and why, including the one that
  was cleared *wrongly*.
- **`receive_backlog.blsp`** — the receive-mark's benchmark; ~4 µs at any backlog.
- **`net_framed_scale.blsp`** — framed reads; ns/chunk must be flat, carries its own controls.
- **`tests/registry_test.blsp`** — carries its own control (a plain `def` rebind, asserted to
  lose entries) so the suite cannot go green on a regressed mechanism.
- **`tests/shared_closure_msg_test.blsp`** — ADR-208; passes with the mechanism on and **fails
  with it off**, which is the only version worth having.
- **`eval_forward_ref.blsp`** — KI-24's reproducer.
- Plus `scripts/fuzz/run.sh <generator>` (differential across 4 engine configs) and
  `dist_chaos*.sh`. `make ab BASE=<ref>` for brood-vs-brood rows; `bench/harness.py` in
  brood-benchmarks for the published cross-language numbers.

## 5. Traps — every one of these cost real time

**Measurement**

- **A full disk looks like a toolchain crash.** `make test` died three times with
  `collect2: fatal error: ld terminated with signal 7 [Bus error], core dumped`. That is what a
  100%-full filesystem looks like from inside the linker; it sent me to an LLVM stack dump
  before `df`. **Check `df` first.** Also: `make ab` leaves ~1.1 GB per baseline worktree and
  `make ab-clean` is **not** automatic — one bisect left 7.7 GB behind; `target/debug` reached
  88 GB.
- **Load contaminates everything; a few-percent delta measured under load is worthless.** Three
  times in one session: `ring` "regressed" 12%, `supervisor` "regressed" 8%, a sweep row moved
  3.7% — all gone on a quiet machine. Wait for load < 0.5 and A/B under identical conditions.
- **Measure the slope, not the ratio.** RSS/N folds the runtime's ~24 MB base into a
  per-process figure; that is how the memory floor was recorded as 5.9 KB when it is 4.19 KB.
- **A ratio near 4× that RISES across bases is not linear.** `format-source` read
  3.80/4.12/4.64 and was cleared as linear; pushing the base gave 4.46 then **6.40**. Only a
  *falling* ratio (warm-up) clears a row. Check the trend across triples, not one triple.
- **Never difference time-boxed runs** — RSS tracks iterations, so the comparison measures the
  iteration count. This produced two wrong versions of frontier A8.
- **Establish the noise floor before believing a delta.** Base-vs-base first.
- **`RSS is not a proxy for live bytes`** here — but check before blaming the allocator: for the
  per-process floor, `MIMALLOC_PURGE_DELAY=0` moved the slope by 2%.

**Testing**

- **A green test proves nothing until you run it with the mechanism off.** Twice in one session.
  The shared-closure test passed *identically* with `BROOD_NO_SHARE_FN_MSG=1`, because the
  closure it sent merely computed — growth requires the sent closure to itself `spawn`. An
  earlier version measured its own harness: `(spawn (busy-worker me))` captures `me`, so the test
  promoted a closure per iteration by itself. **For any mechanism with an off-switch, run the
  test with the switch off before committing it.**
- **`std/*.blsp` is embedded in the binary at build time.** Rebuild `brood` *and* `nest` after
  touching `std/`, or you will debug yesterday's bytes — a flake check read 4/12 failures against
  a stale binary. Same class as `-p brood` vs `--bin brood`.
- **Verify a detector before trusting it.** One you have never seen trip is not a detector.
- **Process death reports go to stdout** — `2>/dev/null` will not filter them; pipe through
  `grep -v 'died: error'`.
- **`pkill -f <pattern>` matches your own shell.** Use `pgrep -f "harness[.]py"`, kill by PID.

**Diagnosis**

- **Threads get named after the mechanism nearest the symptom, not the cause.** Two of four were
  misnamed: "per-message cost" was spawn placement (send+receive is 1.1 µs), and the
  "reclamation threshold" was thread 6's growth. Re-derive a thread's premise before implementing
  against it — 15 minutes closed one that was carrying a plan.
- **A comment asserting a cost is not evidence.** `fuzzy--next` claimed its scan was "the
  dominant cost of ranking"; making that scan much cheaper moved ranking 116 → 106 ms, and phase
  measurement showed `fuzzy-match` was 78 ms of a 112 ms total. Two of my own predictions were
  built on that comment.
- **Read the existing argument before inventing one.** ADR-194's comment stated exactly why
  sharing is sound on the parked path, which identified precisely what the serialised path lacked
  (the queued window). `report_gen_liveness` then explained why extending the reachability probe
  would not have been enough on its own.
- **A benchmark port drifts silently when language semantics change under it.** `mandelbrot`
  looked like a 3.5× runtime regression bisected to kernel exact rationals; it was not a runtime
  regression at all (identical source measures 201 vs 200 ms). `(/ px n)` had simply stopped
  being a float divide. Nothing failed, the checksum never moved, the row just stopped measuring
  what it claimed. **When a numeric primitive's semantics change, grep the benchmark ports.**

## 6. Semantics worth knowing (documented, not bugs)

- **Hot reload does not reach a self-recursive loop.** A tail self-call compiles to
  `Node::SelfCall`, which re-runs the arm without resolving the callee. Redefining any *other*
  global the loop calls does reach it. Erlang's local-vs-remote rule; see `live-editing.md`.
- **A closure that captures no locals is already shared code**; one that captures a local is
  copied on send. That is why supervisor `:start` thunks should avoid captures — ADR-194/208.
- **`/` is exact.** `(/ 3 4)` is the rational `3/4` (ADR-196); `(/ 4 2)` is `2`. Use `quot` for
  an integer count, and convert to float *before* dividing in a float pipeline — a rational built
  per iteration and immediately converted is pure waste.
- **`->float` is a function call, not a cast** (~85 ns), and is not inlined when the arm also
  calls something un-spliceable — §3 item 1.
- **`substring` is O(result) on ASCII, O(index) otherwise**, so a char-by-char scan is linear on
  ASCII text and quadratic on multi-byte (measured 1.16/3.85 vs 10.50/11.52).
- **Duplicate supervisor `:id`s** resolve to the later-started child.
