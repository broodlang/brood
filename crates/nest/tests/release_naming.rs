//! **`nest release` refuses a name it must not write, and says why.**
//!
//! Deciding what a release binary is CALLED is policy, so it lives in Brood
//! (`project/release-plan` in `std/tool/project.blsp`); `nest` keeps only the mechanism
//! it alone can host — the runtime embedded in this binary, the byte assembly, the boot
//! check. These cases drive the real `nest` through that seam: argv in, exit code and
//! message out.
//!
//! Both refusals had no test before. The escaping-name one was recorded in a comment as
//! "verified" — by hand, once, on the day it was written — and a defaulted `:name` of
//! `../../escaped-app` really did write a 30 MB executable two directories above the
//! project root. That is the shape a test has to hold: not that an error was printed, but
//! that the file is NOT there.
//!
//! Every case passes `--runtime /nonexistent-base`, which is what makes them fast in BOTH
//! directions. It is unreachable while the refusal works, and the moment one stops working
//! the run walks into a base binary that cannot be read and exits 1 in milliseconds —
//! instead of resolving a real runtime, which on a tree with no cached one means CARGO
//! BUILDING the lean runtime. Sabotaging the guard proved that: the first case went from
//! 0.25 s to a two-minute nextest timeout, red for the right reason but unreadably slow.

use std::process::Command;

fn nest_in(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .env("BROOD_NO_STDIMAGE", "1")
        .args(args)
        .output()
        .expect("run nest");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A minimal releasable project under a fresh scratch **parent**, returning
/// `(parent, project)`. The parent matters: an escaping `:name` aims at it, so the test
/// can assert nothing landed there.
fn project_with_name(tag: &str, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let parent = std::env::temp_dir().join(format!("nest-release-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    let project = parent.join("app");
    std::fs::create_dir_all(project.join("src")).expect("scratch project");
    std::fs::write(
        project.join("project.blsp"),
        format!("(project :name \"{name}\" :version \"0.1.0\" :main main)\n"),
    )
    .expect("manifest");
    std::fs::write(
        project.join("src/main.blsp"),
        "(defmodule main)\n(defn main () 0)\n",
    )
    .expect("entry module");
    (parent, project)
}

#[test]
fn a_defaulted_name_that_escapes_the_project_directory_is_refused() {
    let (parent, project) = project_with_name("escape", "../escaped-app");
    let (code, _, err) = nest_in(&project, &["release", "--runtime", "/nonexistent-base"]);

    assert_eq!(code, 2, "stderr:\n{err}");
    assert!(
        err.contains("is not a plain filename"),
        "the refusal must say what is wrong with the name:\n{err}"
    );
    assert!(
        err.contains("nest release -o <path>"),
        "and how to proceed anyway:\n{err}"
    );
    // The point of the guard: no artifact outside the project. `..` from the project dir
    // is `parent`, which holds nothing but the project itself.
    assert!(
        !parent.join("escaped-app").exists(),
        "an executable was written OUTSIDE the project directory"
    );
}

#[test]
fn an_explicit_output_path_is_the_users_own_choice_and_is_not_refused() {
    // The mirror of the case above, and the reason the guard tests the DEFAULTED name
    // only: a guard that also refused `-o` would be indistinguishable from one that
    // refuses everything, and this test is what tells the two apart. Asserted on how far
    // it got — past naming, into resolving the base runtime — rather than on the absence
    // of a message, which an unrelated early exit would also satisfy.
    let (parent, project) = project_with_name("explicit", "../escaped-app");
    let out = parent.join("chosen-app");
    let (code, _, err) = nest_in(
        &project,
        &[
            "release",
            "-o",
            out.to_str().unwrap(),
            "--runtime",
            "/nonexistent-base",
        ],
    );
    assert!(
        !err.contains("is not a plain filename"),
        "an explicit -o must not be held to the defaulted-name rule:\n{err}"
    );
    assert_eq!(code, 1, "stderr:\n{err}");
    assert!(
        err.contains("cannot read runtime binary"),
        "the name was accepted and the release went on to resolve a runtime:\n{err}"
    );
}

#[test]
fn a_pinned_runtime_cannot_serve_a_target_matrix() {
    let (_, project) = project_with_name("matrix", "app");
    let (code, _, err) = nest_in(
        &project,
        &[
            "release",
            "--runtime",
            "/nonexistent-base",
            "--target",
            "x86_64-unknown-linux-gnu",
            "--target",
            "aarch64-apple-darwin",
        ],
    );

    assert_eq!(code, 2, "stderr:\n{err}");
    assert!(
        err.contains("--runtime names one base binary"),
        "the refusal must name the conflict:\n{err}"
    );
    // Refused on the argument combination, never by trying to read the base binary —
    // `/nonexistent-base` would exit 1 with a different message if the order slipped.
    assert!(
        !err.contains("cannot read runtime binary"),
        "the matrix check must come before resolving the runtime:\n{err}"
    );
}
