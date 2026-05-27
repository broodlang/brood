//! Microbenchmarks for the Brood interpreter core: startup, parsing, and the
//! evaluator hot path. Run with `cargo bench` (or `make benchmark`).
//!
//! Each `eval` benchmark builds a fresh `Interp` per iteration via
//! `with_inputs`, so the prelude build (a once-per-process `LazyLock`) and the
//! per-instance seeding stay out of the measured region — we time parse + eval
//! of the program itself.

use brood::{syntax::reader, Interp};

fn main() {
    divan::main();
}

/// Standing up a fresh interpreter. The prelude is built once per process; this
/// measures the per-instance cost (seeding the runtime code region from the
/// frozen prelude bindings + cloning the shared `Arc`s).
#[divan::bench]
fn interp_new() -> Interp {
    Interp::new()
}

/// Parsing only — read the whole prelude into `Value`s, no evaluation. A
/// representative chunk of real Brood source for the reader.
#[divan::bench]
fn parse_prelude(bencher: divan::Bencher) {
    let src = include_str!("../../../std/prelude.blsp");
    bencher
        .with_inputs(Interp::new)
        .bench_refs(|interp| reader::read_all(&mut interp.heap, src).unwrap());
}

/// Tail-recursive sum to N — exercises the load-bearing `'tail:` loop in
/// `eval` (proper tail calls, O(1) Rust stack). Arithmetic is defined in Brood,
/// so this also stresses prelude function-call dispatch.
#[divan::bench(args = [1_000, 10_000, 100_000])]
fn sum_tail(bencher: divan::Bencher, n: u64) {
    let src = format!(
        "(def sum-to (fn [n acc] (if (= n 0) acc (sum-to (- n 1) (+ acc n))))) (sum-to {n} 0)"
    );
    bencher
        .with_inputs(Interp::new)
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// Naive (non-tail) recursive Fibonacci — exercises function-call overhead and
/// the growing-then-unwinding Rust call stack. fib(25) is ~242k calls.
#[divan::bench(args = [15, 20, 25])]
fn fib(bencher: divan::Bencher, n: u64) {
    let src =
        format!("(def fib (fn [n] (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))) (fib {n})");
    bencher
        .with_inputs(Interp::new)
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}
