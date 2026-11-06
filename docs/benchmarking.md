# Benchmarking & profiling the VM

Two different questions, two different tools. Conflating them wastes afternoons
(it did — see the devlog 2026-06-07 entry).

**Everything here runs on the test machine or in CI, never on the development desktop**
(`CLAUDE.md`, "Where tests run"). "This machine" and "this box" below mean the box the
measurement was taken on.

| Question | Tool | Build |
|---|---|---|
| **Is it faster?** (timing) | the VM ÷ tree-walker **ratio** | normal/release, *no* counters |
| **Where does the time go?** (attribution) | the **`perf-stats` counters** | `--features perf-stats` |

The timing tool must carry no counter overhead; the attribution tool perturbs
timing (atomics on the hot path), so it reports *counts*, never times. Keep them
separate.

## 1. Timing — trust ratios, not absolutes

On a loaded or low-powered machine, **absolute** benchmark times drift ±10–20%
between separate process runs. So comparing two builds (a git worktree A/B, or
two `make quickbench` invocations) is measuring background load, not your change. This
is not hypothetical: it cost an afternoon — a VM change first looked like a
flat-to-regression across separately-loaded runs, then a same-but-noisy bench read
as a big win, before a load-robust measurement settled it.

The load-robust signal is the **VM ÷ tree-walker ratio measured within one `divan`
process**. Both engines live in every binary; the `eval` benches pin each row to an
engine (`set_forced_ceiling`, the `engine_grid!` macro), so size `N` is measured as a
`(Vm, N)` row and a `(Tw, N)` row in the same run. Under load both slow down
*together*, so their ratio holds where the absolutes wander. The tree-walker is a
stable in-process reference — you don't even need a baseline binary. (Divan sorts
rows by label when printing, so `(Vm, N)` and `(Tw, N)` are not necessarily printed
next to each other; `scripts/bench-ratio.awk` pairs them by `(bench, size)`. What
matters is one process, not one screen line.)

The grid is built from `Tier::ALL` and the labels come from `Tier::short()`
(`eval/compile.rs`), so a new tier gets rows in every eval bench and a column in
`bench-ratio.awk` without either being edited. `Tw` stays the reference — it is the stable
baseline the method rests on, not a tier under test.

Since ADR-222 there are **three** rows per size, not two: `Jit` (ceiling 2), `Vm` (ceiling 1 —
the VM with native tiering off) and `Tw` (ceiling 0). The middle one is the addition, and it is
the useful one for JIT work: `Jit`÷`Vm` per row is exactly the JIT-vs-no-JIT ratio the frontier
docs quote (`fib` 54×, `collatz` 40×, and `nbody`'s 3.2× is what exposed a silently bailing arm),
which previously had to be produced by hand with `BROOD_TIER=1`.

```bash
scripts/bench-ratio.sh                 # the whole eval grid, VM/TW per workload
scripts/bench-ratio.sh defseq_map      # one bench
scripts/bench-ratio.sh fib -- --sample-count 20
```

Output (a ratio < 1.00 means the VM beats the tree-walker):

```
bench                          size   tree-walker            VM    VM/TW
------------------------------------------------------------------------
defseq_map                     3000      12.24 ms      5.179 ms     0.42  58% faster
defseq_map                    30000      136.1 ms      54.12 ms     0.40  60% faster
```

**Track the ratio across changes, not the absolute ms.** A ratio also compares
meaningfully across machines; an absolute ms does not.

**But first confirm the bench exercises the path you mean.** A workload that
*defers* (a top-level `(fn …)`/`letrec` literal — LOCAL region, never VM-compiled;
see §2's `tw_defer`) runs the same code on both engines, so it reads as parity —
and noise around parity can masquerade as a ±30–50% effect. Build `--features
perf-stats` and check `(vm-stats)`: a real VM workload shows non-zero `vm_apply`
(and `self_tail` for `defseq`). This is exactly how the bogus "−30…−54%" reading
that `defseq_map` replaced got caught — its `letrec_loop` predecessor deferred.

- Add a workload to the grid: a `#[divan::bench(args = engine_grid![...])]` fn in
  `crates/lisp/benches/eval.rs` (copy `letrec_loop`). That's what makes it
  measurable both ways in one process.
- `make quickbench` (`scripts/bench.sh --quick`, 3 samples) stays for a *throwaway* directional read of
  a single configuration; `scripts/bench.sh` archives a full headline run to
  `docs/benchmarks/<timestamp>.md` — save that for a **quiet** machine.

### When the tree-walker isn't a valid reference

The ratio trick assumes your change doesn't move the tree-walker. That holds for
VM-only work (the common case). If a change touches shared machinery (the reader,
a builtin, the GC), the TW row moves too and the ratio hides it — then you need a
genuine before/after, which on a loaded box means **interleaving**: run baseline
and candidate binaries in alternation (not back-to-back blocks) so they share the
load window, many samples each.

### Measuring a whole row by hand — the ways it silently lies

`scripts/ab-bench.sh` encodes these; a hand-rolled loop re-learns them the hard way, so
they are written down here rather than only in that script's comments. All four were hit in
one session (2026-08-13, the KI-40 hunt), and each produced a *plausible* table rather than
an obviously broken one — which is what makes them expensive.

1. **Never put `timeout(1)` inside the timed region.** GNU `timeout` rounds a run up to a
   ~100 ms grid: a 78 ms row reads 104 ms, a 103 ms row reads 204 ms. Every sub-second row
   then reads as a multiple of 100 ms and every delta reads `+0.0%` or a clean integer
   ratio. Put the hang guard on an *untimed* warmup run instead. (The benchmark harness uses
   Python's `subprocess(timeout=)`, which does not do this, so published numbers are safe.)
2. **Subtract boot, or a fast row is all boot.** A warm `brood` boot is ~16 ms against a
   ~1.2 s cold one (`BROOD_BOOT_TRACE=1` to see the split, KI-38 for why it swings). Measure
   an empty program and subtract it, and discard one run per binary first so the
   build-id-keyed prelude cache is warm — the cache key includes the executable's mtime, so
   **every rebuild colds it**.
3. **Check you are running the binary you think you are.** `make release-brood` writes to
   `target/release-fast/`, `cargo build --release` to `target/release/`, and `make install`
   to `$PREFIX/bin` — three paths, and a stale one fails silently by agreeing with the
   baseline. `brood --version` prints the build sha for exactly this reason; compare it
   against `git rev-parse --short HEAD`. An installed binary predating a flag simply
   *ignores* that flag: a `BROOD_TIER` sweep against a pre-ADR-222 `brood` reports `1.0x`
   on every row, which looks like a finding rather than a mistake.
4. **A concurrency row must not be pinned, and a VM-path change must not be measured at the
   default ceiling.** `taskset`-ing a scheduler row to one CPU removes the thing it
   measures. And at the default tier a hot arm lowers to native, so the interpreter's call
   path never executes: KI-40 — a 3.19x regression on the VM's call path — read **+1.3%** at
   the default ceiling and only appeared under `--tier 1`. If a change touches
   `exec_chunk`/`dispatch`/`vm_run_bc`, A/B it at `--tier 1` as well as the default, or the
   result is a measurement of the JIT bypassing your change.

```bash
scripts/ab-bench.sh --tier 1 --floor pfib      # the VM's call path, with a noise floor
scripts/ab-bench.sh --floor fib loop collatz   # the default ceiling, single-threaded rows
```

5. **Never A/B against a binary copied out of the tree.** A copy in a scratch directory
   booted without its stdlib image and read every row 30-80% "slower" than the working tree
   (2026-10-15: `startup` 469M against 81M instructions for `(io/puts 0)`). Build the
   baseline with `make ab BASE=<ref>` (or `scripts/ab-bench.sh -b <ref> -n 1 <row>` just
   to build it) and take `target/ab/<sha>/target/release-fast/brood`: each tree writes its
   own image, and the script refuses a pair whose image states differ.
6. **Some rows are bimodal in instructions — compare mode against mode.** `json` lands in
   one of two counts ~4% apart from run to run (background compiles racing the run decide
   which), so a best-of-3 or a median of 5 can compare one binary's low mode with the other's
   high one and invent a ±6% delta. Take 20+ runs a side and compare each mode; that is what
   showed a real +2.5% (both modes moved) on 2026-10-16 and a fake +7.9% the day before.
7. **Instruction samples skid onto the instruction AFTER an atomic.** A plain
   `perf record -e instructions:u` put 34% of a function on the `test` after an `xchg` and
   130 "instructions" on an `Arc` refcount line. Use `instructions:upp` (precise, PEBS) for
   line-level attribution, and scale percentages by an exact `perf stat` count — the 1 kHz
   sample cap here also makes absolute sample totals meaningless.
8. **A module move shifts `release-fast` inlining — confirm in `release-lean`.** `release-fast`
   keeps the default 16 codegen units without fat LTO, so moving functions into a new file
   re-partitions them and changes what inlines across units: retiring `exec_value` (its prim
   helpers moved to `prim_exec.rs`) read `supervisor` +1.4% in `release-fast` and 4 731M against
   4 734M in `release-lean` (fat LTO, one unit — what `make install` ships), 2026-10-16. A
   small delta on a change that moves code is a question for `cargo build --profile
   release-lean -p cli`, not a verdict.
9. **RSS moves in 2 MiB steps — compare it with transparent huge pages off.** mimalloc
   marks its arena for THP (`allow_thp`, default on) and a `madvise`-mode kernel backs it
   with 2 MiB pages, so resident memory jumps by huge pages depending on where allocations
   land and on how fragmented the machine is at that moment. A brood-benchmarks refresh read
   base RSS 40.8 → 52.1 MiB "+27%" for a change that was +1% (19.9 → 20.1 MiB) with
   `MIMALLOC_ALLOW_THP=0` (2026-10-12). Measure memory with that set; leave it unset for
   timing, which is what users run.

### Tail latency — `make latency-gate`

Throughput rows cannot see a process that stops being preemptible: they finish in the same
time whether one quantum ran 40 µs or 4 ms. `stress/latency_probe.blsp` is an open-loop
request stream (20k req/s, every 20th request holding its worker ~500 µs) that reports the
start-latency percentiles and how many quanta held a worker over 1 ms (`:long-schedule`,
ADR-404). `make latency-gate` runs it five times and fails on more than 200 such quanta in a
run or a worst p99 over 250 µs. After ADR-400..405 it reads 2–26 long quanta a run and p99
74–95 µs (v0.37.0: p99 85–115 µs; it predates `:long-schedule`, but `BROOD_STALL_MS=1` saw a
quantum over 1 ms for nearly every fat request). It is a benchmark-box gate, not a CI one: p99.9 and max are reported, not
gated, because on an open-loop row they move 3× between runs here.

## 2. Attribution — where the VM spends work

Build with the `perf-stats` cargo feature to arm process-global work counters
(`crates/lisp/src/diagnostics/perf.rs`). **Off by default** — every counter compiles to
nothing, so normal builds and the timing benches pay zero cost.

```bash
cargo build -p cli --features perf-stats
BROOD_PERF_STATS=1 ./target/debug/brood program.blsp   # dumps counts to stderr
# …or from Brood: (vm-stats) returns the snapshot as a map.
```

Counters (cumulative, across every green process):

| counter | meaning |
|---|---|
| `vm_apply` | closure activations on the VM |
| `tail_call` / `self_tail` | tail-trampoline iterations / direct letrec self-tail-calls |
| `tw_defer` | calls that fell back to the tree-walker (the deopt surface) |
| `call_ic_hit` / `call_ic_miss` | call-site inline cache |
| `global_ic_hit` / `global_ic_miss` | global-read inline cache |
| `prim2_inline` / `prim2_fallback` | inlined 2-ary prim vs native fallback |
| `prim1_inline` / `prim1_fallback` | inlined `first`/`rest` vs fallback |
| `env_get` / `env_hops` | name resolutions / total env-chain frames walked |
| `alloc` | LOCAL heap allocations |

Reading them:

- **dispatch-bound** → high `vm_apply`/`tail_call` with a poor `call_ic` hit rate,
  or lots of `tw_defer`. This is what a bytecode VM / template JIT removes.
- **env-bound** → `env_hops` ≫ `env_get` (deep chains) — a lexical-addressing or
  capture-flattening target.
- **alloc-bound** → `alloc` (and `gc-stats` `:collections`) dominate — the engine
  barely matters; the win is allocation/GC or algorithmic (e.g. transducers).

## 3. Why this exists: the bytecode-lowering / JIT gate

ADR-096 gates bytecode lowering (and any codegen) on *a profile showing
interpretive dispatch — not allocation, GC, or `env_get` — is the bottleneck*.
`perf-stats` is how we get that profile. Concretely: if a representative workload
shows `alloc`/collections dominating and IC hit-rates already high, lowering buys
little; if `vm_apply`/`tail_call` dominate with the time going to node dispatch,
that's the green light. Measure before lowering — don't lower on faith.

(Worked datum: `(count (map inc (range n)))` — the `defseq` family on the VM via
the self-call optimization — is ~58–60% faster than the tree-walker once it
*compiles* (`map`'s body is a prelude `defn`, so RUNTIME-region). The same ops
called with a *top-level lambda* mapper, `(fn (x) …)`, read as parity instead —
because that lambda is LOCAL-region and defers, so its per-element call runs on the
tree-walker. Same `defseq`, opposite verdicts, decided by whether the closures
involved are promoted (RUNTIME) or top-level (LOCAL) — exactly the distinction
`(vm-stats)`'s `tw_defer`/`vm_apply`/`self_tail` make legible. Always confirm which
you're measuring before quoting a ratio.)

## The one rule: measure at two sizes and keep only what scales

Boot is **~13 ms** warm on this tree (the `startup` row, 2026-09-22 — it read ~47 ms when this
section was written, before the prelude image), and a COLD boot is macro-expansion-heavy. Anything shorter than about a
second is *substantially a profile of boot* — and boot does not look like boot. It looks like a
plausible, specific, wrong answer:

- **`perf` on `bintree` at its benchmark size (n=200, 160 ms)** puts `Heap::env_get` top at 14.4%
  with `eval_tail_loop` at 4.25%, which reads as "this JIT'd row is 18% interpreted". At n=2000
  **both symbols disappear**. The tree-walked form count is identical at n=3 and n=6 (13,253
  either way) — fixed startup work, zero per-node.
- **The counters at whole-process scope** on `pipeline` report `alloc` and both call-IC counters
  *identical* between a 1× and a 10× run, while `env-get` scales 8×. Every interesting counter was
  boot's.
- **`hof-decline-queued`** at process scope reads as "the HOF fast path is declined on 96% of
  activations". Scoped and swept, it is a fixed ~33k warm-up cost.

Three different tools, three confident wrong answers, one cause. So:

1. **Scope it** — `(perf/measure thunk)` for counters; a size where boot is <5% for `perf`.
2. **Sweep it** — run at two sizes and compute the ratio per counter/symbol. Keep what tracks the
   work; everything flat is fixed cost and cannot be what you are chasing.
3. **Only then attribute.** A number that has not been swept is not evidence about the row.

This is cheap — two runs instead of one — and it is the difference between the profiles in
`compute-frontier.md` §2d and the two retractions above them.

## Making `perf` usable (two blockers, and each looks like the other's fault)

`perf` answers the one question brood's own counters cannot: **self time per symbol**. The
counters say *how much* work of each kind happened; they have no per-function attribution, which
is the wall the 2026-08-28 call-path profiling hit. Two things block a readable profile, and
fixing one without the other fails in a way that looks like a different problem:

1. **`kernel.perf_event_paranoid`.** Ubuntu ships **4**, which refuses all unprivileged perf use.
   `perf record` then reports *"Failure to open any events for recording"*, which reads like a
   broken perf install rather than a policy setting.

   ```sh
   make enable-perf-check          # report only: no root, no changes
   sudo ./scripts/enable-perf.sh   # set it (default level 1) and persist it
   ```

   The script writes `/etc/sysctl.d/99-brood-perf.conf`, applies the value, and then **verifies by
   actually running `perf record`** rather than trusting the number it just wrote — that failure
   mode is the reason it exists. Undo with
   `sudo rm /etc/sysctl.d/99-brood-perf.conf && sudo sysctl --system`.

   Level **2** is enough for self-time inside brood; **1** also resolves kernel symbols, which is
   what you want the first time a row turns out to be dominated by the allocator or a syscall
   rather than by brood. Pass the level as the first argument.

2. **`[profile.release-fast]` sets `strip = true`**, so a profile of the ordinary build shows bare
   addresses instead of `dispatch::dispatch`. No sysctl fixes this:

   ```sh
   make perf-symbols               # same flags as release-brood, but unstripped + debug=1
   perf record --call-graph dwarf -- target/release-symbols/brood bench.blsp
   perf report --no-children       # self time — what the frontier tables quote
   ```

`make perf-symbols` and `make perf-brood` build into their own profiles
(`target/release-symbols/`, `target/release-perf/`), so neither overwrites the timing binary
`target/release-fast/brood`.

Why the same flags matter: `perf-symbols` keeps `PERF_RUSTFLAGS` (`debug-assertions=off`,
`overflow-checks=off`) and `RUN_FEATURES`, so it profiles the binary `make ab` measures. A profile
of a differently-built binary answers a question nobody asked.

## 6. Measuring a change with `make ab` — the rules, and the runs that taught them

These were the `Commands` section of `CLAUDE.md` until 2026-10-09; `CLAUDE.md` now keeps only
the one-line rules and points here.

**`make ab` (`scripts/ab-bench.sh`), never a hand-rolled A/B.** It builds the baseline ref in a
throwaway worktree and the working tree through the *same* `make release-brood` target (so
profile and features cannot drift), warms each binary's build-id-keyed boot cache, pins with
`taskset`, and reports best-of-N per row against `../brood-benchmarks/bench/brood/*.blsp`. It
aborts if the two binaries are byte-identical — what a silently no-op'd build looks like.
`make ab BASE=HEAD~3`, `make ab N=11 ROWS="fib pfib"`, `./scripts/ab-bench.sh --all` for the
regression sweep, `--list` for the row names, `--json <path>` for machine-readable rows,
`make ab-clean` to drop the worktrees. It measures brood against brood; the published
cross-language numbers come from `bench/harness.py` in `brood-benchmarks`.

**`--floor` for any row that moves a few percent.** It runs the baseline twice and reports the
row's own base-vs-base spread; the verdict calls a regression only past `max(5%, 2 × floor)`.

**Gate at the tier the change runs on — `make ab-vm` (ceiling 1) for anything on the VM's call
path.** At the default ceiling a hot arm is native and `exec_chunk`/`dispatch`/`vm_run_bc` never
execute, so a cost added there reads flat: KI-40 was a 3.19× regression `make ab` reported as
+1.3%. Run `make doctor` first — a stale binary fails by agreeing with the baseline.

**A sweep's few-percent row deserves a solo re-run; a drifting row needs a fixed baseline.**
Interleaving A and B shifts thermal/cache state (`persistent-map` read +3.7% in a sweep, 82 vs
81 ms directly). Some rows drift between whole invocations: `pingpong`'s baseline wandered
209 → 230 ms across one day, so a "+5.3% solo confirmation" was measuring drift twice. Keep the
`target/ab/<sha>/…/brood` binary (or use `make ab-pin`), run `taskset`-pinned best-of-15 for
base, base again, then new, and read base-vs-base as the floor — that change measured +0.9%
against a +0.5% floor. Do not report a regression within a couple of multiples of an unmeasured
floor.

**Pinned rows charge the benchmark for background JIT compilation.** `make ab` pins compute
rows to one core, so the compiler thread competes with the benchmark. Right for generated-code
quality, wrong for a change that alters *how much* gets compiled: ADR-175's shared code tiered
more prelude arms (18 lowered vs 7), read `collatz` +8% pinned and zero unpinned. Re-run such a
row unpinned; `BROOD_JIT_DUMP_IR=1 … | grep -c '^\[jit-ir\]'` counts the compiles.

**Measure both a short and a long run.** A tiered runtime has two steady states: thousands of
calls (interpreted / not yet tiered — what a `nest check`, a script or a fresh request handler
pays) and millions (fully tiered, including the deferred inlined upgrade). Sweep the call count
across two orders of magnitude and check whether the *gap between the arms* moves. Cross-check
with `BROOD_JIT_DUMP_IR=1` (did it lower?) and `BROOD_JIT_BAIL_TRACE=1` (was it refused?); a
bailed arm never reaches the IR dump, so absence there is the signal. Relatedly, timing whole
`brood` invocations in a shell loop only ever measures the cold path (a JSON round-trip: round 1
~300 ms, warm ~224 ms) — time rounds inside one process to see both.

**Count the calls before optimising anything.** A code reading is evidence about what a
function does, never about how often it runs. `compute-frontier.md` §7.8's top-ranked item was
implemented, measured flat on every row, and reverted: a five-line probe showed the function ran
21 times on `fib` against billions of activations. A `static AtomicUsize` and an `eprintln!` at
the call site, one run, then decide.

**Attribution: `make perf-brood`, then `(perf/summary)`.** `perf/report` gives a map,
`(perf/measure thunk)` a window narrower than the process — the counters are cumulative from
start and boot is expansion-heavy (the same program read an 84% tree-walker defer rate on a cold
boot cache and 0.8% warm). `(vm-stats-reset)` is the primitive underneath. `perf/report` never
concludes `:alloc-bound` (a native arm's iterations stop being counted while its allocations do
not) and refuses to judge a rate resting on under 1000 samples.

**A published benchmark run installs the LEAN build**: `make install
INSTALL_FEATURES='$(RUN_FEATURES)'`. The harness runs whatever `brood` is on `PATH`, and
`RUN_FEATURES` is what `make ab` measures and `nest release` embeds — one build for all three.
(Lean and dev-tools startup measured identical, 10 ms / 18.8 MB: the DEV_MODULES load on demand.)
Reinstall with plain `make install` to get `nest test`/`repl` back.

## 7. Traps that cost a verdict across sessions

Carried over from the retired cross-session task list (`docs/archive/perf-handoff.md`, whose
tasks are all answered). Each of these has cost a wrong verdict in this repo.

- **`--floor` bounds error WITHIN one `ab-bench` run, not ACROSS runs.** A bisect built from
  one-baseline-per-run deltas (2026-09-11, KI-128) manufactured a smooth ramp and "converged" on a
  commit that changed four lines of markdown. To compare several commits, put them all in ONE
  session, interleaved, with the same binary measured twice as a control.
- **Every arm must be `:state :live`, and you must check.** A hand-rolled harness compared
  source-booting baselines against an imaged HEAD — ~10 ms on a 146 ms row, in exactly the
  direction that erases a HEAD slowdown — and produced a confident retraction of a true finding.
  `ab-bench` refuses this (`stdimage MISMATCH`); hand-rolling around it reintroduces the bias.
  Probe each binary with `(io/inspect (stdimage/status))`.
- **`stdimage` keeps at most four images** (`max-keep 4`), so at most four baselines + HEAD
  compare imaged in one session; a fifth silently evicts the earliest into a source boot.
- **A rebuild colds the prelude image**, and **a `std/` edit or a new commit moves the stdlib
  image id**: the first runs after either boot from source (1.5 billion instructions against
  78 M on a one-line file — it looks exactly like a catastrophic regression). Run the binary two
  or three times after a build, and assert `:state :live` in the same shell as the measurement.
  **If a revert does not reproduce the number you started from, the rig is wrong, not the code.**
- **The bench corpus only reaches as far back as its names do.** A row may only use names and
  argument orders as old as the oldest baseline anyone will measure against: ADR-302/308's
  data-first reorder made 12 of 31 rows unrunnable against any pre-2026-09-02 commit, and
  brood-benchmarks' CI (built from `main`) cannot notice.

**What a busy box CAN answer.** This is for a loaded test machine or a CI runner; the development
desktop runs none of it. Deferring to a quiet benchmark box is right for a *magnitude* claim and
wrong for a *mechanism* one. In order: turn it into a structural question first (did the arm
lower — `BROOD_JIT_DUMP_IR=1`; refused, and why — `(dev/why-not-native f argc)` in the running
program, which reads the refusal the arm recorded, or `BROOD_JIT_BAIL_TRACE=1`; lowered then fell
back — `BROOD_DEOPT_TRACE=1`, needs `perf-stats`), which is binary and load-immune; trust large
ratios only (spreads of 5–12% were measured on the dev box); use within-process adjacency
(`crates/lisp/benches/eval.rs` runs both arms in one process) for anything smaller; count the
calls before optimising; and check `cat /proc/loadavg` and `pgrep -af 'while :'` before believing
any number (twelve orphaned spinners once inflated every number ~2.5x for four days).

**Instruction counts.** `perf stat -e instructions:u -x, -r 3 <cmd>` when
`/proc/sys/kernel/perf_event_paranoid` allows it — read the sysctl each time, it has moved
between 1 and 4 with no reboot — else `valgrind --tool=callgrind --callgrind-out-file=/dev/null
<cmd>`, which counts deterministically at ~50x wall time. Quote a number with the instrument that
took it.

**Reporting.** Results go in `docs/devlog.md` with the date, the exact `make ab` invocation, N,
whether rows were pinned, and the floor for every row quoted. A delta without its floor is not a
result.
