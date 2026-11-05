//! `nest test` says it is a test run and gives the run a private scratch home (ADR-602).
//!
//! A project's test writes a state file under `$XDG_CONFIG_HOME` the way a program keeps
//! its files, and records what `test/running?` and `test/scratch-dir` answered. Run with
//! `HOME` at a temp directory standing in for the user's: the write must land in the run's
//! scratch home, never under `HOME/.config`, and the scratch home must be gone when the run
//! ends. `--real-home` keeps the real homes, and the same write lands under `HOME/.config`.
//!
//! ADR-608: `BROOD_TEST_ARM=1` makes a plain `nest run` (an editor's generated test runner,
//! a warm test child) the same test run; the REPL history goes to the scratch home too; a
//! nested run keeps the user's config base; and a purge takes only the homes of runs that are
//! gone, however old a live one is.

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
        .env_remove("BROOD_TEST_ARM")
        .env_remove("BROOD_TEST_SCRATCH")
        .env_remove("BROOD_USER_CONFIG_HOME")
        .env_remove("BROOD_HISTORY")
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

/// What a script run by `nest run` records about its environment, one `pr-str` vector:
/// running?, the scratch dir, the three XDG homes, the REPL history, the user's config base
/// and whether the arm request is still in its environment.
const RECORD_SCRIPT: &str = "(file/spit (os/env \"MARKER\")\n\
     \x20\x20(pr-str (vec (map [\"BROOD_TEST_RUN\" \"BROOD_TEST_SCRATCH\" \"XDG_CONFIG_HOME\" \"XDG_STATE_HOME\"\n\
     \x20\x20               \"XDG_DATA_HOME\" \"BROOD_HISTORY\" \"BROOD_USER_CONFIG_HOME\" \"BROOD_TEST_ARM\"]\n\
     \x20\x20  os/env))))\n";

/// `nest run record.blsp` in `dir` (no project) with `HOME` at `home`, a private temp root
/// and cache, and `extra` environment on top. The record it wrote, as its fields.
fn nest_run_record(dir: &Path, home: &Path, extra: &[(&str, &str)]) -> Vec<Option<String>> {
    let script = dir.join("record.blsp");
    let marker = dir.join("record-out.blsp");
    std::fs::write(&script, RECORD_SCRIPT).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_nest"));
    command
        .current_dir(dir)
        .arg("run")
        .arg(&script)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("MARKER", &marker);
    for name in [
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "BROOD_TEST_HOME",
        "BROOD_TEST_RUN",
        "BROOD_TEST_ARM",
        "BROOD_TEST_SCRATCH",
        "BROOD_USER_CONFIG_HOME",
        "BROOD_HISTORY",
    ] {
        command.env_remove(name);
    }
    command.envs(extra.iter().copied());
    let out = command.output().expect("run nest run");
    assert!(
        out.status.success(),
        "nest run failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&marker).expect("the script recorded what it saw");
    parse_record(&text)
}

/// The fields of a `pr-str`'d vector of strings and nils.
fn parse_record(text: &str) -> Vec<Option<String>> {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let mut fields = Vec::new();
    let mut rest = inner.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("nil") {
            fields.push(None);
            rest = after.trim_start();
        } else {
            let after = rest.strip_prefix('"').expect("a string field");
            let end = after.find('"').expect("a closed string");
            fields.push(Some(after[..end].to_string()));
            rest = after[end + 1..].trim_start();
        }
    }
    fields
}

#[test]
fn an_arm_request_makes_a_plain_nest_run_a_test_run_with_its_own_home() {
    let base = temp_dir("arm-request");
    let home = base.path.join("home");
    std::fs::create_dir_all(&home).unwrap();

    let unarmed = nest_run_record(&base.path, &home, &[]);
    assert_eq!(
        unarmed[0], None,
        "a plain nest run is no test run: {unarmed:?}"
    );

    let armed = nest_run_record(&base.path, &home, &[("BROOD_TEST_ARM", "1")]);
    assert_eq!(armed[0].as_deref(), Some("1"), "BROOD_TEST_RUN: {armed:?}");
    let scratch = armed[1].clone().expect("a scratch home");
    assert!(scratch.contains("brood-test-home-"), "scratch: {scratch}");
    assert_eq!(armed[2], Some(format!("{scratch}/config")));
    assert_eq!(armed[3], Some(format!("{scratch}/state")));
    assert_eq!(armed[4], Some(format!("{scratch}/data")));
    assert_eq!(
        armed[5],
        Some(format!("{scratch}/state/brood_history")),
        "the REPL history stays in the scratch home"
    );
    assert_eq!(
        armed[6],
        Some(format!("{}/.config", home.display())),
        "the user's config base is kept"
    );
    assert_eq!(
        armed[7], None,
        "the request was consumed, so a child joins this run instead of arming its own"
    );
    assert!(
        !Path::new(&scratch).exists(),
        "the scratch home outlived the run: {scratch}"
    );
}

#[test]
fn a_run_inside_a_run_keeps_the_users_config_base() {
    let base = temp_dir("nested-run");
    let home = base.path.join("home");
    let outer_scratch_config = base.path.join("outer-scratch/config");
    let real_config = base.path.join("real-config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&outer_scratch_config).unwrap();
    let outer = outer_scratch_config.to_string_lossy().to_string();
    let real = real_config.to_string_lossy().to_string();

    // What an outer run hands down: its scratch as XDG_CONFIG_HOME, the user's base kept.
    let nested = nest_run_record(
        &base.path,
        &home,
        &[
            ("BROOD_TEST_ARM", "1"),
            ("BROOD_TEST_RUN", "1"),
            ("XDG_CONFIG_HOME", &outer),
            ("BROOD_USER_CONFIG_HOME", &real),
        ],
    );
    assert_eq!(
        nested[6].as_deref(),
        Some(real.as_str()),
        "a nested run pointed nest's preferences at the outer run's scratch"
    );
    assert_ne!(
        nested[2].as_deref(),
        Some(outer.as_str()),
        "a fresh home of its own"
    );
}

#[test]
fn a_purge_keeps_a_live_runs_home_however_old_and_takes_a_dead_ones() {
    let base = temp_dir("purge-liveness");
    let temp_root = base.path.join("tmp");
    let home = base.path.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let live = temp_root.join("brood-test-home-1-live");
    let dead = temp_root.join("brood-test-home-2-dead");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&dead).unwrap();
    // This test process is the long-running run: it holds the live home's lock.
    let held = std::fs::File::create(live.join("run.lock")).unwrap();
    held.try_lock().expect("hold the live run's lock");
    std::fs::File::create(dead.join("run.lock")).unwrap();
    let ancient = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    for dir in [&live, &dead] {
        std::fs::File::open(dir)
            .unwrap()
            .set_modified(ancient)
            .unwrap();
    }
    let temp = temp_root.to_string_lossy().to_string();

    nest_run_record(
        &base.path,
        &home,
        &[("BROOD_TEST_ARM", "1"), ("TMPDIR", &temp)],
    );

    assert!(
        live.exists(),
        "a second run purged a live run's home mid-run"
    );
    assert!(!dead.exists(), "a dead run's home was kept");
    drop(held);
}
