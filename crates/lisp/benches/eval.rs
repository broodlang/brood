//! Microbenchmarks for the Brood interpreter core: startup, parsing, and the
//! evaluator hot path. Run with `cargo bench` (or `make benchmark`).
//!
//! Each `eval` benchmark builds a fresh `Interp` per iteration via
//! `with_inputs`, so the prelude build (a once-per-process `LazyLock`) and the
//! per-instance seeding stay out of the measured region — we time parse + eval
//! of the program itself.
//!
//! **Both execution engines are measured side by side** (ADR-076): every eval
//! benchmark runs once under the closure-compiling **VM** (the default engine)
//! and once under the **tree-walker** (`BROOD_VM=0`'s fallback), labelled `Vm` /
//! `Tw` in the arg column. The engine is pinned per-input via
//! `compile::set_forced_engine` (the same override the differential test uses),
//! so a single `cargo bench` shows the speedup the VM buys — e.g. `fib 20` is
//! ~7.3 ms on the VM vs ~13 ms on the tree-walker. Don't read a single number as
//! "the" eval cost without noting which engine row it's on.

use brood::eval::compile::set_forced_engine;
use brood::{syntax::reader, Interp};

fn main() {
    divan::main();
}

/// Which execution engine a benchmark row is pinned to. `Debug` is what divan
/// prints in the arg column (`Vm` / `Tw`). Since ADR-100 Stage 5 the VM *is* the
/// bytecode stepping engine (the `Node`-walking executor was retired); `Tw` is the
/// tree-walker fallback.
#[derive(Clone, Copy, Debug)]
enum Eng {
    Vm,
    Tw,
}

/// A fresh interpreter with the given engine forced on this thread for the
/// measured region. `set_forced_engine` takes precedence over the `BROOD_VM`
/// env default (`compile::vm_enabled`), and divan runs this input setup on the
/// same worker thread as the benched closure, so the pin holds through the eval.
fn interp_on(eng: Eng) -> Interp {
    set_forced_engine(Some(matches!(eng, Eng::Vm)));
    Interp::new()
}

/// `[(Vm, n), (Tw, n)]` for every `n` — the engine × size grid each eval
/// benchmark iterates, so the two engines sit on adjacent rows per size.
macro_rules! engine_grid {
    ($($n:expr),+ $(,)?) => {
        [ $( (Eng::Vm, $n), (Eng::Tw, $n) ),+ ]
    };
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
#[divan::bench(args = engine_grid![1_000, 10_000, 100_000])]
fn sum_tail(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src = format!(
        "(def sum-to (fn [n acc] (if (= n 0) acc (sum-to (- n 1) (+ acc n))))) (sum-to {n} 0)"
    );
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// `(count (map inc …))` — the `defseq` self-recursive `--loop` (ADR-096 round 2)
/// running on the VM via the **self-call optimization** (`Step::SelfTail`). `map`
/// is a prelude `defn`, so its body is RUNTIME-region and VM-compiles; calling it
/// at top level (no `def` wrapper, a *named* mapper so no deferring top-level
/// lambda) exercises the self-tail-call per element. Adjacent Vm/Tw rows give the
/// load-robust ratio. (NB: a *top-level* `(letrec (s (fn …)) …)` does **not** test
/// this — its `fn` is LOCAL-region and defers to the tree-walker by design, which
/// is why an earlier top-level-letrec bench misread as a big win when it was
/// actually parity. See docs/benchmarking.md.)
#[divan::bench(args = engine_grid![3_000, 30_000])]
fn defseq_map(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src = format!("(count (map inc (range {n})))");
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

#[divan::bench(args = engine_grid![200_000, 1_000_000])]
fn reduce_range(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    // `(reduce <named-fn> 0 (range n))` — drives the `%range-reduce` native,
    // which calls the reducer back per element through `apply_value` when
    // `vm_enabled` (commit `4af9d2a`). A named `defn` reducer is RUNTIME-region
    // and VM-compiles, so the Vm row shows a clear speedup over the Tw row
    // (~65–67% faster measured at the time of routing).
    let src = format!("(defn rf (a x) (+ a (* x 2))) (reduce rf 0 (range {n}))");
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// A `(try … (catch e …))` body — the `try` macro wraps the body in a LOCAL
/// `(fn () …)` thunk, so `apply_engine` (which routes VM-eligible callees)
/// falls back to the tree-walker for this LOCAL thunk regardless of engine.
/// The inner `acc-sum` recursion therefore runs on TW in both rows, and the
/// Vm/Tw ratio is ~1.0. This is the expected and correct result: it confirms
/// `try`-heavy code has no regression, and that the routing benefit lands only
/// when the thunk itself is a RUNTIME closure (a `defn` passed directly to
/// `%try`, rather than the typical macro-expanded LOCAL wrapper).
#[divan::bench(args = engine_grid![1_000, 10_000])]
fn try_body(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src = format!(
        "(defn acc-sum (n acc) (if (= n 0) acc (acc-sum (- n 1) (+ acc n)))) \
         (try (acc-sum {n} 0) (catch _ -1))"
    );
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// `(apply f …)`-driven tail recursion. The VM's `dispatch` now unfolds `apply`
/// inline (like the TW's `'dispatch` loop) and re-dispatches the real callee —
/// so a RUNTIME-region `loop-` runs on the VM via `Step::Tail` trampolining with
/// O(1) Rust stack. `apply_builtin` itself stays on `eval::apply` for the TW
/// fallback path only. The Vm row should be clearly faster than Tw.
#[divan::bench(args = engine_grid![10_000, 100_000])]
fn apply_driven(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src =
        format!("(def loop- (fn (n) (if (= n 0) :done (apply loop- (list (- n 1)))))) (loop- {n})");
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// Naive (non-tail) recursive Fibonacci — exercises function-call overhead and
/// the growing-then-unwinding Rust call stack. fib(25) is ~242k calls.
#[divan::bench(args = engine_grid![15, 20, 25])]
fn fib(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src =
        format!("(def fib (fn [n] (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))) (fib {n})");
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// Tail-recursive `cons`-builder — every iteration resolves the *global* `cons`
/// (a full lexical-chain walk to GLOBAL, then a globals-table probe) plus one
/// allocation. The clearest measure of the global-lookup + dispatch tax the
/// eval-dispatch campaign targets; the
/// later lexical-addressing step should move this most.
#[divan::bench(args = engine_grid![10_000, 100_000])]
fn cons_build(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src = format!(
        "(def build (fn [n acc] (if (= n 0) acc (build (- n 1) (cons n acc))))) \
         (count (build {n} nil))"
    );
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// End-to-end Brood `(sort < …)` — the workload that motivated the campaign.
/// Forces the in-language `merge-sort` path (custom comparator), not the Rust
/// `%sort-asc` fast-path, so it reflects interpreter dispatch over list-walking.
/// Data is built in-language (xorshift) so parsing stays out of the hot region.
#[divan::bench(args = engine_grid![1_000, 5_000])]
fn sort_brood(bencher: divan::Bencher, (eng, n): (Eng, u64)) {
    let src = format!(
        "(def gen (fn [n seed acc] \
           (if (= n 0) acc \
             (let (x (bit-xor seed (bit-shift-left seed 13)) \
                   y (bit-xor x (bit-shift-right x 7)) \
                   z (bit-xor y (bit-shift-left y 17))) \
               (gen (- n 1) z (cons (rem (bit-and z 1048575) 1000000) acc)))))) \
         (def data (gen {n} 123456789 nil)) \
         (count (sort < data))"
    );
    bencher
        .with_inputs(|| interp_on(eng))
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}
