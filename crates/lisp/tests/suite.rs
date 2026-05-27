//! Runs the whole in-language test suite through the project test runner
//! (ADR-020): from the repo root it discovers every `tests/**/*_test.blsp`,
//! loads each (which only *registers* its tests), and runs them all once. The
//! runner raises on any failure, so an `Ok` result means every in-language
//! assertion passed.
//!
//! We `cd` to the repo root first so the runner's walk-up for `project.blsp` is
//! deterministic regardless of cargo's working directory. This is its own test
//! binary with a single test, so the process-wide `set_current_dir` is safe.

use brood::Interp;

#[test]
fn brood_suite_passes() {
    std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .expect("cd to repo root");
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str("(require 'project) (run-project-tests)") {
        panic!("Brood test suite failed: {}", e);
    }
}
