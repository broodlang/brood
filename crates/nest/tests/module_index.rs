//! The project's file → modules index (ADR-380, `std/tool/module-index.blsp`): a warm
//! `nest run` must learn which file declares which module WITHOUT reading a source file.
//!
//! Before the index, every `nest` invocation re-parsed every source file — whole — to find
//! its `defmodule` header, twice (package rooting and the package-identity map). At 3 000
//! lines a file that was the entire warm start of a 1 000-file project (3.0 s, measured
//! 2026-09-21, `docs/large-project-scaling.md`), on a path whose contract is O(what the
//! entry point reaches). `strace` counted 2 000 `openat` of `src/mod*.blsp` on a warm run.
//!
//! `strace` is not on CI, so the observable here is the loader's own account: under
//! `BROOD_IMAGE_TRACE=1` each index query prints `[index] N files: H from the module index,
//! P parsed`, where P is the number of files whose text was read. The number is computed
//! whether or not the trace is on (KI-171: a trace must never be on the path of what it
//! traces) — the flag only prints it.
//!
//! Sabotage-verified: with the size/mtime comparison forced true the edit case reads
//! `P == 0` where two files changed (the stale entries are served — and the in-language
//! `tests/module_index_test.blsp` reads the OLD module name from them); with the index
//! bypassed (every file parsed) the warm case reads `P == N`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A project directory of its own, so cases can run concurrently under nextest.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-module-index-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

/// The shape of `scripts/bench/gen-project.py`: `n` modules of `fns` functions each, plus an
/// entry point `app` that reaches exactly two of them (`mod0` and `helper`).
fn gen_project(dir: &Path, n: usize, fns: usize) {
    write(dir, "project.blsp", "(project\n  :name big\n  :main app)\n");
    write(
        dir,
        "src/app.blsp",
        "(defmodule app (:use mod0) (:use helper))\n\n(defn main ()\n  (io/puts (str \"ANSWER: \" (+ (mod0-total 3) (helper-scale 4)))))\n",
    );
    write(
        dir,
        "src/helper.blsp",
        "(defmodule helper \"A small module the entry point reaches.\")\n\n(defn helper-scale (x) (* x 10))\n",
    );
    for i in 0..n {
        let mut s = format!("(defmodule mod{i} \"Generated module {i}.\")\n\n");
        for k in 0..fns {
            s.push_str(&format!(
                "(defn m{i}-f{k} (x)\n  (let (y (+ x {}) z (* y {}))\n    (cond (< z 0) 0 (= z {k}) (- z 1) else (+ z 1))))\n\n",
                k + 1,
                i % 7 + 2
            ));
        }
        s.push_str(&format!("(defn mod{i}-total (x)\n  (fold (list "));
        for k in 0..fns {
            s.push_str(&format!("m{i}-f{k} "));
        }
        s.push_str(")\n    0 (fn (acc f) (+ acc (f x)))))\n");
        write(dir, &format!("src/mod{i}.blsp"), &s);
    }
}

/// `nest run` in `dir` with the loader trace on; (stdout, stderr), asserting success.
fn nest_run(dir: &Path) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .env("BROOD_IMAGE_TRACE", "1")
        .current_dir(dir)
        .output()
        .expect("spawn nest");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "nest run failed: status={:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status
    );
    (stdout, stderr)
}

/// Every `[index] N files: H from the module index, P parsed` line as `(N, H, P)`.
fn index_lines(stderr: &str) -> Vec<(usize, usize, usize)> {
    stderr
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("[index] ")?;
            let nums: Vec<usize> = rest
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().unwrap())
                .collect();
            (nums.len() == 3).then(|| (nums[0], nums[1], nums[2]))
        })
        .collect()
}

fn answer(stdout: &str) -> String {
    stdout
        .lines()
        .find(|l| l.starts_with("ANSWER: "))
        .unwrap_or_else(|| panic!("no ANSWER line in:\n{stdout}"))
        .to_string()
}

/// The gate for `docs/large-project-scaling.md` Finding 1: a warm run parses no source file
/// to learn the module graph, and answers the same as the cold run that built the index.
#[test]
fn a_warm_run_learns_the_module_graph_without_parsing_a_source_file() {
    let dir = scratch("warm");
    gen_project(&dir, 12, 60);
    let files = 14; // 12 modules + app + helper

    let (out1, err1) = nest_run(&dir);
    let cold = index_lines(&err1);
    assert!(!cold.is_empty(), "no [index] line on the cold run:\n{err1}");
    // The first query of a fresh project reads every file…
    assert_eq!(
        cold[0],
        (files, 0, files),
        "cold run's first index query: {cold:?}"
    );
    // …and the second (the package-identity map, same files) already reads none.
    assert!(
        cold[1..].iter().all(|&(_, _, p)| p == 0),
        "cold run re-parsed within itself: {cold:?}"
    );
    assert!(
        dir.join(".brood/module-index").is_file(),
        "no index written"
    );

    let (out2, err2) = nest_run(&dir);
    let warm = index_lines(&err2);
    assert!(!warm.is_empty(), "no [index] line on the warm run:\n{err2}");
    for &(n, h, p) in &warm {
        assert_eq!(
            p, 0,
            "a warm run parsed {p} of {n} source files: {warm:?}\n{err2}"
        );
        assert_eq!(
            h, n,
            "a warm run answered {h} of {n} from the index: {warm:?}"
        );
    }
    assert_eq!(answer(&out1), answer(&out2));
}

/// The index is keyed per file, so an edit re-parses the edited files alone. The edit renames
/// a module AWAY from its filename (`helper2` in `helper.blsp`) so that the module graph the
/// index answers actually differs from what the filename convention would guess; the run
/// still succeeds either way here because a cold load evaluates every source file, so the
/// count is the observable — the module NAMES the index answers are pinned in
/// `tests/module_index_test.blsp`.
#[test]
fn an_edited_file_reindexes_itself_alone_and_a_renamed_module_is_found_through_the_index() {
    let dir = scratch("edit");
    gen_project(&dir, 6, 40);
    let files = 8;
    let (out1, _) = nest_run(&dir);
    let before = answer(&out1);

    write(
        &dir,
        "src/helper.blsp",
        "(defmodule helper2 \"Renamed: the module no longer matches its filename.\")\n\n(defn helper-scale (x) (* x 1000))\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use mod0) (:use helper2))\n\n(defn main ()\n  (io/puts (str \"ANSWER: \" (+ (mod0-total 3) (helper-scale 4)))))\n",
    );
    let (out2, err2) = nest_run(&dir);
    let edited = index_lines(&err2);
    assert_eq!(
        edited[0],
        (files, files - 2, 2),
        "the edit should re-parse two files: {edited:?}"
    );
    let after = answer(&out2);
    // `helper-scale 4` went from 40 to 4000; `mod0-total 3` is unchanged.
    let num = |a: &str| a.trim_start_matches("ANSWER: ").parse::<i64>().unwrap();
    assert_eq!(
        num(&after) - num(&before),
        3960,
        "before {before}, after {after}"
    );

    let (out3, err3) = nest_run(&dir);
    let warm = index_lines(&err3);
    assert!(
        warm.iter().all(|&(_, _, p)| p == 0),
        "warm run after the edit parsed: {warm:?}"
    );
    assert_eq!(after, answer(&out3));
}
