//! Arming the default crash reporter must NOT load `crash-report` (ADR-313).
//!
//! The reporter is armed before every `brood file`, `nest run`, bundle and REPL run, and
//! until 2026-09-01 the arm was `crash-report/arm-default` — a function inside the module
//! it might decide not to arm. Reaching it loaded `crash-report` and its whole require
//! closure (`io/puts` alone brings `io`, `file`, `path`, `string`, `reflect`, `math`;
//! `os/env` another four), which measured **9.0 ms of a ~24 ms `brood file` run**: ten
//! modules materialised instead of one, on every run, whether or not anything crashed.
//!
//! The arm now lives in the prelude (`%crash-report-arm-default`): it subscribes to the
//! kernel's abnormal exits — which is the part that genuinely cannot wait, since
//! `sysmon::crash_reported_elsewhere` reads it and a crash before we subscribe is a crash
//! nobody reports — and reaches the reporting code only through the qualified call to
//! `crash-report/take-over` inside its listener, which ADR-246's autoload stubs resolve at
//! CALL time.
//!
//! This is a one-token regression in both directions: any bare `crash-report/…` reference
//! reintroduced into the arm path silently makes it eager again, and nothing else in the
//! tree would notice — `tests/crash_report_test.blsp` cannot, because its own
//! `(:use crash-report)` header loads the module before any test there runs. Hence a
//! separate file with a fresh `Interp`, which carries the prelude and nothing else.

use brood::Interp;

/// Is `sym` globally bound in this image?
fn bound(interp: &mut Interp, sym: &str) -> bool {
    let out = interp
        .eval_str(&format!("(bound? '{sym})"))
        .unwrap_or_else(|e| panic!("evaluating (bound? '{sym}) failed: {e}"));
    interp.print(out) == "true"
}

/// A name only `crash-report` defines. `render` is public and has no namesake elsewhere,
/// so `false` means "the module has not loaded" rather than "spelled differently".
const REPORTER_ONLY_NAME: &str = "crash-report/render";

#[test]
fn arming_the_default_reporter_does_not_load_the_crash_report_module() {
    let mut interp = Interp::new();
    // Sanity: absent in a fresh image, so a later `false` is evidence of nothing loading
    // rather than of the name never existing.
    assert!(
        !bound(&mut interp, REPORTER_ONLY_NAME),
        "{REPORTER_ONLY_NAME} was bound in a fresh image — this test cannot conclude anything"
    );

    let armed = interp
        .eval_str("(%crash-report-arm-default)")
        .expect("arming the default crash reporter");
    assert_ne!(
        interp.print(armed),
        "nil",
        "the reporter did not arm, so this test proves nothing about laziness \
         (is BROOD_NO_CRASH_REPORT set in this environment?)"
    );

    assert!(
        !bound(&mut interp, REPORTER_ONLY_NAME),
        "arming the default crash reporter loaded `crash-report` — the arm is eager again \
         (ADR-313). Look for a bare `crash-report/…` reference on the arm path in \
         std/prelude/process.blsp; it must appear only inside the listener's receive."
    );
}

// `the_opt_out_arms_nothing_and_still_loads_nothing` lives in `crash_report_optout.rs`, in
// its OWN test binary. It sets `BROOD_NO_CRASH_REPORT` in the process environment, and
// `make asan` runs plain `cargo test`, which puts every test in one process on parallel
// threads — so the variable leaked into the test above and made it assert that arming had
// failed. Exactly KI-86's shape (two tests `set_var`-ing a flag the runtime reads once).
// nextest hid it by giving each test its own process; a separate binary fixes it for both.
