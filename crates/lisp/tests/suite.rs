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
    // is sized for a `WORKER_STACK_BYTES` stack, so the cargo test-harness thread's
    // small default stack would overflow before the guard could fire a clean
    // error. Sizing this thread to match makes the guard behave as it does under
    // the real binaries. The body runs entirely inside this thread.
    let handle = std::thread::Builder::new()
        .name("brood-suite".into())
        .stack_size(brood::process::WORKER_STACK_BYTES)
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
    // `*test-timeout-ms*` is a wall deadline for an ENTIRE BATCH of parallel workers
    // (`collect-units` computes `(+ (now) *test-timeout-ms*)` once per chunk), not a
    // per-test hang guard: any worker that hasn't reported by then is presumed hung, and
    // every test it holds is reported as timed out. So the budget has to cover the
    // slowest *chunk*, and this wrapper is the one place the suite runs in a **debug**
    // build, where every case is roughly an order of magnitude slower than the release
    // path `nest test` uses. The default 120 s is sized for release and is simply too
    // tight here — it blamed whichever conformance worker happened to report last
    // (2026-07-26), which is a measurement artifact, not a hung test.
    //
    // Raise it for this context only, the same "the harness needs different limits"
    // reasoning as the memory ceiling above. The release path keeps the 120 s default, so
    // a genuinely hung test is still caught quickly where it matters.
    // ── The three sampling knobs that used to live here are gone (2026-08-27). ──
    //
    // `BROOD_UCD_PART1_OF=16`, `BROOD_GABRIEL_NBOYER_MAX_N=1` and `BROOD_JDR_OF=4` each cut
    // real coverage out of THIS wrapper — a 1-in-16 slice of the UCD normalisation sweep, the
    // smaller nboyer size, a quarter of the deep-recursion repetitions — and every one of them
    // was justified by the same sentence: this is the one place the suite runs in a **debug**
    // build, where a case costs roughly an order of magnitude more than on the release path.
    // The recorded figures were 670 s / 256 s / 210 s here against 3.6 s / 19.6 s / 2.7 s for
    // `nest test`.
    //
    // That premise no longer holds. `[profile.test]` is `opt-level = 2` with
    // `debug-assertions = true` (see the workspace Cargo.toml), so this wrapper now runs at
    // roughly release speed with every tripwire still armed — which was the only thing it
    // offered over the release path, and the reason it must not simply be deleted.
    //
    // Measured on the same commit: the FULL, unsampled suite is **66 s** here, against **933 s**
    // for the SAMPLED one under the old profile. More coverage, an order of magnitude less time,
    // so there is nothing left for the knobs to buy. Each remains readable from the environment
    // by its own test file, so a developer can still narrow a run by hand; this wrapper simply
    // stops forcing them.
    //
    // If this wrapper ever needs shrinking again, re-read that paragraph first: sampling here
    // is how coverage quietly diverged between `make test` and `nest test` in the first place.
    if let Err(e) = interp.eval_str(
        "(require-one 'test) (def *test-timeout-ms* 600000) \
         (project/run-tests)",
    ) {
        panic!("Brood test suite failed: {}", e);
    }
}
