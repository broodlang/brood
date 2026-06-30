# Brood fuzz targets (cargo-fuzz)

Coverage-guided fuzzers (libFuzzer + AddressSanitizer). Requires a nightly
toolchain and `cargo-fuzz`:

```
rustup toolchain install nightly
cargo install cargo-fuzz
```

Run (nightly is forced per-invocation, so it needn't be your default):

```
cd crates/lisp
RUSTUP_TOOLCHAIN=nightly cargo fuzz run reader -- -max_total_time=120
RUSTUP_TOOLCHAIN=nightly cargo fuzz run eval   -- -max_total_time=120 -timeout=10
```

Targets:
- **reader** — `bytes -> reader::read_all`: the reader must never panic/abort on
  any input, only return `Ok` or a clean `Err`.
- **eval** — `bytes -> Interp::eval_str`: the full evaluator must never
  panic/abort/corrupt memory (ASAN-checked) on any parseable input; `Err` is fine.

A crash writes a reproducer to `artifacts/<target>/`. `corpus/`, `artifacts/`,
and `target/` are git-ignored.
