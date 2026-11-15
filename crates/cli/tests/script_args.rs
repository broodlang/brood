//! A script must be able to take its own arguments.
//!
//! `brood` accepts `[FILE]...`, so before `--` was given a meaning every trailing argument
//! parsed as another file to run: `brood run.blsp -- --publish` tried to *open a file named
//! `--publish`* and exited with "cannot read --publish". A script therefore could not have
//! options at all, and the workaround was to configure it through the environment — which is
//! how `scripts/release-ecosystem.blsp` came to be driven by `PUBLISH=1`.
//!
//! Two properties are asserted here, because fixing this could plausibly break the other:
//! post-`--` arguments reach the program and are NOT opened as files, and pre-`--` arguments
//! are still files (brood genuinely runs several).

use std::process::Command;

mod support;
use support::script;

fn run(args: &[&str]) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .args(args)
        .output()
        .expect("run brood");
    (
        String::from_utf8_lossy_owned(output.stdout),
        String::from_utf8_lossy_owned(output.stderr),
        output.status.success(),
    )
}

const PRINT_ARGS: &str = r#"(require-one 'system)
(io/puts (pr-str (system/script-args)))
"#;

#[test]
fn post_double_dash_arguments_reach_the_script() {
    let path = script("script-args-reach", PRINT_ARGS);
    let (stdout, stderr, ok) = run(&[
        path.to_str().unwrap(),
        "--",
        "--root",
        "/some/path",
        "--publish",
    ]);
    assert!(ok, "brood failed: {stderr}");
    assert_eq!(stdout.trim(), r#"["--root" "/some/path" "--publish"]"#);
}

/// The actual regression: a hyphenated argument must not be treated as a filename.
#[test]
fn a_flag_after_double_dash_is_not_opened_as_a_file() {
    let path = script("script-args-notafile", PRINT_ARGS);
    let (_, stderr, ok) = run(&[path.to_str().unwrap(), "--", "--publish"]);
    assert!(ok, "brood failed: {stderr}");
    assert!(
        !stderr.contains("cannot read"),
        "brood tried to open the argument as a file: {stderr}"
    );
}

#[test]
fn a_script_with_no_arguments_sees_an_empty_vector() {
    let path = script("script-args-empty", PRINT_ARGS);
    let (stdout, stderr, ok) = run(&[path.to_str().unwrap()]);
    assert!(ok, "brood failed: {stderr}");
    assert_eq!(stdout.trim(), "[]");
}

/// Guards the other half: `brood` runs several files, and routing arguments away from
/// `files` must not stop it.
#[test]
fn arguments_before_the_separator_are_still_files() {
    let first = script("script-args-multi1", "(io/puts \"first\")\n");
    let second = script("script-args-multi2", "(io/puts \"second\")\n");
    let (stdout, stderr, ok) = run(&[first.to_str().unwrap(), second.to_str().unwrap()]);
    assert!(ok, "brood failed: {stderr}");
    assert!(
        stdout.contains("first") && stdout.contains("second"),
        "both files should have run, got: {stdout}"
    );
}

/// `argv` stays the raw invocation — a bundled app (which boots before any CLI parsing)
/// depends on seeing exactly what the OS handed it.
#[test]
fn argv_still_reports_the_raw_invocation() {
    let path = script(
        "script-args-raw",
        "(require-one 'system)\n(io/puts (pr-str (system/argv)))\n",
    );
    let (stdout, stderr, ok) = run(&[path.to_str().unwrap(), "--", "--flag"]);
    assert!(ok, "brood failed: {stderr}");
    assert!(
        stdout.contains("--") && stdout.contains("--flag"),
        "argv should include the separator and the argument, got: {stdout}"
    );
}
