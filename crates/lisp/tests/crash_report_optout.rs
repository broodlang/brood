//! `BROOD_NO_CRASH_REPORT` must opt out without loading the reporter (ADR-313).
//!
//! **Its own test binary on purpose.** This mutates the process environment, and `make asan`
//! runs plain `cargo test`, where every test in a binary shares one process and runs on
//! parallel threads — so beside `crash_report_lazy.rs` this variable leaked into that test
//! and made it fail claiming the reporter "did not arm". That is KI-86 again (a flag one
//! test sets and the runtime reads once), and nextest hides it by giving every test its own
//! process. A separate binary makes the isolation real rather than harness-dependent.

use brood::Interp;

/// Is `sym` globally bound in this image?
fn bound(interp: &mut Interp, sym: &str) -> bool {
    let out = interp
        .eval_str(&format!("(bound? '{sym})"))
        .unwrap_or_else(|e| panic!("evaluating (bound? '{sym}) failed: {e}"));
    interp.print(out) == "true"
}

/// A name only `crash-report` defines.
const REPORTER_ONLY_NAME: &str = "crash-report/render";

#[test]
fn the_opt_out_arms_nothing_and_still_loads_nothing() {
    // `BROOD_NO_CRASH_REPORT` is read with `%getenv`, not `os/env`, precisely so that
    // deciding NOT to arm cannot load `os` + `string` + `reflect` + `math`.
    unsafe { std::env::set_var("BROOD_NO_CRASH_REPORT", "1") };
    let mut interp = Interp::new();
    let armed = interp
        .eval_str("(%crash-report-arm-default)")
        .expect("arm-default under the opt-out");
    assert_eq!(interp.print(armed), "nil", "the opt-out did not opt out");
    assert!(
        !bound(&mut interp, REPORTER_ONLY_NAME),
        "opting OUT of the crash reporter still loaded it"
    );
    assert!(
        !bound(&mut interp, "os/env"),
        "the opt-out path loaded `os` — read the env var with %getenv, not os/env"
    );
    unsafe { std::env::remove_var("BROOD_NO_CRASH_REPORT") };
}
