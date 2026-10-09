//! Regressions from the 2026-10-08 review: interval arithmetic that overflows `i64`
//! widens instead of saturating (ADR-350), and a top-level definition of a reserved name
//! is reported before it fails at run time (ADR-166).

use super::*;
use crate::types::Range;

#[test]
fn an_overflowing_interval_end_widens_instead_of_saturating() {
    // `(+ i64::MAX 1)` saturated to `i64::MAX`, so the body — 1 at run time, `+`
    // promotes past i64 — read as exactly 0: a correct `(int 1 1)` was reported as
    // violated by "0", and a WRONG `(int 0 0)` passed silently.
    let body = "(- (+ 9223372036854775807 1) 9223372036854775807)";
    let w = file_warnings(&format!("(sig one (-> (int 1 1)))\n(defn one () {body})"));
    assert!(
        w.iter().all(|m| !m.contains("yields 0")),
        "no wrong exact claim: {w:?}"
    );
    // What remains is the checker's ordinary answer for an int it cannot bound — the same
    // verdict an unbounded int parameter gets.
    let unbounded = file_warnings("(sig one (int -> (int 1 1)))\n(defn one (x) (+ x 0))");
    let unbounded_warns = unbounded.iter().any(|m| m.contains("yields int"));
    assert_eq!(
        w.iter().any(|m| m.contains("yields int")),
        unbounded_warns,
        "{w:?}"
    );
    // …and the wrong `(int 0 0)` is no longer "proved" by the saturated 0.
    let w = file_warnings(&format!("(sig zero (-> (int 0 0)))\n(defn zero () {body})"));
    assert_eq!(
        w.iter().any(|m| m.contains("yields int")),
        unbounded_warns,
        "{w:?}"
    );
    // The lattice directly: each operation widens the overflowing end only.
    let sum = Range::plus(Range::point(i64::MAX), Range::point(1));
    assert_eq!((sum.lo, sum.hi), (None, None));
    let partly = Range::plus(Range::new(Some(0), Some(i64::MAX)), Range::point(1));
    assert_eq!((partly.lo, partly.hi), (Some(1), None));
    let negated = Range::negated(Range::new(Some(i64::MIN), Some(0)));
    assert_eq!((negated.lo, negated.hi), (Some(0), None));
    let product = Range::times(Range::new(Some(-4), Some(i64::MAX)), Range::point(2));
    assert_eq!((product.lo, product.hi), (Some(-8), None));
    let negative = Range::times(Range::new(Some(i64::MIN), Some(3)), Range::point(2));
    assert_eq!((negative.lo, negative.hi), (None, Some(6)));
}

#[test]
fn a_top_level_definition_of_a_reserved_name_warns() {
    // `(defn dec () 1)` passed `--check` and always failed at run time (E0030).
    let w = file_warnings("(defn dec () 1)");
    assert!(
        w.iter().any(|m| m.contains("`dec` is a reserved name")),
        "the runtime refuses this def, so the checker must say so: {w:?}"
    );
    let w = file_warnings("(def map 1)");
    assert!(
        w.iter().any(|m| m.contains("`map` is a reserved name")),
        "{w:?}"
    );
    // An ordinary name, and a module's own definition of a shipped spelling, are fine.
    let w = file_warnings("(defn my-dec () 1)");
    assert!(w.iter().all(|m| !m.contains("reserved")), "{w:?}");
    let w = file_warnings("(defmodule my-app)\n(defn dec () 1)");
    assert!(w.iter().all(|m| !m.contains("reserved")), "{w:?}");
}
