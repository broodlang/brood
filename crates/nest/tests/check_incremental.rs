//! `nest check` is incremental FOR REAL (ADR-119, finished 2026-09-21): after a cold build
//! the next check re-derives nothing; an edit re-derives the edited file and its dependents
//! and no other.
//!
//! What broke it before: the ADR-119 cache reuses a file's verdict when its dependency
//! FINGERPRINT is unchanged, and a user global's fact in that fingerprint is its definition
//! site (`D<file>@<mtime>`, `types::check::deps::fact_of_sym`). A module materialised from
//! the project image carried no definition sites, so the fact of every project global read
//! `F` after an imaged start and `D…` after a source load — and no cached fingerprint could
//! match across the flip. The cache only ever hit image → image: every check after a cold
//! build re-checked the whole project, and so did every check after an edit (the edit makes
//! the image stale, so that check loads from source). Measured on 1 000 × 3k-line files:
//! unchanged re-check 154 s, then 15 s; a one-function edit 188 s. The image carries def
//! sites now (`boot/image.rs` `KIND_DEF_SITE`).
//!
//! And an UNCHANGED project is answered without loading it (ADR-382): the driver keys the
//! whole run's printed lines on the project fingerprint (every source and test file's path,
//! size and mtime, the dependency files, the binary), the checking mode, the walk-changing
//! flags and the list of files asked about, and replays them when the key recurs — no image
//! materialised, no file parsed, no fingerprint recomputed (15 s → tens of ms at 1 000 × 3k).
//!
//! Observables, under `BROOD_DERIVE_DBG=1`: `[check] reuse-test (N to re-check)` is the count
//! of files whose verdict the per-file cache could not reuse; `[check] replayed K lines` says
//! the run was a replay, and `[check] ensure-loaded` that it was not. Sabotage-verified: with
//! the def-site entries not written, the listed re-check reads `N == files`; with the replay
//! key comparison forced true, the arity-edit case replays the stale, warning-free verdict.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brood-check-incremental-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

/// `n` leaf modules nobody imports, plus `app` which `:use`s `helper`.
fn gen_project(dir: &Path, n: usize) {
    write(
        dir,
        "project.blsp",
        "(project\n  :name incr\n  :main app)\n",
    );
    write(
        dir,
        "src/app.blsp",
        "(defmodule app (:use helper))\n\n(defn main ()\n  (io/puts (str \"ANSWER: \" (helper-scale 4))))\n",
    );
    write(
        dir,
        "src/helper.blsp",
        "(defmodule helper)\n\n(defn helper-scale (x) (* x 10))\n",
    );
    for i in 0..n {
        write(
            dir,
            &format!("src/leaf{i}.blsp"),
            &format!("(defmodule leaf{i})\n\n(defn leaf{i}-f (x) (+ x {i}))\n(defn leaf{i}-g (x) (leaf{i}-f (* x 2)))\n"),
        );
    }
}

/// `nest check ARGS…` in `dir` with the check-phase trace on: (exit code, stdout + stderr).
/// A private cache dir per project, so cases never see each other's verdicts.
fn nest_check_with(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.arg("check")
        .args(args)
        .env("BROOD_DERIVE_DBG", "1")
        .env("XDG_CACHE_HOME", dir.join(".xdg-cache"))
        .current_dir(dir);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn nest");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code().unwrap_or(-1), combined)
}

fn nest_check(dir: &Path) -> (i32, String) {
    nest_check_with(dir, &[], &[])
}

fn replayed(output: &str) -> bool {
    output.lines().any(|l| l.starts_with("[check] replayed "))
}

fn loaded(output: &str) -> bool {
    output
        .lines()
        .any(|l| l.starts_with("[check] ensure-loaded "))
}

/// The `N` of `[check] reuse-test (N to re-check)`.
fn to_recheck(output: &str) -> usize {
    let line = output
        .lines()
        .find(|l| l.starts_with("[check] reuse-test ("))
        .unwrap_or_else(|| panic!("no reuse-test line in:\n{output}"));
    line.trim_start_matches("[check] reuse-test (")
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn built_from_source(output: &str) -> bool {
    output.lines().any(|l| l.starts_with("Building "))
}

#[test]
fn the_check_after_a_cold_build_reuses_every_verdict() {
    let dir = scratch("cold");
    gen_project(&dir, 6);
    let files = 8;

    let (code, out1) = nest_check(&dir);
    assert_eq!(code, 0, "cold check failed:\n{out1}");
    assert!(
        built_from_source(&out1),
        "the first check should build the image:\n{out1}"
    );
    assert_eq!(
        to_recheck(&out1),
        files,
        "a cold cache re-checks everything:\n{out1}"
    );

    // Nothing changed: the whole run is replayed from its record — the image is not even
    // installed, and no file is parsed.
    let (code, out2) = nest_check(&dir);
    assert_eq!(code, 0, "warm check failed:\n{out2}");
    assert!(
        replayed(&out2),
        "an unchanged project should replay:\n{out2}"
    );
    assert!(
        !loaded(&out2),
        "a replay must not load the project:\n{out2}"
    );
    assert!(
        !built_from_source(&out2),
        "a replay must not build:\n{out2}"
    );

    // The same files LISTED is a different question (a different key), so this run takes
    // the per-file cache, materialising from the image — the arrival that used to flip every
    // global's fact and re-check the whole project. Every verdict must be reused.
    let listed: Vec<String> = (0..6)
        .map(|i| format!("src/leaf{i}.blsp"))
        .chain(["src/app.blsp".to_string(), "src/helper.blsp".to_string()])
        .collect();
    let args: Vec<&str> = listed.iter().map(|s| s.as_str()).collect();
    let (code, out3) = nest_check_with(&dir, &args, &[]);
    assert_eq!(code, 0, "listed check failed:\n{out3}");
    assert!(
        !replayed(&out3) && loaded(&out3),
        "the listed form should take the cached path:\n{out3}"
    );
    assert!(
        !built_from_source(&out3),
        "the listed check should read the image:\n{out3}"
    );
    assert_eq!(
        to_recheck(&out3),
        0,
        "an unchanged project after a cold build re-checked files:\n{out3}"
    );

    // A flag that changes what a walk does is part of the key: no replay under it.
    let (_, out4) = nest_check_with(&dir, &[], &[("BROOD_NO_IMAGE_SIGS", "1")]);
    assert!(
        !replayed(&out4) && loaded(&out4),
        "a walk flag must defeat the replay:\n{out4}"
    );
}

#[test]
fn an_edit_re_derives_the_edited_file_and_its_dependents_only() {
    let dir = scratch("edit");
    gen_project(&dir, 6);
    nest_check(&dir);
    let (_, warm) = nest_check(&dir);
    assert!(replayed(&warm), "not warm before the edit:\n{warm}");

    // A leaf nobody imports: itself alone. The edit makes the image stale, so this check
    // loads from SOURCE — the other direction of the flip the cache used to fall over.
    write(
        &dir,
        "src/leaf3.blsp",
        "(defmodule leaf3)\n\n(defn leaf3-f (x) (+ x 300))\n(defn leaf3-g (x) (leaf3-f (* x 2)))\n",
    );
    let (code, out) = nest_check(&dir);
    assert_eq!(code, 0, "leaf edit check failed:\n{out}");
    assert!(!replayed(&out), "an edit must not be replayed:\n{out}");
    assert!(
        built_from_source(&out),
        "an edit should rebuild the image:\n{out}"
    );
    assert_eq!(
        to_recheck(&out),
        1,
        "a leaf edit should re-check the leaf alone:\n{out}"
    );

    // A dependency whose ARITY changes: the dependent's verdict must move too — `app` calls
    // `(helper-scale 4)`, which is now an arity error — so both re-derive, and nothing else.
    write(
        &dir,
        "src/helper.blsp",
        "(defmodule helper)\n\n(defn helper-scale (x y) (* x y))\n",
    );
    let (code, out) = nest_check(&dir);
    assert!(
        !replayed(&out),
        "a dependency edit must not be replayed:\n{out}"
    );
    assert_eq!(
        to_recheck(&out),
        2,
        "a dependency edit should re-check it and its dependent:\n{out}"
    );
    assert_eq!(
        code, 1,
        "the dependent's new arity warning should fail the check:\n{out}"
    );
    assert!(
        out.contains("app.blsp") && out.contains("helper-scale"),
        "the dependent's warning names the call:\n{out}"
    );

    // And once more unchanged: the recorded verdict — warning, exit code and all — replays.
    let (code, out) = nest_check(&dir);
    assert!(
        replayed(&out) && !loaded(&out),
        "unchanged after the edits should replay:\n{out}"
    );
    assert_eq!(
        code, 1,
        "the replayed warning still fails the check:\n{out}"
    );
    assert!(
        out.contains("helper-scale"),
        "the reused verdict names the call:\n{out}"
    );
}

/// KI-186: a listed file outside the source and test trees (`nest check bin/x.blsp`) was
/// keyed by its PATH alone, so an edit to it replayed the previous verdict — here, a clean
/// one over a file that now calls an unbound function. Asserted on the exit code and the
/// warning, the two things a gate reads, not on the key.
#[test]
fn an_edit_to_a_listed_file_outside_the_source_paths_is_not_replayed() {
    let dir = scratch("listed");
    gen_project(&dir, 2);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    write(&dir, "bin/tool.blsp", "(defn tool () (+ 1 2))\n");
    let (code, out) = nest_check_with(&dir, &["bin/tool.blsp"], &[]);
    assert_eq!(code, 0, "the clean tool should check clean:\n{out}");
    let (_, warm) = nest_check_with(&dir, &["bin/tool.blsp"], &[]);
    assert!(
        replayed(&warm),
        "an unchanged listed file should replay:\n{warm}"
    );

    write(
        &dir,
        "bin/tool.blsp",
        "(defn tool () (no-such-function 1 2))\n",
    );
    let (code, out) = nest_check_with(&dir, &["bin/tool.blsp"], &[]);
    assert!(
        !replayed(&out),
        "an edited listed file must not be replayed:\n{out}"
    );
    assert_eq!(
        code, 1,
        "the new unbound call should fail the check:\n{out}"
    );
    assert!(
        out.contains("no-such-function"),
        "the warning names the unbound call:\n{out}"
    );
}
