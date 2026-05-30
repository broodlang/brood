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
    // Run on a large, explicitly-sized stack — like the `brood`/`nest` binaries
    // (see `crates/cli/src/main.rs`). The in-language suite runs its `:isolated`
    // units *on the runner thread*, and some legitimately recurse non-tail a few
    // hundred frames (heavy in a debug build); the stack-budget guard (ADR-043)
    // is sized for a `CORO_STACK_BYTES` stack, so the cargo test-harness thread's
    // small default stack would overflow before the guard could fire a clean
    // error. Sizing this thread to match makes the guard behave as it does under
    // the real binaries. The body runs entirely inside this thread.
    let handle = std::thread::Builder::new()
        .name("brood-suite".into())
        .stack_size(brood::process::CORO_STACK_BYTES)
        .spawn(run_suite)
        .expect("spawn brood-suite thread");
    handle.join().expect("brood-suite thread panicked");
}

fn run_suite() {
    std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .expect("cd to repo root");
    // Match `nest test` / `brood --test`: default a memory ceiling on (ADR-043)
    // so the in-language suite (which includes tests/adversarial_test.blsp) can't
    // OOM the host. An explicit BROOD_MEM_LIMIT still wins.
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str("(require 'project) (project/run-project-tests)") {
        panic!("Brood test suite failed: {}", e);
    }
}
