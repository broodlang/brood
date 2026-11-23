//! A warm `nest run` reports a duplicate global WITHOUT reading a source file (ADR-779).
//!
//! The run's pre-flight warns when two source files bind the same global. It used to learn
//! what each file defines by re-reading and re-parsing every file in the entry point's
//! closure on every launch, which was a quarter of bedit's warm start (0.28 s → 0.21 s).
//! The definitions now live in the module index (ADR-380), keyed by each file's size and
//! mtime, so an unchanged project's pre-flight opens no source file for them.
//!
//! The observable: after a warm-up run, one source file is made unreadable. Everything a
//! warm run needs — the startup image, the module index, the cached verdicts — is keyed on
//! `stat`, which needs no read permission, so the run still starts and still prints the
//! duplicate-def warning. A pre-flight that re-reads source fails on that file.
//!
//! Sabotage-verified: with `project-check-duplicate-defs` scanning the files again
//! (`project-gather-extracts`) instead of reading the index, the third run reads the
//! unreadable file and this test fails.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A project directory and a cache directory of its own, so cases run concurrently.
fn scratch() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "brood-warm-run-reads-no-source-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    let project = base.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    let cache = base.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    (project, cache)
}

fn write(dir: &Path, relative: &str, source: &str) {
    std::fs::write(dir.join(relative), source).unwrap();
}

fn nest_run(project: &Path, cache: &Path) -> (Output, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .env("XDG_CACHE_HOME", cache)
        .current_dir(project)
        .output()
        .expect("spawn nest");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output, stdout, stderr)
}

/// The run succeeded, ran the program, and reported the duplicate.
fn assert_reported(label: &str, run: &(Output, String, String)) {
    let (output, stdout, stderr) = run;
    assert!(
        output.status.success(),
        "{label}: nest run failed: status={:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("ANSWER 4"),
        "{label}: the program did not run\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("global 'app/shared' is defined in 2 source files"),
        "{label}: the duplicate-def warning is missing\nstderr:\n{stderr}"
    );
}

#[test]
fn a_warm_run_reports_a_duplicate_global_without_reading_source() {
    let (project, cache) = scratch();
    write(
        &project,
        "project.blsp",
        "(project\n  :name dupprobe\n  :main app)\n",
    );
    write(
        &project,
        "src/app.blsp",
        "(defmodule app (:use extra))\n(defn main () (io/puts (str \"ANSWER \" (twice 2))))\n(defn shared () 1)\n",
    );
    let extra = project.join("src/extra.blsp");
    write(
        &project,
        "src/extra.blsp",
        // Top level, not under `check-allow`: the lint reads top-level def forms only. The
        // pre-flight's cross-module-def advisory this also draws is not what is asserted.
        "(defmodule extra)\n(defn twice (x) (* 2 x))\n(defn /app/shared () 2)\n",
    );

    // Cold, then warm: the image, the module index and the verdicts are all written.
    assert_reported("cold run", &nest_run(&project, &cache));
    assert_reported("warm run", &nest_run(&project, &cache));

    std::fs::set_permissions(&extra, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&extra).is_ok() {
        // Running as root: permissions do not stop a read, so there is nothing to observe.
        std::fs::set_permissions(&extra, std::fs::Permissions::from_mode(0o644)).unwrap();
        eprintln!("skipped: a file with mode 000 is still readable (running as root)");
        return;
    }
    let unreadable = nest_run(&project, &cache);
    std::fs::set_permissions(&extra, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_reported("warm run with src/extra.blsp unreadable", &unreadable);

    let _ = std::fs::remove_dir_all(project.parent().unwrap());
}
