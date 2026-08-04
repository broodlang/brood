# Handoff — what to do next, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); decisions in [`decisions.md`](decisions.md); the
option book in [`runtime-frontier.md`](runtime-frontier.md); bugs in
[`known-issues.md`](known-issues.md). Read this to pick the work back up cold.

**As of 2026-08-04**, brood `46f8790a`. Nothing half-finished. Rust suite **946/946** (nextest),
in-language **4367/4367**, `nest check` clean, `nest format --check` clean, rustfmt clean, both
default and `--no-default-features` builds warning-clean. No open correctness bugs.

---

## 1. START HERE — the next lever, and the decision it needs

**Char→byte conversion for NON-ASCII strings is O(index), and it is now the biggest measured
lever in `std/`.** Brood indexes strings by Unicode scalar; they are stored as UTF-8. A char index
*is* a byte offset only for pure-ASCII text, so every conversion is O(1) on that fast path and a
walk-from-the-start off it. Two sweep rows are quadratic **only** in the multi-byte regime:

| row (`scripts/fuzz/stress/scale_sweep.blsp`) | ASCII | `UTF8=1` |
|---|---|---|
| `string inc-scan` (`index-of` with a rising `from`) | 0/2 ms, unmeasurable | **16.85×** (7 → 118 ms, N=800) |
| `sexp motions` | 5.48× | **9.80×** |

Reproduce with `UTF8=1 N=800 brood scripts/fuzz/stress/scale_sweep.blsp`, and compare against the
same command without `UTF8=1`. Those two rows are the scoreboard: the fix lands when `inc-scan`
goes linear and `sexp motions` drops to its ASCII ratio.

**Read this before starting — the 2026-08-02 char-count cache did NOT fix this, and the reason is
structural.** That change cached each string slot's char count so `string-length` is O(1) and
`chars == as_str().len()` is an O(1) pure-ASCII test. It was written up as making `inc-scan`
"LINEAR too", which is true and only on ASCII: **the mechanism of that fix *is* the fast-path
test**, so it cannot reach the slow path. Don't re-apply the same idea expecting a different result.

**The shape of the real fix** is a sparse char→byte index on the string slot — `marks[k]` = the byte
offset of char `k * STRIDE` — making conversion an O(1) lookup plus a walk bounded by STRIDE.
`LocalString` in `crates/lisp/src/core/heap.rs` (~line 63) already caches `chars` and is the natural
home; its doc comment lays out the existing reasoning and names "non-ASCII still walks" as the
accepted cost.

**The decision that blocks it, and why you cannot just write the obvious version.** A string slot
can hold `StrData::Shared(Arc<SharedBlob>)` — PRELUDE/RUNTIME strings, reachable from **multiple
processes concurrently**. So a lazily-populated index (`OnceCell`/`RefCell` on the slot, built on
first conversion) is a data race on exactly the long shared strings most worth indexing. Pick one:

- **eager at construction** — simple and race-free, but every string pays the build and the memory,
  including the ASCII ones that need no index at all;
- **lazy + synchronised** — an atomic/lock per slot, paid on the read path;
- **LOCAL slots only** — race-free by construction, and covers the editor/buffer case (per-process
  text) while leaving shared strings on the slow path.

I would try the third first: smallest change, sound without new synchronisation, and the measured
rows are LOCAL text. **Verify that assumption before building** — check which region the sweep's
fixtures actually land in.

**Gate it as a kernel change**: `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` on the suite, the
`--no-default-features` build, and the differential fuzzer — a wrong byte offset is a silent
wrong-substring, not a crash.

## 2. Then: finish the `std/` sweep

Unswept, in the order I would take them. **Measure both regimes** (`UTF8=1`) — a pure-ASCII corpus
hides this entire class, which is how `markdown` stayed quadratic through an earlier pass.

- `editor/lineedit` — 39 `char-at`, each bounded by one input line, so low risk; but it runs per
  keystroke, and a long pasted line is the case to check.
- `std/net/sse`, `std/net/reconnect` — stream accumulation, the shape `stream-lines` had.
- The rest of `std/tool/*` beyond `project`/`test`/`complete`/`coverage`/`repl`/`sexp`.

The method that keeps working: grep for the shape (an `append`/`concat`/`str` whose argument is the
**accumulator**; a `substring`/`char-at` with a loop-carried index), then **filter by reachability
before measuring** — most hits are quadratic in a genuinely small N (source dirs, tokens of one
fragment, files in a one-shot report) and are not worth touching. Confirm a real one on **three
rising points**, not two.

## 3. Then: `spawn-live`

The worst published row — 2.8× slower and 1.9× heavier than the BEAM — and untouched. Its own noise
floor is **20.6%**, so nothing smaller is resolvable on it; it has produced phantom results
repeatedly. Related and measured: boxing the `Heap` costs it **+6.4%** (§4).

## 4. Closed — do NOT re-attempt these

Each was measured to a conclusion. Re-deriving them costs a session each.

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
**`markdown-spans` multi-byte 1287→559 ms (2.3×, encoding penalty gone)**.

`sexp motions` is still a confirmed quadratic in *shape* — each motion's `narrow` needs a form-start
scan that is O(point) by design (a backward scan cannot know whether a bracket sits inside a
string). Two constant-factor fixes landed; the asymptote needs resumable lexer state, and
`highlight/safe-restart` is the same O(pos) native rather than an existing bound, so there is
nothing to reuse.

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
  that only exercises the fast path will report that it did.** The char-count cache is exactly that
  shape; so is anything gated on `is_ascii`. Sweep **both** encoding regimes.
- **A ratio near 4× that RISES across bases is not linear.** `format-source` read 3.80/4.12/4.64 and
  was cleared as linear; pushing the base gave 4.46 then 6.40. Only a *falling* ratio (warm-up)
  clears a row. Check the trend across triples, not one triple.
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
  off-switch, run the test with the switch off before committing it.
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
- **`pkill -f <pattern>` matches your own shell** — it killed my own command twice in one session.
  Use `pgrep -f "[h]arness.py"` and kill by PID.
- **Process death reports go to stdout** — `2>/dev/null` will not filter them.

**Diagnosis**

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

- **A char index is a byte offset only for pure-ASCII strings.** Everything indexing a string by
  character therefore has **two complexity regimes**: `substring` is O(result) on ASCII and O(index)
  otherwise, so a char-by-char scan is linear on ASCII and quadratic on multi-byte. This is §1.
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
  **`UTF8=1` re-runs every row in the multi-byte regime**; a row cleared without it is half-cleared.
  Its header records which rows are cleared, which were cleared *wrongly*, and why.
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
