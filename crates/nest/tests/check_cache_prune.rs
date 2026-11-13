//! The check cache (`$XDG_CACHE_HOME/brood/check/<sha256 of root>`) is pruned of week-old
//! entries — at most once a DAY, not on every run.
//!
//! Every run that touched the cache directory (`nest test`'s `--failed` record, `nest check`)
//! used to sweep it: list it and stat every entry. Each ephemeral project root leaves an entry
//! behind, so on a machine that runs tests the directory held 2 067 of them, and the sweep was
//! 18 ms of every one-file `nest test` — more than the file's tests took (2026-11-13). A stamp
//! file's mtime now says when the last sweep ran.
//!
//! Both directions are asserted, because each failure is silent: a sweep that never runs
//! again lets the cache grow without bound, and one that runs every time is the cost again.

use std::path::Path;
use std::time::{Duration, SystemTime};

mod support;
use support::temp_dir;

fn set_age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
    std::fs::File::open(path)
        .and_then(|file| file.set_modified(when))
        .expect("set mtime");
}

fn nest_test(project: &Path, cache: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_nest"))
        .args(["test", "tests/one_test.blsp"])
        .current_dir(project)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .expect("run nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn the_check_cache_is_swept_once_a_day_not_on_every_run() {
    let tmp = temp_dir("check-cache-prune");
    let project = tmp.path.join("proj");
    let cache = tmp.path.join("cache");
    let checks = cache.join("brood").join("check");
    std::fs::create_dir_all(project.join("tests")).unwrap();
    std::fs::create_dir_all(&checks).unwrap();
    std::fs::write(project.join("project.blsp"), "(project :name pruned)\n").unwrap();
    std::fs::write(
        project.join("tests/one_test.blsp"),
        "(defmodule one-test (:use test))\n(describe \"one\" (test \"runs\" (is true)))\n",
    )
    .unwrap();

    // No stamp yet: the first run sweeps, and a week-old entry goes.
    let first_old = checks.join("first-abandoned-root");
    std::fs::create_dir_all(&first_old).unwrap();
    set_age(&first_old, 8);
    let out = nest_test(&project, &cache);
    assert!(out.contains("1 tests, 1 passed"), "first run:\n{out}");
    assert!(
        !first_old.exists(),
        "the first run did not sweep a week-old cache entry:\n{out}"
    );
    let stamp = checks.join(".last-prune");
    assert!(stamp.exists(), "the sweep left no stamp:\n{out}");

    // Stamped just now: the next run does not sweep, so a second week-old entry survives.
    let second_old = checks.join("second-abandoned-root");
    std::fs::create_dir_all(&second_old).unwrap();
    set_age(&second_old, 8);
    let out = nest_test(&project, &cache);
    assert!(out.contains("1 tests, 1 passed"), "second run:\n{out}");
    assert!(
        second_old.exists(),
        "a run within a day of the last sweep swept again — the per-run cost is back:\n{out}"
    );

    // A day and more later, the next run sweeps again.
    set_age(&stamp, 2);
    let out = nest_test(&project, &cache);
    assert!(out.contains("1 tests, 1 passed"), "third run:\n{out}");
    assert!(
        !second_old.exists(),
        "a run a day after the last sweep did not sweep — the cache would grow forever:\n{out}"
    );
}
