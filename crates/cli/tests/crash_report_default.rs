//! A crash in an unsupervised process still prints a report through the LAZY arm.
//!
//! ADR-313 moved the arm out of `crash-report` and into the prelude, so the module is
//! loaded by the first crash rather than by every run (9.0 ms of ~24 ms — see
//! `crates/lisp/tests/crash_report_lazy.rs`). That test proves the module stays unloaded;
//! this one proves the reporting still WORKS once it is, which is the half a laziness
//! optimization breaks. It runs the real `brood file` entry point — the one a user
//! reaches — rather than calling `take-over` directly, because the handoff from the
//! prelude shim to the module is exactly the seam under test.

use std::io::Write;
use std::process::Command;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-crashrep-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

fn stderr_of(path: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env_remove("BROOD_NO_CRASH_REPORT")
        .arg(path)
        .output()
        .expect("run brood");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_unsupervised_crash_is_reported_through_the_lazy_arm() {
    // The spawned process is unsupervised and unmonitored: nothing but the default
    // reporter can say anything about it. The root sleeps so the report lands before exit.
    let path = script(
        "boom",
        "(spawn (fn () (/ 1 0)))\n(sleep 500)\n(io/puts \"done\")\n",
    );
    let err = stderr_of(&path);
    assert!(
        err.contains("[crash]"),
        "no crash report for an unsupervised crash — the lazy arm subscribed but never \
         handed off to `crash-report/take-over` (ADR-313).\nstderr was:\n{err}"
    );
    assert!(
        err.contains("division by zero"),
        "the report reached stderr but without the reason.\nstderr was:\n{err}"
    );
}

#[test]
fn the_opt_out_still_suppresses_the_report() {
    let path = script(
        "quiet",
        "(spawn (fn () (/ 1 0)))\n(sleep 500)\n(io/puts \"done\")\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(&path)
        .output()
        .expect("run brood");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !err.contains("[crash]"),
        "BROOD_NO_CRASH_REPORT=1 still produced a crash report.\nstderr was:\n{err}"
    );
}
