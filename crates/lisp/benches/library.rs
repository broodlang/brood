//! Standard-library, pattern-matching, compile-pass, and concurrency
//! microbenchmarks — the parts of the language the `eval` core benches (startup,
//! parsing, tail/non-tail recursion) don't touch. Almost all of this lives in
//! Brood (`std/prelude.blsp`), so these measure the library *as written in the
//! language*. Run with `cargo bench` (or `make benchmark`).
//!
//! Like `eval.rs`, each builds a fresh `Interp` per iteration via `with_inputs`,
//! so the once-per-process prelude build stays out of the measured region — we
//! time parse + eval of the workload itself.

use brood::Interp;

fn main() {
    divan::main();
}

/// Eval a whole program in a fresh interpreter, timing the parse + eval.
fn bench_prog(bencher: divan::Bencher, src: String) {
    bencher
        .with_inputs(Interp::new)
        .bench_refs(|interp| interp.eval_str(&src).unwrap());
}

/// The sequence library (all Brood, over `first`/`rest`/`cons`): the everyday
/// list-processing workloads, which the recursion benches don't represent.
mod sequence {
    use super::*;

    /// `map` → `filter` → `reduce` over `0..n` — the canonical functional
    /// pipeline. Exercises higher-order calls, a closure per element, cons
    /// allocation, and the (fold-based) arithmetic/comparison operators.
    #[divan::bench(args = [1_000, 10_000])]
    fn pipeline(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(reduce + 0 (filter even? (map (fn (x) (* x x)) (range {n}))))"),
        );
    }

    /// `mapcat` over `0..n` — each element expands to a 2-list, all concatenated.
    /// Stresses the linear, tail-safe `append`/`mapcat` (was O(lists²)).
    #[divan::bench(args = [1_000, 10_000])]
    fn mapcat(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (mapcat (fn (x) (list x x)) (range {n})))"),
        );
    }

    /// Merge-sort `n` pseudo-shuffled integers — recursion- and comparison-heavy.
    #[divan::bench(args = [1_000, 10_000])]
    fn sort(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (sort (map (fn (x) (rem (* x 7919) {n})) (range {n}))))"),
        );
    }

    /// Same workload as `pipeline`, expressed as a transducer chain — `xmap`
    /// and `xfilter` fuse with the reducer into one pass over `range`, with no
    /// intermediate lists. Paired with `pipeline` to show the fusion win.
    #[divan::bench(args = [1_000, 10_000])]
    fn transduce_pipeline(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(transduce (comp (xmap (fn (x) (* x x))) (xfilter even?)) + 0 (range {n}))"),
        );
    }

    /// Same workload as `mapcat`, fused via `xmapcat` — feeds each expanded
    /// list's items straight into the reducer, no per-element intermediate.
    #[divan::bench(args = [1_000, 10_000])]
    fn transduce_mapcat(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(transduce (xmapcat (fn (x) (list x x))) (fn (acc _) (+ acc 1)) 0 (range {n}))"
            ),
        );
    }

    /// Same workload as `transduce_short_circuit` but expressed eagerly: the
    /// filter must run against every item of the n-long mapped list (no way to
    /// stop early). Paired with the transducer version to show the
    /// `xtake-while` / `reduced` short-circuit win.
    #[divan::bench(args = [10_000, 100_000])]
    fn pipeline_no_short_circuit(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(reduce + 0 (filter (fn (x) (< x 1000)) (map (fn (x) (* x x)) (range {n}))))"),
        );
    }

    /// Short-circuiting transducer: `xtake-while` returns `reduced` once
    /// squares cross the threshold, so the driver halts and the rest of the
    /// n-long input is never touched. Should be ~constant time regardless of
    /// `n` (only ~32 items processed before the first square ≥ 1000).
    #[divan::bench(args = [10_000, 100_000])]
    fn transduce_short_circuit(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(transduce (comp (xmap (fn (x) (* x x))) (xtake-while (fn (x) (< x 1000)))) + 0 (range {n}))"
            ),
        );
    }
}

/// The string library — char-indexed `substring`/`str` over short strings (the
/// rope/buffer engine in M2 is the home for large-text performance).
mod strings {
    use super::*;

    /// Join `n` numbers with a separator — the O(n) `join` (was O(total²)) + `str`.
    #[divan::bench(args = [1_000, 10_000])]
    fn join(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(string-length (join \", \" (map number->string (range {n}))))"),
        );
    }

    /// Split a long comma-separated string back into `n` pieces — char-indexed
    /// `index-of`/`substring` scanning.
    #[divan::bench(args = [1_000])]
    fn split(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (string-split (join \",\" (map number->string (range {n}))) \",\"))"),
        );
    }
}

/// Maps — currently insertion-ordered association vectors, so build-then-read is
/// O(n²). These are the benches that will show the HAMT win (ADR-030) when it
/// lands, with no surface change.
mod maps {
    use super::*;

    /// Build a map of `n` entries, then look every key up.
    #[divan::bench(args = [200, 1_000])]
    fn build_and_get(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(let (m (fold (fn (acc i) (assoc acc i (* i i))) {{}} (range {n}))) \
                 (fold (fn (s i) (+ s (get m i))) 0 (range {n})))"
            ),
        );
    }

    /// `frequencies` over `0..n` bucketed into 7 keys — one `assoc` per element.
    #[divan::bench(args = [1_000, 10_000])]
    fn frequencies(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (frequencies (map (fn (x) (rem x 7)) (range {n}))))"),
        );
    }

    // --- transient map build (docs/transients.md) ---------------------------
    // `build_via_into` is the end-to-end prelude path (`into {}` → `%map-into`),
    // now routed through the in-place transient builder; `build_transient` hits
    // the `%map-into` kernel hook directly. Both build an n-entry map from a
    // freshly-mapped `[k v]` sequence.

    /// Transient build: kernel `%map-into` mutates build-local trie nodes.
    #[divan::bench(args = [200, 1_000, 10_000])]
    fn build_transient(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (%map-into {{}} (map (fn (i) [i (* i i)]) (range {n}))))"),
        );
    }

    /// End-to-end prelude `into {}` (now routed through the transient builder).
    #[divan::bench(args = [200, 1_000, 10_000])]
    fn build_via_into(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!("(count (into {{}} (map (fn (i) [i (* i i)]) (range {n}))))"),
        );
    }
}

/// Pattern matching — the Brood `match` compiler emits nested `if`/`let`; this
/// times the generated dispatch, not the (once-only) expansion.
mod pattern {
    use super::*;

    /// Dispatch a vector-tagged value through a multi-clause `match`, `n` times.
    #[divan::bench(args = [1_000, 10_000])]
    fn dispatch(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(defn area (s) (match s ([:circle r] (* 3 r r)) ([:square w] (* w w)) \
                 ([:rect w h] (* w h)) (_ 0))) \
                 (fold (fn (acc i) (+ acc (area [:square i]))) 0 (range {n}))"
            ),
        );
    }
}

/// The macro compile pass in isolation (`macroexpand_all`, no eval). Run at every
/// load / definition boundary, and a heavy reader of the symbol interner
/// (`symbol_is` per node) — so it reflects interner read cost.
mod compile {
    use super::*;

    #[divan::bench]
    fn macroexpand(bencher: divan::Bencher) {
        // A macro-dense program: threading, cond, and/or, when — ~50 of each.
        let mut src = String::from("(do ");
        for i in 0..50 {
            src.push_str(&format!(
                "(when (and (> {i} 0) (or (even? {i}) false)) \
                 (cond (= {i} 1) :one (> {i} 10) :big else :small)) \
                 (-> {i} (+ 1) (* 2) (- 3)) "
            ));
        }
        src.push(')');

        bencher
            // Read the forms outside the timed region; time only the expansion.
            .with_inputs(|| {
                let mut interp = Interp::new();
                let forms = brood::syntax::reader::read_all(&mut interp.heap, &src).unwrap();
                (interp, forms)
            })
            .bench_refs(|(interp, forms)| {
                for &form in forms.iter() {
                    brood::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root)
                        .unwrap();
                }
            });
    }
}

/// The green-process scheduler — spawn, copy-on-send messaging, per-process
/// heaps, fan-in. None of this is exercised by the single-threaded benches, and
/// it's the language's whole point.
mod concurrency {
    use super::*;

    /// Fan-out / fan-in: spawn `n` green processes, each sends back a computed
    /// value; the parent collects all `n`. (Wall-clock, so noisier than the
    /// CPU-bound benches — it includes scheduling latency.)
    #[divan::bench(args = [100, 1_000])]
    fn spawn_fanout(bencher: divan::Bencher, n: usize) {
        bench_prog(
            bencher,
            format!(
                "(def me (self)) \
                 (defn sw (k) (if (= k 0) nil (do (spawn (send me (* k k))) (sw (- k 1))))) \
                 (defn coll (acc k) (if (= k 0) acc (coll (+ acc (receive (x x))) (- k 1)))) \
                 (do (sw {n}) (coll 0 {n}))"
            ),
        );
    }

    /// Fan-out send of a string payload to N workers (each replies with its
    /// length). With `payload_bytes >= SHARED_BLOB_THRESHOLD` (256 B), the
    /// per-send cost should be near-constant (atomic refcount incr,
    /// O(1) per send) because the bytes ride along as `Arc<SharedBlob>`. Below
    /// the threshold, the cost scales with `payload_bytes * n` (deep copy per
    /// send). Compare 100 B vs 10 000 B at the same `n` to see ADR-041 in
    /// action; the ratio is the size of the win.
    ///
    /// `big` is a LOCAL Shared string (path through `to_message`'s blob
    /// short-circuit), not a `def`'d RUNTIME string (which goes through the
    /// `promote` deep-copy path — a separate optimisation surface).
    #[divan::bench(args = [128, 10_000])]
    fn big_string_fanout(bencher: divan::Bencher, payload_bytes: usize) {
        let n = 100;
        bench_prog(
            bencher,
            format!(
                "(defn bsf-w (p) (receive (s (send p (string-length s))))) \
                 (defn bsf-sw (parent big k) \
                   (if (= k 0) nil \
                     (do (send (spawn (bsf-w parent)) big) \
                         (bsf-sw parent big (- k 1))))) \
                 (defn bsf-coll (acc k) \
                   (if (= k 0) acc (bsf-coll (+ acc (receive (x x))) (- k 1)))) \
                 (let (me (self) big (string-repeat \"a\" {payload_bytes})) \
                   (bsf-sw me big {n}) \
                   (bsf-coll 0 {n}))"
            ),
        );
    }
}
