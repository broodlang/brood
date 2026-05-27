//! Runs the Brood-level test suite (`tests/suite.lisp`) through the
//! interpreter. The suite signals failure by raising an error, so an `Ok`
//! result means every in-language assertion passed.

use brood::Interp;

#[test]
fn brood_suite_passes() {
    // The suite does `(require 'test)` itself, and the framework is embedded in
    // the binary — so no file paths or working-directory assumptions here.
    let suite = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/suite.lisp"));
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str(suite) {
        panic!("Brood test suite failed: {}", e);
    }
}

/// The dedicated, exhaustive pattern-matching suite (its own file, so it can grow
/// without bloating the main suite). Same convention: it raises on any failure.
#[test]
fn brood_pattern_matching_suite_passes() {
    let suite = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/pattern_matching.lisp"
    ));
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str(suite) {
        panic!("Brood pattern-matching suite failed: {}", e);
    }
}
