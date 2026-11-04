//! `nest test` says it is a test run and gives the run a private scratch home (ADR-602).
//!
//! A project's test writes a state file under `$XDG_CONFIG_HOME` the way a program keeps
//! its files, and records what `test/running?` and `test/scratch-dir` answered. Run with
//! `HOME` at a temp directory standing in for the user's: the write must land in the run's
//! scratch home, never under `HOME/.config`, and the scratch home must be gone when the run
//! ends. `--real-home` keeps the real homes, and the same write lands under `HOME/.config`.

use std::path::Path;
use std::process::Command;

mod support;
use support::temp_dir;

/// A project whose one test writes `$XDG_CONFIG_HOME/scratchapp/state.blsp` and records, in
/// the file `$MARKER` names (outside any XDG home), `running?`, `scratch-dir` and the config
/// home it saw.
fn write_project(root: &Path) {
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"scratchapp\" :version \"0.1.0\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/home_test.blsp"),
        "(defmodule home-test (:use test))\n\
         (describe \"the run's home\"\n\
         \x20\x20(test \"writes its state under the XDG config home\"\n\
         \x20\x20\x20\x20(let (dir (path/join (or (os/env \"XDG_CONFIG_HOME\") (path/join (path/home) \".config\")) \"scratchapp\"))\n\
         \x20\x20\x20\x20\x20\x20(file/mkdir dir)\n\
         \x20\x20\x20\x20\x20\x20(file/spit (path/join dir \"state.blsp\") \"[1]\")\n\
         \x20\x20\x20\x20\x20\x20(file/spit (os/env \"MARKER\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20(pr-str [(test/running?) (test/scratch-dir) (os/env \"XDG_CONFIG_HOME\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20(os/env \"XDG_STATE_HOME\") (os/env \"XDG_DATA_HOME\")]))\n\
         \x20\x20\x20\x20\x20\x20(is true))))\n",
    )
    .unwrap();
}

/// `nest test` in `project` with `HOME` at `home`, a private cache, and `extra` arguments.
fn nest_test(project: &Path, home: &Path, cache: &Path, marker: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(project)
        .arg("test")
        .args(extra)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", cache)
        .env("MARKER", marker)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("BROOD_TEST_HOME")
        .env_remove("BROOD_TEST_RUN")
        .output()
        .expect("run nest test");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "nest test failed:\n{text}");
    text
}

#[test]
fn a_test_run_writes_a_scratch_home_never_the_users() {
    let base = temp_dir("scratch-home");
    let project = base.path.join("project");
    let home = base.path.join("home");
    let cache = base.path.join("cache");
    let marker = base.path.join("marker.blsp");
    std::fs::create_dir_all(&home).unwrap();
    write_project(&project);

    nest_test(&project, &home, &cache, &marker, &[]);

    assert!(
        !home.join(".config/scratchapp").exists(),
        "the test's state reached the user's ~/.config"
    );
    let seen = std::fs::read_to_string(&marker).expect("the test recorded what it saw");
    assert!(
        seen.starts_with("[true \""),
        "test/running? and scratch-dir: {seen}"
    );
    let scratch = seen
        .split('"')
        .nth(1)
        .expect("a scratch dir in the record")
        .to_string();
    assert!(
        scratch.contains("brood-test-home-"),
        "scratch dir: {scratch}"
    );
    assert!(
        seen.contains(&format!("\"{scratch}/config\"")),
        "config home: {seen}"
    );
    assert!(
        seen.contains(&format!("\"{scratch}/state\"")),
        "state home: {seen}"
    );
    assert!(
        seen.contains(&format!("\"{scratch}/data\"")),
        "data home: {seen}"
    );
    assert!(
        !Path::new(&scratch).exists(),
        "the scratch home outlived the run: {scratch}"
    );
}

#[test]
fn real_home_keeps_the_users_homes() {
    let base = temp_dir("real-home");
    let project = base.path.join("project");
    let home = base.path.join("home");
    let cache = base.path.join("cache");
    let marker = base.path.join("marker.blsp");
    std::fs::create_dir_all(&home).unwrap();
    write_project(&project);

    nest_test(&project, &home, &cache, &marker, &["--real-home"]);

    let seen = std::fs::read_to_string(&marker).expect("the test recorded what it saw");
    // still a test run, but no scratch home: XDG_CONFIG_HOME stays unset
    assert!(
        seen.starts_with("[true nil nil"),
        "with --real-home: {seen}"
    );
    assert!(
        home.join(".config/scratchapp/state.blsp").exists(),
        "the write went elsewhere"
    );
}
