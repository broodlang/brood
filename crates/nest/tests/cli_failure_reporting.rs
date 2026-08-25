//! `nest` argument-handling / exit-code regressions.
//!
//! Each case drives the real binary, because each bug was invisible to a unit test:
//! they are about what the *process* reports, not what a function returns.

use std::path::Path;
use std::process::Command;

fn nest() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nest"))
}

/// A scratch directory with no `project.blsp` anywhere above it, so `nest run FILE`
/// takes its documented outside-a-project path.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-nest-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// `nest run --for` is documented as the CI-friendly way to exercise a long-running
/// app "end-to-end and in CI without a manual `timeout`". It used to report SUCCESS
/// when the app crashed: `--for`/`--watch` wrap the program in a monitored process,
/// and the `[:down …]` arm printed the reason and fell out of the eval's `Ok`. So a
/// crash printed a stack trace and exited 0 — in the one mode whose whole point is
/// that a machine, not a person, reads the result.
#[test]
fn run_for_exits_nonzero_when_the_program_dies() {
    let dir = scratch("for");
    let file = dir.join("boom.blsp");
    std::fs::write(&file, "(println \"starting\")\n(error \"boom\")\n").unwrap();

    let out = nest()
        .current_dir(&dir)
        .args(["run", "--for", "5s", "boom.blsp"])
        .output()
        .expect("run nest");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        text.contains("[exit]"),
        "expected the monitor report: {text}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a crashed program must not report success (stdout: {text})"
    );

    // The other half of the contract: a program that finishes, and one stopped by the
    // cap itself, are both successes.
    std::fs::write(dir.join("ok.blsp"), "(println \"fine\")\n").unwrap();
    let out = nest()
        .current_dir(&dir)
        .args(["run", "--for", "5s", "ok.blsp"])
        .output()
        .expect("run nest");
    assert_eq!(out.status.code(), Some(0), "a clean run must exit 0");

    std::fs::write(
        dir.join("spin.blsp"),
        "(defn spin () (sleep 20) (spin))\n(spin)\n",
    )
    .unwrap();
    let out = nest()
        .current_dir(&dir)
        .args(["run", "--for", "700ms", "spin.blsp"])
        .output()
        .expect("run nest");
    assert_eq!(
        out.status.code(),
        Some(0),
        "reaching the --for cap is the intended outcome, not a failure"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `require_readable_files` probed with `metadata`, which a DIRECTORY satisfies. So
/// `nest check src` got past the boundary guard and failed deep inside Brood with
/// `check-file-deps: cannot read src: Is a directory (os error 21)` and a four-frame
/// trace — the internals-leak that guard exists to prevent.
#[test]
fn check_rejects_a_directory_at_the_boundary() {
    let dir = scratch("dir");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.blsp"), "(defn f () 1)\n").unwrap();

    let out = nest()
        .current_dir(&dir)
        .args(["check", "src"])
        .output()
        .expect("run nest");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "stderr: {err}");
    assert!(
        err.starts_with("nest check: src is a directory"),
        "the boundary must say it, not Brood's internals: {err}"
    );
    assert!(
        !err.contains("check-file-deps"),
        "an internal frame leaked: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The default `nest release` output name is the manifest's `:name` — data from a
/// project.blsp that may not be ours. `(project :name |../../escaped-app|)` wrote a
/// 30 MB **executable** two directories above the project root with no `-o` and no
/// warning. An explicit `-o` is still unrestricted; only the defaulted name is
/// required to be a plain filename.
///
/// The check runs before any runtime is resolved, so this needs no embedded runtime
/// and never builds one.
#[test]
fn release_refuses_a_traversing_default_output_name() {
    let dir = scratch("rel");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/main.blsp"),
        "(defn main () (println \"x\"))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project :name |../../escaped-app| :main main)\n",
    )
    .unwrap();

    let out = nest()
        .current_dir(&dir)
        .arg("release")
        .output()
        .expect("run nest");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(2), "stderr: {err}");
    assert!(
        err.contains("not a plain filename") && err.contains("nest release -o"),
        "must name the problem and the way out: {err}"
    );
    assert!(
        !Path::new("/tmp/escaped-app").exists(),
        "nothing may be written outside the project"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
