# Handoff — what to do next, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); decisions in [`decisions.md`](decisions.md); the
option book in [`runtime-frontier.md`](runtime-frontier.md); bugs in
[`known-issues.md`](known-issues.md). Read this to pick the work back up cold.

**As of 2026-08-04**, brood with ADR-213 (char→byte index) + ADR-214 (form-start safepoints) on
top of the ADR-211/212 registry + package-signing work. Nothing half-finished. Rust
suite **954/954** (nextest), in-language **4390/4390** — also green under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` — `nest check` clean, `nest format --check` clean, rustfmt and
clippy clean, both default and `--no-default-features` builds warning-clean, metamorphic
differential clean across 4 engine configs. **No open issues** — KI-25 (five suites failing when
re-run inside one image, which blocked `--repeat-until-failure` across the suite) was fixed
2026-08-04: four suites marked `:isolated` for the `%isolate` rollback, and `pid_identity_test`
now takes the one-shot `node-start` only when `(node-name)` is `:nonode`. The whole suite
re-runs clean in one image (4390/4390 twice), so `--repeat-until-failure` is usable for flake
hunting again.

---

## 1. START HERE — `spawn-live`, and the measurement discipline it demands

**Every quadratic in `std/` is gone** (§4), so the next lever is the worst *published* row.
`spawn-live` is **2.8× slower and 1.9× heavier than the BEAM** and has never been worked on.

Its own noise floor is **20.6%**, which is the whole difficulty: nothing smaller than that is
resolvable on it, and it has produced phantom results repeatedly. Before believing anything
here, read §5's measurement traps — in particular, measure a **control row that cannot be
affected** by the change (a ~2% whole-binary drift is normal between two builds), and prefer a
mechanism switch on ONE binary over a two-binary delta.

Related and already measured: boxing the `Heap` inside `Process` shrinks the per-process floor
3.5% but costs `spawn-live` **+6.4%** (§4) — so the direction is cutting the *number* of
allocations per process, not their sizes, and the `spawn`/`spawn-live` pair must be measured
together with the floor from the start.

## 2. Then: rope-native structural motion, the editor half of the `sexp` story

ADR-214 made a *sequence* of structural motions linear when the caller holds one text value —
the tooling/LSP shape. The **editor** shape is untouched and cannot be fixed by that cache:
`sexp/forward` and friends call `(buffer-text buf)`, which is `rope->string`, so every motion
allocates a fresh O(n) string before any scanning happens, and the safepoint table can never
hit. A keystroke-driven motion is therefore O(buffer) no matter how fast the scan is.

The options, in the order I would try them:

- **Motions over the rope.** `std/tool/sexp.blsp` is written against `(text point) -> point`
  with `text` a string; `parse-source-positioned` and `scan-form-start` both take strings. A
  rope-native path needs at least a rope-shaped form-start scan (ropey exposes chunk iteration,
  so the same safepoint idea applies) and a decision about whether the CST walk slices a rope
  or a string.
- **Or: let the caller hold the string.** A command loop that keeps one `text` value across a
  run of motions gets ADR-214's cache for free. Cheap, and it only helps a *sequence* of
  motions between edits — worth checking against how the downstream editor actually drives it
  before building anything in the kernel.

Measure `(buffer-text buf)` first: if the stringify dominates a single motion at realistic
buffer sizes, that number decides which of the two above is worth the work.

## 3. Then: the `expect_string` copy seam, by body cost

`expect_string` returns an **owned** `String` at ~105 remaining call sites — one copy of the
argument per call. Eight were converted on 2026-08-04, and the measurement gives the rule:

- the copy is worth removing where the call's own work is **O(1)** and the argument is a whole
  buffer — `(grapheme-at txt 0)` on 212 KB measured **−74%**;
- it is **noise** where the body is per-char — `grapheme-count` / `string->codepoints` over the
  whole string measured −0.6%, inside the drift floor, because UAX #29 segmentation dwarfs a
  memcpy.

So triage by what the body costs. Two sites **cannot** take the borrow at all: `string-split`
and `scan-tokens` allocate per piece *while* scanning, and `string-split` would gain nothing
anyway (its copies are the parts, not the input). `scan-tokens` would need a two-pass rewrite
(collect ranges, then allocate) — worth it only if the fontifier shows up in a measurement.

## 4. Closed — do NOT re-attempt these

Each was measured to a conclusion. Re-deriving them costs a session each.

- **`sexp motions`, the last quadratic in the sweep** — **fixed** (ADR-214): the form-start scan
  resumes from a safepoint table cached against the string value, and runs over bytes instead of
  a per-call `Vec<char>`. Ratio 7.97 → 3.88, 18570 → 6146 ms at 12800 forms, linear on four
  rising bases. Do not re-attempt it as a *constant-factor* fix: two of those landed first and
  left the shape untouched. What is left is the **editor** path (§2), which this cannot reach.
- **`sse--frames`** — **fixed**: one `string-split` instead of a `substring` of the rest per
  event, 5671 → 15 ms at 25600 events, with its own sweep row.
- **`editor/lineedit`'s per-keystroke geometry** — measured and **declined**: 2.5 ns/char, so
  28 µs at 10 K chars and 2452 µs at 1 M. Under a frame even on a 1 MB pasted line.
- **Char→byte conversion for non-ASCII strings** — **fixed** (ADR-213): a sparse char→byte index on
  the string slot, so a char index costs a lookup plus a walk bounded by 32 chars in either
  encoding regime. `inc-scan` 16.85× → linear; `sexp motions` lost its 9.80× → 5.42× encoding
  penalty. Do not re-apply the *char-count cache* idea expecting more: its mechanism is the ASCII
  test itself, which is why it never reached the slow path.
- **Forcing `mandelbrot`'s `row-sum` onto the native path.** Refused by the call-mediated
  profitability gate in `jit_lower_arm`; exempting it makes `mandelbrot` **+0.7%** and `matmul`
  **+5.1%** (0.3% floors). The arm *does* lower under the exemption — it simply is not faster. The
  real `mandelbrot` lever is removing the boxing (unboxed floats across call boundaries), a much
  larger piece of work.
- **Boxing the `Heap` inside `Process`** (memory-floor experiment (a)). `Process` 1304 → 112 bytes
  and the floor fell 4273 → 4124 B/proc (−3.5%), but it adds a second allocation per spawn:
  `spawn` **+3.2%**, `spawn-live` **+6.4%**. Wrong trade; reverted.
- **The allocator size-class histogram** (experiment (b)) — retired by (a)'s shape: `Process` shrank
  1192 bytes of struct and the floor moved 149, so size-class rounding is not the dominant term. The
  floor is spread thin across ~25–30 blocks. If anyone wants that row, the direction is cutting the
  *number* of allocations per process, not their sizes — and (a) shows that pulls against `spawn`,
  so measure the `spawn`/`spawn-live` pair alongside the floor from the start.
- A memory leak (chased, does not exist); endurance (16/16 soak, 12.7 M iterations); thread 6's
  throughput decay (fixed, ADR-208); the RUNTIME reclamation threshold (dissolved); per-message cost
  as an explanation for `latency` (it was spawn placement — send+receive is 1.1 µs).

## 5. What shipped recently, and how to turn each piece off

Every mechanism has an off-switch, because each is an optimisation whose fallback is the old
behaviour. **If something misbehaves, bisect with these before bisecting commits** — and for a
mechanism with a switch, the switch on ONE binary is the attribution (§6).

| Change | Off-switch | Worth |
|---|---|---|
| Form-start safepoint table on the string value (ADR-214) | — (a cache that changes no answer; gated by equality with the pre-table scan at every position) | `sexp motions` 7.97× → 3.88×, 3.0× at 12800 forms |
| Sparse char→byte index on the string slot (ADR-213) | — (a cache that changes no result; its gate is that its answers equal the walk's) | multi-byte char indexing **96×** on a micro (60.2 s → 0.62 s); `inc-scan` 16.85× → linear; `sexp motions` 9.80× → 5.42×; ASCII flat |
| Partial leaf splicing (ADR-210) | `BROOD_NO_PARTIAL_LEAF=1` | 2.4× on a lowering caller with a leaf beside a residual call; every published row flat |
| Shared closure crosses a **serialised** send by handle (ADR-208) | `BROOD_NO_SHARE_FN_MSG=1` | `rt_closures` 143,752 → 66 constant; RSS 213 vs 502 MB |
| Idle peer told at once that a peer queued a child | `BROOD_NO_STEAL_WAKE=1` | `latency` p50 27 → 19 µs, p99 124 → 78 µs |
| Owner keeps first refusal on a fresh child | `BROOD_STEAL_GRACE_NS=<n>` (0 disables) | protects `supervisor`; the cliff is at 2.5 µs on this machine and the default takes a 2× margin deliberately |
| Closure sends share already-shared code, **parked** path (ADR-194) | `BROOD_NO_SHARE_FN=1` | retained closure 436 B → 48 B |
| Spawn placement spills off a backlogged worker | `BROOD_SPAWN_SPILL=999999` / `BROOD_SPAWN_RR=1` | `latency` p50 5×, p99 2.9× |
| Receive-mark (ADR-195) | `BROOD_NO_RECV_MARK=1` | backlogged reply O(backlog) → O(1), 653 → 4 µs at 32k |
| Fast-link deopt shape check is flag-free (KI-26) | — (correctness) | a peer's stale link no longer re-runs a journaled effect from ip 0 |
| Registry updates are atomic (KI-22/23, ADR-202) | — (correctness) | ~40% of concurrent registrations were being lost |

**`std/` quadratics fixed** (each with a `scale_sweep.blsp` row so it cannot come back):
`template/render` 318→24 ms · `last-index-of` 540→1 ms · `strip-ansi` 1583→109 ms ·
`stream-lines` 303→39 ms · `format-source` 3593→1988 ms · **`sexp` motions 12061→6037 ms (2.0×)** ·
**`markdown-spans` multi-byte 1287→559 ms (2.3×)** · **`inc-scan` multi-byte 118→3 ms (ADR-213)** ·
**`sexp motions` 18570→6146 ms, quadratic→linear (ADR-214)** · **`sse frames-1c` 5671→15 ms**.

**Every row in the sweep is now linear in both encoding regimes.** Its job from here is
regression detection, which means running it **both** ways (`UTF8=1`) and checking the ratio
*trend* across bases, not one triple.

## 6. Traps — every one of these cost real time

**Measurement**

- **For a mechanism with an off-switch, the switch IS the attribution; a two-binary delta is only a
  hint.** One binary, one invocation, `MECHANISM=off` vs on. That disposed of a `nqueens` −5% in
  seconds (the switch is worth 0.3% there, so the mechanism cannot be worth 5%). Reach for it
  *before* building a fixed-baseline harness.
- **Hand-measuring against a `make ab` baseline requires `target/release-fast/brood`, not
  `target/release/brood`.** `make ab` builds both sides with `make release-brood` (profile
  `release-fast`); comparing its baseline against a `cargo build --release` binary compares two
  profiles and yields confident nonsense — it gave me `nqueens` −4.3% and `startup` −5.6%, both
  fictional. Footgun #1 in `ab-bench.sh`'s own header.
- **An optimisation whose mechanism is a fast-path test cannot clear the slow path, and a corpus
  that only exercises the fast path will report that it did.** The char-count cache was exactly
  that shape; so is anything gated on `is_ascii`. Sweep **both** encoding regimes.
- **A ratio near 4× that RISES across bases is not linear.** `format-source` read 3.80/4.12/4.64 and
  was cleared as linear; pushing the base gave 4.46 then 6.40. Only a *falling* ratio (warm-up)
  clears a row. Check the trend across triples, not one triple.
- **Before believing a small two-binary delta, measure a row that CANNOT be affected by the
  change.** ADR-213's ASCII micro read `char-at` +2.1% and `last-index-of` +2.4% against 0.0%
  floors, and I spent two builds reshaping hot paths to chase it — the reshapes made it *worse*
  (+5.5%). `wordcount` calls none of the changed code and read **+2.2%** on the same binary pair:
  ~2% of whole-binary codegen/layout drift, and every string row was inside it. A size argument
  works too, and faster: `last-index-of`'s delta scaled with a byte scan costing ~90 µs per call,
  and no per-call change of a few instructions accounts for 2.4 µs.
- **Establish the noise floor first — and the floor measured *inside* one invocation does not bound
  the drift *across* invocations.** `nqueens` read −5.0% against a 0.2% base-vs-base floor while the
  same binary measured 104.6 and 107.6 ms in two best-of-15 runs.
- **A short row needs a mean, not a best-of.** `startup` is ~17 ms and `make ab` reports whole
  milliseconds, so it reads ±6% from quantisation alone; a 40-run mean gives +0.4%/0.4% floor.
- **Discard the first run after a fresh build.** Cold boot cache reports a ~44 MB base instead of
  ~24 MB in `process_floor.blsp` — the same size as the effect being measured.
- **Measure the slope, not the ratio.** RSS/N folds the runtime's ~24 MB base into a per-process
  figure; that is how the memory floor was once recorded as 5.9 KB when it is ~4.27 KB.
- **A harness can measure itself.** `process_floor.blsp` retaining the spawned pids put N cons cells
  of per-process cost into the very slope it measured (4470 vs 4271 B/proc).
- **Load contaminates everything.** `ring` "regressed" 12%, `supervisor` 8%, a sweep row 3.7% — all
  gone on a quiet machine. Wait for load < 0.5.
- **Never difference time-boxed runs** — RSS tracks iterations, so the comparison measures the
  iteration count.
- **A full disk looks like a toolchain crash** (`ld terminated with signal 7`). Check `df` first.
  `make ab-clean` is not automatic — ~1.1 GB per baseline worktree.
- **`RSS is not a proxy for live bytes`** here — but check before blaming the allocator:
  `MIMALLOC_PURGE_DELAY=0` moved the per-process floor by 2%.

**Testing**

- **Build the concurrent reproducer before arguing about a flake rate, and compare failure *modes*
  not counts.** `live_migration deep_receive_…` failed inside a full `nextest` run and never in
  isolation (0/65). Two different failures were conflated: HEAD fails it on a *liveness* assert,
  while the change under test failed with an **out-of-bounds `root_at`** — a real GC bug. 16
  concurrent copies of that one test separated them in seconds: **8/16 vs 0/16 at HEAD**, where the
  full suite gave a 1-in-8 murmur that six baseline runs had failed to contradict.
- **A green test proves nothing until you run it with the mechanism off.** For any mechanism with an
  off-switch, run the test with the switch off before committing it. For a mechanism with **no**
  switch (a pure cache, like ADR-213's index), the equivalent is **sabotage**: break it by one
  character and confirm every new test fails. If they don't, they are not gates.
- **Verify a detector before trusting it — but "make it fire" need not mean "reproduce it end to
  end".** KI-26's runtime detector could only fire by winning a race, and never did across the
  suite, `pfib`, and a 24-process purpose-built race. The hazard was a *predicate*, so extracting
  the predicate and table-testing it was both possible and stronger: it covers both flag states and
  every nearby frame size, which no amount of hammering would.
- **Before adding a nextest retry, ask what else that test guards.** `live_migration`'s liveness
  assert flakes under load, but its *other* assertion catches intermittent continuation corruption —
  the very bug it caught in ADR-210. A retry would have absorbed that as FLAKY. Fixed by raising the
  burst budget instead (8/60 → 0/60, free on a normal run).
- **A derivation firing is not an optimisation landing.** `BROOD_INLINE_DBG` reported a
  partially-spliced derivation for `row-sum`; `BROOD_JIT_DUMP_IR` showed it never lowered. Leaf
  inlining is JIT-only and the VM always runs the small body, so a bailed arm gets nothing. A bailed
  arm never reaches the `[jit-ir]` dump, so *absence* there is the signal.
- **`std/*.blsp` is embedded at build time.** Rebuild `brood` **and** `nest` after touching `std/`,
  or you will debug yesterday's bytes. Same class as `-p brood` vs `--bin brood`.
- **The conformance tests need `nest test`, not `--test`** — they `(:use corpus)`, which only
  resolves through the project's module path. `brood --test tests/conformance_utf8_test.blsp` fails
  with "cannot find module 'corpus'", which is a harness error, not a failure.
- **`pkill -f <pattern>` matches your own shell** — it killed my own command twice in one session.
  Use `pgrep -f "[h]arness.py"` and kill by PID.
- **Process death reports go to stdout** — `2>/dev/null` will not filter them.

**Diagnosis**

- **"Restrict the scope" is not automatically simpler than "synchronise".** I had written up
  LOCAL-slots-only as the smallest sound way to populate ADR-213's index, because the shared regions
  race; a `OnceLock` turned out to be *smaller* (no region split in the accessor) and broader. When
  the cached value is a pure function of immutable data, a race between builders is benign — and
  this kernel has immutability everywhere (ADR-026), so reach for that argument before narrowing a
  feature's reach.
- **When a fix underdelivers against a mechanism you were confident about, that gap is evidence
  about where the cost actually is.** The `sexp` allocation fix I predicted was worth −18%; being
  disappointed by it sent me back and found the real one (two O(point) passes where one suffices,
  −39% more). I would otherwise have written up a true and incomplete story.
- **Threads get named after the mechanism nearest the symptom, not the cause.** Two of four were
  misnamed. Re-derive a thread's premise before implementing against it.
- **A comment asserting a cost is not evidence** — and a docstring can describe a bound's *intent*
  rather than its achievement. `sexp/narrow` says motions cost "~three forms, not the whole buffer";
  true of the CST work it wraps, false of finding the window, which was the whole cost.
- **Read the existing argument before inventing one.** ADR-194's comment named exactly why sharing is
  sound on the parked path, which identified what the serialised path lacked.
- **A benchmark port drifts silently when language semantics change under it.** `mandelbrot` looked
  like a 3.5× regression bisected to exact rationals; identical source measures 201 vs 200 ms —
  `(/ px n)` had simply stopped being a float divide. When a numeric primitive's semantics change,
  grep the benchmark ports.

## 7. Semantics worth knowing (documented, not bugs)

- **Char indexing costs O(1) in both encoding regimes** (ADR-213). A char index *is* a byte offset
  for pure-ASCII text; off that path the string slot's sparse char→byte index makes the conversion a
  lookup plus a walk bounded by 32 chars. So `substring` is O(result), and a `char-at` loop or an
  `index-of` scan with a rising `from` is linear on any text. The code-point-vector rewrites this
  class of bug once forced (`url`, `csv`, `ansi`) are no longer required — they are left alone.
- **A cache keyed to a string VALUE goes on the slot's `StrAux` cell, not in a map keyed by
  `StrId`.** A handle is unique only within a GC epoch; the cell travels with the bytes. The heap
  owns the cell as `dyn Any` and never interprets it, so a higher layer can cache its own table
  (ADR-214's lexer safepoints) without the core depending on it.
- **`(buffer-text buf)` is `rope->string` — a fresh O(n) string per call.** Anything that calls it
  per keystroke is O(buffer) per keystroke before it does any work of its own. This is why the
  editor gets nothing from ADR-214 (§2).
- **Hot reload does not reach a self-recursive loop.** A tail self-call compiles to
  `Node::SelfCall`, which re-runs the arm without resolving the callee. Redefining any *other* global
  the loop calls does reach it. Erlang's local-vs-remote rule; see `live-editing.md`.
- **A closure that captures no locals is already shared code**; one that captures a local is copied
  on send. That is why supervisor `:start` thunks should avoid captures — ADR-194/208.
- **`/` is exact.** `(/ 3 4)` is the rational `3/4` (ADR-196); `(/ 4 2)` is `2`. Use `quot` for an
  integer count, and convert to float *before* dividing in a float pipeline.
- **`->float` is a function call, not a cast** (~85 ns).
- **Leaf inlining is JIT-only** — the VM always runs the small body, so an arm whose native bails
  gets nothing from any splice.
- **Duplicate supervisor `:id`s** resolve to the later-started child.

## 8. Tools

All in `scripts/fuzz/stress/`, each with a usage header worth reading first.

- **`scale_sweep.blsp`** — a `std/` op at N and 4N, ratio printed (linear ~4×, quadratic ~16×).
  **`UTF8=1` re-runs every row in the multi-byte regime.** Its header records which rows are
  cleared, which were cleared *wrongly*, and why. **Every row is linear today**, so a
  superlinear reading is a regression, not a known gap.
- **`leaf_splice.blsp`** — partial leaf splicing's benchmark (ADR-210); ~220 ms vs ~520 ms with
  `BROOD_NO_PARTIAL_LEAF=1`. Its header carries the derivation-vs-lowering trap.
- **`process_floor.blsp`** — the per-process idle floor; ~4.27 KB, flat across N. Read the slope,
  never `rss/n`; discard the first run after a fresh build.
- **`soak_selfcheck.blsp`** — sustained load with an invariant checked every iteration. **Always pair
  it with a control** reverting the mechanism under test.
- **`decay_isolate.blsp`** — throughput per fixed-size window plus RSS and `:runtime-closures`; run
  modes sequentially.
- **`receive_backlog.blsp`** · **`net_framed_scale.blsp`** — the receive-mark and framed-read
  benchmarks, each carrying its own controls.
- **`tests/registry_test.blsp`** / **`tests/shared_closure_msg_test.blsp`** — each carries a control
  that fails with its mechanism off, which is the only version worth having.
- **`tests/collection_identities_test.blsp`** — seeded-random laws over maps/vectors/strings,
  including the multi-byte char-index laws (each op against the code-point vector). The place to add
  a property that every engine would agree on, which the engine-differential is therefore blind to.
- `scripts/fuzz/run.sh <generator>` — differential across 4 engine configs (tree-walker, VM-no-JIT,
  VM+JIT, GC-stress+verify). `make ab BASE=<ref>` for brood-vs-brood rows; `bench/harness.py` in
  brood-benchmarks for the published cross-language numbers.

## 9. Where we stand against the field

From the published run (`brood-benchmarks/results/`, 2026-08-02):

- **`latency`** (open-loop, ranked by p99) — Elixir 58 µs, **Brood 78 µs**, Node 461, Python 485,
  .NET 783. 2nd of five, **5.9× ahead of third**. p50 19 µs against Elixir's 8.
- **`supervisor`** — Brood 876 ms vs Elixir 439 ms. Neutral to thread 6's fix by construction (it
  runs ~25,000 operations where the decay needs ~175,000 to appear). Don't go looking for it there.
- **Compute aggregate** — 2.9× the fastest, 3rd of seven, ahead of Elixir.
- **`spawn-live`** — the worst row: 2.8× slower and 1.9× heavier than the BEAM. See §3.
