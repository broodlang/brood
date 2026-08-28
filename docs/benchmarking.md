# Benchmarking & profiling the VM

Two different questions, two different tools. Conflating them wastes afternoons
(it did — see the devlog 2026-06-07 entry).

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
two `quickbench` invocations) is measuring background load, not your change. This
is not hypothetical: it cost an afternoon — a VM change first looked like a
flat-to-regression across separately-loaded runs, then a same-but-noisy bench read
as a big win, before a load-robust measurement settled it.

The load-robust signal is the **VM ÷ tree-walker ratio measured within one `divan`
process**. Both engines live in every binary; the `eval` benches pin each row to an
engine (`set_forced_engine`, the `engine_grid!` macro), so size `N` is measured as a
`(Vm, N)` row and a `(Tw, N)` row in the same run. Under load both slow down
*together*, so their ratio holds where the absolutes wander. The tree-walker is a
stable in-process reference — you don't even need a baseline binary. (Divan sorts
rows by label when printing, so `(Vm, N)` and `(Tw, N)` are not necessarily printed
next to each other; `scripts/bench_ratio.py` pairs them by `(bench, size)`. What
matters is one process, not one screen line.)

The grid is built from `Tier::ALL` and the labels come from `Tier::short()`
(`eval/compile/mod.rs`), so a new tier gets rows in every eval bench and a column in
`bench_ratio.py` without either being edited. `Tw` stays the reference — it is the stable
baseline the method rests on, not a tier under test.

Since ADR-222 there are **three** rows per size, not two: `Jit` (ceiling 2), `Vm` (ceiling 1 —
the VM with native tiering off) and `Tw` (ceiling 0). The middle one is the addition, and it is
the useful one for JIT work: `Jit`÷`Vm` per row is exactly the JIT-vs-no-JIT ratio the frontier
docs quote (`fib` 54×, `collatz` 40×, and `nbody`'s 3.2× is what exposed a silently bailing arm),
which previously had to be produced by hand with `BROOD_NO_JIT`.

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
- `scripts/quickbench.sh` (3 samples) stays for a *throwaway* directional read of
  a single configuration; `scripts/bench.sh` archives a full headline run to
  `docs/benchmarks/<timestamp>.md` — save that for a **quiet** machine.

### When the tree-walker isn't a valid reference

The ratio trick assumes your change doesn't move the tree-walker. That holds for
VM-only work (the common case). If a change touches shared machinery (the reader,
a builtin, the GC), the TW row moves too and the ratio hides it — then you need a
genuine before/after, which on this machine means **interleaving**: run baseline
and candidate binaries in alternation (not back-to-back blocks) so they share the
load window, many samples each.

### Measuring a whole row by hand — the four ways it silently lies

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

## 2. Attribution — where the VM spends work

Build with the `perf-stats` cargo feature to arm process-global work counters
(`crates/lisp/src/perf.rs`). **Off by default** — every counter compiles to
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
   perf record --call-graph dwarf -- target/release-fast/brood bench.blsp
   perf report --no-children       # self time — what the frontier tables quote
   ```

**`make perf-symbols` overwrites the timing binary**, exactly as `make perf-brood` does — same
path, same trap. Re-run `make release-brood` before timing anything, or you time a build with
debug info and no strip and charge your change for it. `file target/release-fast/brood` says
`stripped` / `not stripped` if you are unsure which one you have.

Why the same flags matter: `perf-symbols` keeps `PERF_RUSTFLAGS` (`debug-assertions=off`,
`overflow-checks=off`) and `RUN_FEATURES`, so it profiles the binary `make ab` measures. A profile
of a differently-built binary answers a question nobody asked.
