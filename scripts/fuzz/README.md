# Brood stress / fuzz harness

Differential + chaos stress tools used to hunt GC/VM/JIT and distribution bugs.
Re-runnable. Build the armed binary first (per-deref GC tripwire + heap verifier):

```
RUSTFLAGS="-C debug-assertions=on" cargo build --release --features jit --bin brood
```

## Differential / oracle fuzzing

```
scripts/fuzz/run.sh <generator> [count] [base-seed]
```

Generates `count` programs and runs each under the tree-walker (reference),
VM-no-JIT (`BROOD_NO_JIT=1`), VM+JIT (default), and GC-stress
(`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`); flags any output **divergence** between
engines, any `BAD` line (oracle generators self-check), or any **crash** (a
crashing input is copied next to this README). Generators (`scripts/fuzz/generators/`):

| generator | what it stresses |
|-----------|------------------|
| `numeric` / `arithmetic` | numeric tower (int/float/bignum/decimal), div/overflow/shift edges |
| `metamorphic` | semantics-preserving rewrites of a body — all must agree (JIT lowering) |
| `tier_transition` | arg types cycle int→float→bignum across iterations (JIT deopt/retier) |
| `match` | pattern matching, value-derived oracle |
| `quasiquote` | `` ` ``/`~`/`~@` vs an explicit constructor oracle |
| `trycatch` | errors raised inside JIT'd code stay catchable (no abort) |
| `rope` | rope edits vs a string oracle (unicode, newlines) |
| `syntax` | malformed source — the reader must never crash (only clean errors) |
| `checker` | random/malformed `(sig …)` — the checker must never panic/hang |

Each generator is `python3 generators/<g>.py <count> <base-seed> <outdir>`.

## Distribution chaos

```
scripts/fuzz/dist_chaos.sh <run-id>               # mesh churn + wrong-cookie flood + dead-peer dials
scripts/fuzz/dist_chaos_remote_spawn.sh <run-id>  # remote-spawn + monitor under node deaths
```

Spawns a TCP node mesh under traffic, kills nodes (incl. the hub) mid-flight, and
flags any node that exits with a crash code (134/139/132/101 — not the harness's
own SIGKILL=137), any panic/SIGSEGV in a node's stderr, or a new `.brood_crash_dump`.

## cargo-fuzz (coverage-guided, ASAN)

See `crates/lisp/fuzz/README.md` (`reader` + `eval` targets; needs nightly).
