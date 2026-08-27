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
    // Sample the UCD Part1 sweep here, and only here. Its ~16,000 cases x 6 normalisations
    // are ~670 s of DEBUG work — more than nextest's whole-binary cap for this suite (600 s),
    // so the full sweep cannot fit in this wrapper however the inner budget is set. A
    // 1-in-16 slice is ~42 s and still ~1,000 cases, and the release path (`nest test`,
    // `make suite`) keeps sweeping all of Part1 in ~5 s. See the knob's comment in
    // tests/conformance_ucd_test.blsp for why sampling only started working once the
    // per-test 20,000-line walk was collected once instead.
    // Set it in the ENVIRONMENT, not as a global in the program below: the test file reads
    // this when it LOADS, which is after that `eval_str` would have `def`'d it, so the
    // file's own default won and the sweep stayed full (which is how this was missed).
    // SAFETY: single-threaded, before the interpreter loads any test file.
    unsafe { std::env::set_var("BROOD_UCD_PART1_OF", "16") };
    // Same treatment, same reason, for the heavier `nboyer` size in
    // tests/conformance_gabriel_test.blsp. That one test measured **256 s here — a third
    // of the entire `make test` wall** (715 s of in-language suite inside a 790 s run)
    // against 19.6 s on the release path: upstream's scaling parameter roughly triples the
    // rewrite count per step, so n=2's 1.8M rewrites dominate everything else in the suite
    // put together. Capping at n=1 keeps 0.6M rewrites — still 6x the 95k baseline, so the
    // sustained-allocation pressure the case exists for is still applied, and applied under
    // debug-assertions, which is the only thing this wrapper offers that the release path
    // does not. `nest test` / `make suite` keep running both sizes, and the *answer* is
    // pinned independently by the small-size case against upstream's published table.
    // SAFETY: as above.
    unsafe { std::env::set_var("BROOD_GABRIEL_NBOYER_MAX_N", "1") };
    // Same treatment, same reason, for tests/jit_deep_recursion_test.blsp (the KI-14
    // guard-page regression). Those three cases are ~2.7 s release but ~210 s here — 38%
    // of this wrapper — because the property under test is deep recursion and a debug
    // frame is far heavier. The knob scales *repetition* only (warm-up passes, and how
    // much of the 188-document corpus the third case re-scans); both deep documents are
    // still parsed at full depth in a spawned process, so the regression stays guarded.
    // Release (`nest test`, `make suite`) keeps the exhaustive form.
    // SAFETY: as above — single-threaded, before any test file loads.
    unsafe { std::env::set_var("BROOD_JDR_OF", "4") };
    if let Err(e) = interp.eval_str(
        "(require-one 'test) (def *test-timeout-ms* 600000) \
         (project/run-tests)",
    ) {
        panic!("Brood test suite failed: {}", e);
    }
}
