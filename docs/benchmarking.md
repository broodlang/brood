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

The load-robust signal is the **VM ÷ tree-walker ratio measured as adjacent rows
in one `divan` process**. Both engines live in every binary; the `eval` benches
pin each row to an engine (`set_forced_engine`, the `engine_grid!` macro), so
size `N` appears as neighbouring `(Vm, N)` and `(Tw, N)` rows. Under load both
slow down *together*, so their ratio holds where the absolutes wander. The
tree-walker is a stable in-process reference — you don't even need a baseline
binary.

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
