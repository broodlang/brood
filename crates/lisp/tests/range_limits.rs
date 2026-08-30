//! Realising a **lazy range** that cannot fit in memory must fail as a catchable Brood
//! error, not as a Rust panic.
//!
//! A `Value::Range` is O(1) however wide, so `(range 0 9223372036854775807)` is a legal
//! value; `Heap::range_len` saturates its count at `i64::MAX`, and `range_to_vec` used to
//! feed that straight into `Vec::with_capacity`. That exceeds `isize::MAX`, so it hit the
//! `capacity overflow` panic **immediately** — no large allocation, no slow path, nothing
//! for `try`/`catch` to intercept — and took the green process's whole worker with it.
//! The trigger is ordinary surface syntax: `seq`, `reverse` and `nth` on a range all
//! realise it.

use brood::Interp;

/// Every realising path over an absurdly wide range raises a catchable error.
#[test]
fn realising_an_unrealisable_range_is_a_catchable_error() {
    let mut interp = Interp::new();
    for form in [
        "(seq (range 0 9223372036854775807))",
        "(%range->list (%range 0 9223372036854775807 1))",
        // Descending, and a step that does not divide the span — the count still
        // saturates, and the same guard has to catch it.
        "(seq (range 9223372036854775807 -9223372036854775807 -3))",
    ] {
        let prog = format!("(try {form} (catch e :caught))");
        let v = interp
            .eval_str(&prog)
            .unwrap_or_else(|e| panic!("{form} escaped `try` as a hard error: {e:?}"));
        assert_eq!(
            interp.print(v),
            ":caught",
            "{form} should have raised a catchable error",
        );
    }
}

/// The error names the problem rather than leaking an allocator detail.
#[test]
fn the_refusal_explains_itself() {
    let mut interp = Interp::new();
    let v = interp
        .eval_str("(try (seq (range 0 9223372036854775807)) (catch e (:message e)))")
        .expect("caught");
    let msg = interp.print(v);
    assert!(
        msg.contains("range too large to realise"),
        "unhelpful message: {msg}",
    );
}

/// The guard must not have moved the line under anything that legitimately works: a
/// range small enough to realise still realises, exactly, at both ends.
#[test]
fn ordinary_ranges_still_realise() {
    let mut interp = Interp::new();
    for (form, want) in [
        ("(count (seq (range 0 5)))", "5"),
        ("(nth (range 10 20) 3)", "13"),
        ("(first (reverse (range 0 100)))", "99"),
        ("(count (seq (range 0 100000)))", "100000"),
        // Wide, but consumed lazily — never realised, so never refused.
        ("(count (take (range 0 9223372036854775807) 4))", "4"),
    ] {
        let v = interp
            .eval_str(form)
            .unwrap_or_else(|e| panic!("{form}: {e:?}"));
        assert_eq!(interp.print(v), want, "{form}");
    }
}
