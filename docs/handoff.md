# Handoff — current state, open threads, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); the option book lives in
[`runtime-frontier.md`](runtime-frontier.md). Read this to pick the work back up cold.

**As of 2026-08-03**, brood at ADR-210, brood-benchmarks `71c21a9`. Nothing half-finished.
Rust suite **943/943**, in-language suite **4350/4350** (with the new mechanism on *and* off),
`nest check` clean, rustfmt clean, `nest format --check` clean, and both `--no-default-features`
and default builds warning-clean.

**There are no open correctness bugs.** KI-22, KI-23 and KI-24 all closed. Every `std/` scale
sweep row is linear. **The item §3 used to carry as the one genuinely open thread — partial leaf
splicing — shipped as ADR-210**; §3 item 1 now records what it actually took, including two
rules that are easy to re-break. The remaining §3 entries are the memory floor, the unfinished
`std/` sweep, `spawn-live`, and one easy test-hygiene fix.

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
| Fast-link deopt shape check is flag-free (KI-26) | — (correctness) | a peer's stale link no longer re-runs a journaled effect from ip 0 |

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

**1. ~~Partial leaf splicing~~ — DONE 2026-08-03 (ADR-210), default ON,
`BROOD_NO_PARTIAL_LEAF=1` off-switch.** A derivation may now keep a residual non-tail call, so
one un-spliceable callee no longer blocks inlining of every small callee beside it. **Measured
2.4×** on the shape it targets (a lowering self-tail caller, one spliceable leaf beside one
residual call: 562 → 237 ms / 2M), **every published benchmark row flat**. The documented blocker
(checkpoint-area layout) was the easy half — the real one was the inlined body's own **bytecode ip
space**, fixed with a resume arm (`ir::LeafInline::resume`).

**`mandelbrot` was the wrong motivating example and the correction is worth more than the row:
`row-sum` never lowers to native at all**, with the flag on or off, so no splice can help it —
leaf inlining is JIT-only and the VM always runs the small body. A derivation firing in
`BROOD_INLINE_DBG` proves only that it was derived; a bailed arm never reaches `[jit-ir]`.
**Check `BROOD_JIT_DUMP_IR=1 … | grep '^\[jit-ir\] ====='` for the arm's name before assuming any
inlining change can move it.**

**And "get `row-sum` onto the native path" is NOT the mandelbrot lever — measured 2026-08-04,
don't re-attempt it.** `row-sum` is refused by the **call-mediated profitability gate** in
`jit_lower_arm` (`dbg_name` set · ≥1 non-tail call · no vector op · a `Float` in the slot
profile), which exists because tiering that shape regressed `nbody` 15–20%. Partial splicing
looked like a reason to revisit it — the gate assumes every op pays a box/unbox around a call,
and splicing removes some of those calls — so the exemption was tried and measured on one binary:

| row | gate on (default) | arm forced native | floor | delta |
|---|---|---|---|---|
| `mandelbrot` | 191.7 ms | 193.0 ms | 0.3% | **+0.7%** |
| `matmul` | 154.1 ms | 161.9 ms | 0.3% | **+5.1%** |

`row-sum` does lower with the exemption (twice — small native plus the partially-spliced
upgrade), so the mechanism works; it simply **is not faster**. The gate's premise survives
partial splicing intact. Making `mandelbrot` move needs the *boxing* removed — unboxed floats
across call boundaries — which is a different and much larger piece of work, not an inlining
tweak.

Three more things a later reader should know, because two of them cost real time:

- **A probe used to poison the callee's cache.** Resolving a callee during a leaf probe
  compiles it under the reentrancy guard that suppresses the nested probe, and that arm was
  then *cached and published* — permanently denying the callee its own derivation. The
  feature did nothing for mid-level functions until `probe_arm_for` (caches nothing) fixed it.
  Any future probe that resolves through the heap has this hazard.
- **`jit_tier` itself flips `inline_installed`**, so nothing after it may re-derive "which
  engine ran this frame" from that flag. Decide from the size the frame was *built* to.
- **Read the deopt journal once.** Reading it to size the frame and again to resume gave an
  out-of-bounds `root_at` (the second read came from an already-truncated frame).

**2. ~~Per-process memory floor~~ — both named experiments are now DONE, both negative.** The
idle floor is **~4.27 KB** (`scripts/fuzz/stress/process_floor.blsp`, slope at N = 10k/40k/80k:
4731 / 4343 / 4271 — read the slope, never `rss/n`). It is **not** allocator retention
(`MIMALLOC_PURGE_DELAY=0` moves it ~2%). Attribution: `Process` 1304 B (of which **`Heap`
1200 B, inline**), `Mailbox` 184 B, `Suspended` 136 B = 1624 B structural; the remaining ~2.6 KB
is not process *data* (`process-info :memory` for an idle process is **64 bytes**) but
per-allocation overhead across the ~25–30 distinct blocks a process owns.

**(a) Boxing the `Heap` inside `Process`: tried, measured, reverted (2026-08-04).** It does what
it says — `Process` 1304 → **112 bytes** — and the floor did fall, **4273 → 4124 B/process
(−3.5%**, −11.4 MB at 80k). But it adds a **second allocation per spawn**, and that costs more
than it saves:

| | value | base-vs-base floor |
|---|---|---|
| idle floor | 4273 → 4124 B/proc (**−3.5%**) | ±0.1% |
| `spawn` | **+3.2%** | 1.0% |
| `spawn-live` | **+6.4%** | 1.3% |

`spawn-live` is already the worst published row (2.8× the BEAM), so trading 6% there for 3.5% of
idle memory is the wrong direction. Reverted; the reasoning is recorded at the `heap` field so it
is not re-attempted blind.

**(b) The allocator size-class histogram: retired WITHOUT building it, and (a) is why.** `Process`
shrank by **1192 bytes of struct** and the floor fell only **149**. So size-class rounding of the
`Process` block is *not* the dominant term, and a histogram would not locate the missing ~2.6 KB
either — it is spread thin across ~25–30 blocks with no single one dominating.

**The remaining direction, if anyone wants this row: cut the NUMBER of allocations per process,
not their sizes.** That is what (a) got backwards — it added one. Everything measured so far says
per-allocation overhead × block count is the floor, so consolidating several of a process's blocks
into one arena would attack it; shuffling bytes between existing blocks will not. Nobody has
scoped that, and note it pulls against `spawn` throughput in exactly the way (a) did, so it needs
the `spawn`/`spawn-live` pair measured alongside the floor from the start.

**3. `std/` sweep — one confirmed quadratic found, and it is NOT fixed.**

**Found: a SEQUENCE of `std/tool/sexp` structural motions over one buffer is O(n²).** Three
rising points, which is the signature `scale_sweep.blsp`'s header demands: ASCII **5.68× then
9.40×**; with a single non-ASCII character **8.84× then 12.30×**. At 6400 forms (~530 KB) that is
**9.9 s** for the walk, i.e. ~1.55 ms per motion — and per-motion cost grows with buffer size, so
a 60k-line file is ~15 ms *per keypress*. Row added: `sexp motions` in `scale_sweep.blsp`.

Root cause is by design and documented at the primitive: each motion calls `sexp/narrow`, which
calls the native `scan-form-start` **twice**, and that is a forward lexical pass from offset 0
because a backward scan cannot know whether a bracket sits inside a string without the lexer
state a forward pass carries. So per motion is O(point). `narrow`'s own docstring claims motions
cost "~three forms, not the whole buffer" — that is true of the CST work it wraps and false of
finding the window, which is where the whole cost is.

**Fixed 2.0× of constant factor; the asymptote is untouched, which is the honest headline.**
Two independent costs, both real:

1. `scan_form_start` took `expect_string` (an owned `String` — a copy of the entire text per call,
   the seam the handoff said to convert "when a workload shows the cost"; this was that workload)
   and then `chars().collect()`'d the *whole* text including everything past `pos`. Now
   `expect_string_ref` + `take(pos + 1)`, semantically identical (the outer loop stops at
   `i > pos`, and the only thing reading past `pos` is the string-skip inner loop, which cannot
   touch `best`). **−18% ASCII.**
2. `narrow` made the O(point) pass **twice** — the second call re-walking the prefix the first had
   already covered — to get the enclosing form start *and* the one before it. New native
   `scan-form-start-2` returns both from one pass. **−39% more.**

Together: 6400 forms ASCII **12061 → 6037 ms (2.0×)**. Guarded by an equivalence test over every
position of eight texts including a bracket inside a comment, a string spanning a newline, and an
unterminated string (`tests/sexp_test.blsp`). The now-dead `sexp--defun-start` wrapper was removed.

**The multi-byte gap is now the striking part**: 27571 vs 6037 ms at 6400 forms, **4.6×**. That is
`substring` being O(index) rather than O(result) off the ASCII fast path — a documented kernel
property — so it is the next lever for any buffer containing a non-ASCII character, and probably
cheaper to attack than the asymptote.

**The asymptote needs a design decision, so it was left.** Two options, neither scoped: cache
lexer state keyed by buffer version (fast, but the pure `(text point) -> point` API deliberately
has nowhere to put it), or bound the restart — `highlight/safe-restart` already exists and may
supply the notion. Whoever takes it should measure the *sequence*, not one motion; one motion
looks fine at any size.

**Cleared by measurement, so don't re-grep these:**

- **`editor/highlight` `highlight-spans` is linear** — 800→12800 lines is 16× input for 14.6×
  time, and the multi-byte ratio *falls* (4.49 → 3.42). The `char-at`-over-whole-text shape is
  present and is *not* quadratic here, so that shape alone is not evidence.
- **Every remaining `(append acc …)` fold in `std/tool/*` / `editor/*` is quadratic in a SMALL
  N** — folds over source *directories* (`project`, `docs`, `complete`, `coverage`), the tokens of
  one whitespace fragment (`complete--keywords`), or files in a one-shot report
  (`coverage`). `test--all-tests` (the O(units×tests) flatten) runs only for explicit `FILE:LINE`
  selectors. Shape present, workload absent — the handoff's own "convert only when a workload
  shows the cost" applies.
- **`std/net/http` is already hardened** — its own comments record the old `(str acc …)` being
  fixed, it searches with `bytes-index-of` on bytes rather than char indices, and header parsing
  is per-line.

**Fixed: `editor/markdown` was quadratic on multi-byte text only.** `markdown-spans` walked lines
with `(substring src i e)` at a rising `i`, plus a `string-span-until` newline scan — both O(1)
only on pure ASCII, O(i) off it. So the document scan was linear on ASCII (3.81×/3.88×) and
**superlinear on any text containing one non-ASCII character (5.03× then 6.92×, rising)**. Now one
`string-split` and a running offset — the same fix `stream-lines` needed, for the same reason.
**6400 lines: 1287 → 559 ms (2.3×), and multi-byte is now identical to ASCII (559 vs 556)**, i.e.
the encoding penalty is gone rather than reduced. Equivalence checked against the old walker over
13 sources (every newline edge case, fences, multi-byte) before the swap; the durable guard is 5
offset/edge tests in `tests/markdown_test.blsp`, since the risk the rewrite carries is the offset
arithmetic and `string-split`'s trailing empty. Dead `md--lex` / `md--line-end` removed.

**Still unswept:** `editor/lineedit` (39 `char-at`, bounded by one input line), `std/net/sse` +
`reconnect`, and the rest of `std/tool/*` beyond the modules above. ~115 `expect_string` call sites still copy their argument; two more sit in
`syntax_scan.rs` itself (`scan-tokens`, `span-runs`) and were left because they allocate on the
heap after reading, so the borrow needs restructuring rather than a one-line swap.

**4. `spawn-live`** — the worst published row, untouched. Its own noise floor is **20.6%**, so
nothing smaller is resolvable there; it has produced phantom results repeatedly.

**5. ~~`live_migration`'s deep-receive liveness flake~~ — FIXED 2026-08-04.** It failed its
*liveness* assert in 3/12 full-suite runs (8/60 with 20 concurrent copies), never on correctness:
under an oversubscribed machine a burst can finish without any process being stolen, so the
40-burst budget was too small. Raised to 400 — **0/60 after**, and free on a normal run (0.025 s,
it exits on the first burst that migrates).

Fixed by budget, **not** by adding `retries = 1` to `.config/nextest.toml`, and that choice is
the point worth keeping: this test's *other* assertion catches intermittent continuation
corruption — it is what caught the ADR-210 deopt-resume bug — and a retry would let exactly that
class pass on attempt two and be dismissed as FLAKY. Before reaching for a retry on any test, ask
what else that test guards.

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
- **`process_floor.blsp`** — the per-process idle memory floor; ~4.27 KB, flat across N. Prints
  `base_kb` so you can see the one artifact that corrupts it: **discard the first run after a
  fresh build** (cold boot cache reports a ~44 MB base instead of ~24 MB and reads ~150 B/proc
  low). Read the slope across N, never `rss/n`.
- **`leaf_splice.blsp`** — partial leaf splicing's benchmark (ADR-210); ~220 ms against ~520 ms
  with `BROOD_NO_PARTIAL_LEAF=1`, i.e. ~2.4×. Its header carries the trap: a derivation firing
  is not the same as the arm lowering, and the two must be checked separately.
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
- **Establish the noise floor before believing a delta.** Base-vs-base first — but note the
  floor you measure *inside* one invocation does not bound the drift *across* invocations.
  `nqueens` read −5.0% against a 0.2% base-vs-base floor while the same new binary measured
  104.6 and 107.6 ms in two best-of-15 runs (~3% apart).
- **For a mechanism with an off-switch, the switch IS the attribution — a two-binary delta is
  only a hint.** One binary, one invocation, `MECHANISM=off` vs on: that disposed of the
  `nqueens` −5% in seconds (the switch is worth 0.3% there, so the mechanism cannot be worth 5%).
  Reach for this *before* building a fixed-baseline harness; it is cheaper and it answers the
  question the harness only approximates.
- **Hand-measuring against a `make ab` baseline requires `target/release-fast/brood`, not
  `target/release/brood`.** `make ab` builds both sides with `make release-brood` (profile
  `release-fast`); comparing its baseline against a `cargo build --release` binary compares two
  profiles and produces confident nonsense (it gave me `nqueens` −4.3% and `startup` −5.6%, both
  fictional). This is footgun #1 in `ab-bench.sh`'s own header — read it before hand-rolling.
- **A short row needs a mean, not a best-of.** `startup` is ~17 ms and `make ab` reports whole
  milliseconds, so it reads ±6% from quantisation alone; a 40-run mean gives +0.4% against a
  0.4% floor.
- **`RSS is not a proxy for live bytes`** here — but check before blaming the allocator: for the
  per-process floor, `MIMALLOC_PURGE_DELAY=0` moved the slope by 2%.

**Testing**

- **Build the concurrent reproducer before arguing about a flake rate — and compare failure
  *modes*, not counts.** `live_migration deep_receive_continuations_resume_correctly_across_workers`
  failed inside a full `cargo nextest` run and never in isolation (0/65), which reads exactly
  like a flake. Two different failures were being conflated: HEAD fails it **3/12** full runs
  on its *liveness* assert ("no live migration observed"), while the change under test failed it
  with an **out-of-bounds `root_at`** — a real GC bug. Running 16 copies of that one test
  concurrently separated them in seconds: **8/16 against 0/16 at HEAD**, where the full suite
  gave a 1-in-8 murmur that six baseline runs had failed to contradict. A liveness assert and a
  corrupted read are not the same bug wearing different hats.
- **`live_migration` is not in `.config/nextest.toml`'s retry list** (unlike `distribution` /
  `serve_attach` / `observe_attach` / `suite`), though its liveness assertion is the same
  "blown deadline under a loaded runner" class those retries exist for. It flakes ~25% under
  full-suite load **at HEAD**. Don't attribute it to your change without a baseline run.
- **A derivation firing is not an optimisation landing.** `BROOD_INLINE_DBG` said `row-sum` got a
  partially-spliced derivation; `BROOD_JIT_DUMP_IR` said it never lowered, in either
  configuration — so the splice could not possibly have helped, because leaf inlining is JIT-only
  and the VM always runs the small body. A bailed arm never reaches the `[jit-ir]` dump, so
  *absence* there is the signal. Confirm the arm is in the dump before attributing anything to an
  inlining change; the 2026-08-02 write-up named `mandelbrot` as the payoff row on exactly this
  mistake.
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
