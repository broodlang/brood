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
