//! The linear-map rewrite recognises the tally a Brood user WRITES, not only the kernel
//! spelling. `(assoc m k (+ (get m k 0) e))` used to be an escape of the accumulator —
//! the fold stayed a path-copying CHAMP update with two prelude calls per element, 8x
//! the `%map-int-add` form nobody should have to know (brood-benchmarks `wordcount`:
//! 848 ms vs 99 ms). Now the probe (`LinIdiom`, `eval/compile/inline.rs`) admits the
//! prelude `get` as a read and the fused `assoc`/`+`/`get` shape as an update, and the
//! source rewrite (`eval/macros.rs`) emits `%table-get` / `%table-add`.
//!
//! These pin the EXPANSION, so a probe that quietly stops admitting the shape fails by
//! name rather than as a benchmark row. `tests/linmap_soundness_test.blsp` asserts the
//! values; the expansion is what says the fast path was taken at all.

use brood::Interp;

/// The fully macro-expanded top-level form, printed.
fn expansion(src: &str) -> String {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all(&mut interp.heap, src).expect("read");
    let global = interp.heap.global();
    let form = *forms.first().expect("one form");
    let expanded =
        brood::eval::macros::macroexpand_all(&mut interp.heap, form, global).expect("expand");
    interp.print(expanded)
}

/// `want` must appear in the expansion; `forbid` must not appear in the REWRITTEN loop —
/// the `linmap-loop` def — since the unrewritten copy behind the seed check keeps the
/// source spelling by design. With no split, the whole expansion is the loop.
fn assert_split(src: &str, want: &[&str], forbid: &[&str]) {
    let out = expansion(src);
    for w in want {
        assert!(
            out.contains(w),
            "expected `{w}` in the expansion of {src}:\n{out}"
        );
    }
    let rewritten = match out.find("linmap-loop") {
        Some(i) => out[i..].split("(def ").next().unwrap(),
        None => &out,
    };
    for f in forbid {
        assert!(
            !rewritten.contains(f),
            "unexpected `{f}` in the rewritten loop of {src}:\n{rewritten}"
        );
    }
}

const SPLIT: &str = "linmap-loop";

#[test]
fn the_split_keeps_an_unrewritten_copy_behind_the_seed_check() {
    // The wrapper seeds a table from the accumulator's input map, and a table cannot hold
    // every value a map can (a rope) nor stand in for a record (its misses consult
    // `Lookup`). `%table-from-map` answers nil for those, and the wrapper must then run
    // the loop AS WRITTEN — a copy whose self-calls point at itself, not at the wrapper.
    let out = expansion(
        "(defn tally (xs m) (if (empty? xs) m (tally (rest xs) (%map-int-add m (first xs) 1))))",
    );
    assert!(
        out.contains("linmap-slow"),
        "no unrewritten copy in:\n{out}"
    );
    assert!(out.contains("%table-from-map"), "no seed copy in:\n{out}");
    // The slow copy recurses into itself: the only `(tally ` calls left are the def and
    // the wrapper's own name, never a self-call inside the slow body.
    let start = out.find("(def tally/linmap-slow").expect("slow def");
    let slow = out[start + 5..].split("(def ").next().unwrap();
    assert!(
        !slow.contains("(tally "),
        "the slow copy recurses through the wrapper:\n{slow}"
    );
    assert!(
        slow.contains("%map-int-add"),
        "the slow copy was rewritten:\n{slow}"
    );
}

#[test]
fn the_idiomatic_tally_is_fused_into_table_add() {
    // `(+ (get m k 0) e)` — the addend on the right.
    assert_split(
        "(defn tally (xs m) (if (empty? xs) m (tally (rest xs) (assoc m (first xs) (+ (get m (first xs) 0) 1)))))",
        &[SPLIT, "%table-add"],
        &["%map-assoc", "(get "],
    );
}

#[test]
fn the_addend_may_come_first() {
    // `(+ 1 (get m k 0))` — wordcount's spelling.
    assert_split(
        "(defn tally (xs m) (if (empty? xs) m (tally (rest xs) (assoc m (first xs) (+ 1 (get m (first xs) 0))))))",
        &[SPLIT, "%table-add"],
        &[],
    );
}

#[test]
fn a_let_bound_key_and_a_compound_addend_fuse() {
    // persistent-map's spelling: the key is a local, the addend an expression.
    assert_split(
        "(defn go (i x acc) (if (>= i 10) acc (let (k (math/rem x 7)) (go (+ i 1) (+ x 1) (assoc acc k (+ (get acc k 0) (+ 1 (math/rem k 3))))))))",
        &[SPLIT, "%table-add"],
        &[],
    );
}

#[test]
fn a_constant_key_fuses() {
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc :hits (+ (get acc :hits 0) 1)))))",
        &[SPLIT, "%table-add"],
        &[],
    );
}

#[test]
fn a_get_read_beside_the_update_is_a_table_get() {
    assert_split(
        "(defn go (i acc) (if (>= i 10) (get acc 0 -1) (go (+ i 1) (assoc acc 1 (+ (get acc 1 0) (get acc 0 0))))))",
        &[SPLIT, "%table-add", "%table-get"],
        &["(get "],
    );
}

#[test]
fn a_two_arity_get_read_is_a_table_get_with_a_nil_default() {
    assert_split(
        "(defn go (i acc) (if (>= i 10) (get acc 0) (go (+ i 1) (assoc acc 1 (+ (get acc 1 0) 1)))))",
        &[SPLIT, "(%table-get acc 0 nil)"],
        &[],
    );
}

#[test]
fn inc_dec_and_subtraction_fuse_too() {
    // `(inc (get m k 0))` — the pocket reference's own spelling — is a %table-add of 1…
    assert_split(
        "(defn go (xs m) (if (empty? xs) m (go (rest xs) (assoc m (first xs) (inc (get m (first xs) 0))))))",
        &[SPLIT, "(%table-add m (first xs) 1)"],
        &["(inc "],
    );
    // …`dec` a %table-sub of 1, and `(- (get m k 0) e)` a %table-sub of e.
    assert_split(
        "(defn go (xs m) (if (empty? xs) m (go (rest xs) (assoc m (first xs) (dec (get m (first xs) 0))))))",
        &[SPLIT, "(%table-sub m (first xs) 1)"],
        &[],
    );
    assert_split(
        "(defn go (i m) (if (>= i 10) m (go (+ i 1) (assoc m :k (- (get m :k 0) (* 2 i))))))",
        &[SPLIT, "(%table-sub m :k (* 2 i))"],
        &["%table-add"],
    );
}

// ---- what must NOT fuse: each of these has semantics the table op would change ----

#[test]
fn a_subtraction_from_the_addend_is_not_a_tally() {
    // `(- e (get m k 0))` stores e minus the count — not a read-modify-write of the count.
    assert_split(
        "(defn go (i m) (if (>= i 10) m (go (+ i 1) (assoc m :k (- 100 (get m :k 0))))))",
        &[],
        &[SPLIT, "%table-sub", "%table-add"],
    );
}

#[test]
fn a_different_read_key_is_not_a_tally() {
    // `(assoc m k (+ (get m j 0) 1))` stores k from j's count — a plain assoc, an escape.
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc i (+ (get acc 0 0) 1)))))",
        &[],
        &[SPLIT, "%table-add"],
    );
}

#[test]
fn a_non_zero_default_is_not_a_tally() {
    // `(+ (get m k 1) e)` seeds an absent key at 1+e, which table-add would make e.
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc i (+ (get acc i 1) 1)))))",
        &[],
        &[SPLIT, "%table-add"],
    );
}

#[test]
fn a_two_arity_get_in_the_sum_is_not_a_tally() {
    // `(+ (get m k) e)` raises on an absent key; table-add would store e.
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc i (+ (get acc i) 1)))))",
        &[],
        &[SPLIT, "%table-add"],
    );
}

#[test]
fn a_global_key_is_not_a_tally() {
    // The source reads the key twice; a global can change between the reads.
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc *k* (+ (get acc *k* 0) 1)))))",
        &[],
        &[SPLIT, "%table-add"],
    );
}

#[test]
fn a_plain_assoc_still_declines_the_split() {
    // The table cannot hold every value `%map-assoc` can; a non-tally assoc is an escape.
    assert_split(
        "(defn go (i acc) (if (>= i 10) acc (go (+ i 1) (assoc acc i (str i)))))",
        &[],
        &[SPLIT],
    );
}

#[test]
fn the_kernel_spelling_splits_to_the_same_op() {
    // `%map-int-add` → `%table-add` too, never `%table-incr`: the incr raises where `+`
    // promotes, which made the rewrite observable at the i64 boundary and on a float.
    assert_split(
        "(defn tally (xs m) (if (empty? xs) m (tally (rest xs) (%map-int-add m (first xs) 1))))",
        &[SPLIT, "%table-add"],
        &["%table-incr"],
    );
}
