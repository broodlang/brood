//! **The subcommands implemented in Brood behave as the Rust arms they replaced did.**
//!
//! ADR-322 moves `nest`'s dispatch into `std/tool/nest.blsp` one subcommand at a time;
//! `main.rs` routes a name in `BLSP_SUBCOMMANDS` there before clap runs. These cases drive
//! the real `nest` binary — the entry point a user reaches — through the seam: argv in, exit
//! code and output out. Each asserts PRESENCE of the expected output, never only the absence
//! of an error (an empty stdout "agrees" with anything).

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

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nest-blsp-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn grammar_emits_the_requested_target_and_rejects_an_unknown_one() {
    let dir = scratch("grammar");
    let (code, out, _) = nest_in(&dir, &["grammar", "emacs"]);
    assert_eq!(code, 0);
    assert!(
        out.starts_with("(defconst brood-special-forms"),
        "emacs grammar:\n{out}"
    );
    let (code, out, _) = nest_in(&dir, &["grammar"]);
    assert_eq!(code, 0);
    assert!(
        out.trim_start().starts_with('{'),
        "default (tmlanguage) grammar is JSON:\n{out}"
    );
    let (code, _, err) = nest_in(&dir, &["grammar", "bogus"]);
    assert_eq!(
        code, 2,
        "an unknown target is a usage error (clap's exit 2)"
    );
    assert!(
        err.contains("invalid value 'bogus'") && err.contains("tmlanguage, emacs, tree-sitter"),
        "{err}"
    );
}

#[test]
fn doc_all_works_anywhere_but_the_project_form_needs_a_project() {
    let dir = scratch("doc");
    let (code, out, _) = nest_in(&dir, &["doc", "--all"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("# Brood") && out.contains("io/puts"),
        "the global reference:\n{}",
        &out[..out.len().min(300)]
    );
    let (code, _, err) = nest_in(&dir, &["doc"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("nest doc: no project.blsp in") && err.contains("nest doc --all"),
        "{err}"
    );
    let (code, out, _) = nest_in(&dir, &["doc", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest doc") && out.contains("--all"),
        "{out}"
    );
}

#[test]
fn unknown_flags_are_usage_errors_with_clap_shaped_messages() {
    let dir = scratch("flags");
    let (code, _, err) = nest_in(&dir, &["grammar", "--nope"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("unexpected argument '--nope'") && err.contains("--help"),
        "{err}"
    );
    let (code, _, err) = nest_in(&dir, &["docs", "--out"]);
    assert_eq!(code, 2);
    assert!(
        err.contains("a value is required for '--out <DIR>'"),
        "{err}"
    );
}

#[test]
fn format_and_doctest_run_inside_a_scaffolded_project() {
    let dir = scratch("proj");
    let (code, _, err) = nest_in(&dir, &["new", "demo"]);
    assert_eq!(code, 0, "scaffold:\n{err}");
    let proj = dir.join("demo");
    let (code, out, err) = nest_in(&proj, &["format", "--check"]);
    assert_eq!(code, 0, "a fresh scaffold is formatted:\n{out}\n{err}");
    assert!(out.contains("all clean"), "format --check reports:\n{out}");
    // A docstring example that lies (`expr ;=> expected` is the doctest form): doctest
    // must exit 1 and name it.
    std::fs::write(
        proj.join("src").join("lies.blsp"),
        "(defmodule demo/lies)\n(defn two ()\n  \"Two.\n\n    (two) ;=> 3\"\n  2)\n",
    )
    .expect("write lies");
    let (code, out, err) = nest_in(&proj, &["doctest"]);
    assert_eq!(code, 1, "a failing example exits 1:\n{out}\n{err}");
    assert!(
        out.contains("(two)") || err.contains("(two)"),
        "the report names the example:\n{out}\n{err}"
    );
}

#[test]
fn completion_knows_the_brood_implemented_subcommands() {
    let dir = scratch("complete");
    let (_, out, _) = nest_in(&dir, &["complete", "--", ""]);
    for name in ["doc", "docs", "doctest", "grammar", "format"] {
        assert!(out.lines().any(|l| l == name), "missing {name} in:\n{out}");
    }
    let (_, out, _) = nest_in(&dir, &["complete", "--", "format", "--c"]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.contains(&"--check") && lines.contains(&"--changed"),
        "{lines:?}"
    );
    let (_, out, _) = nest_in(&dir, &["complete", "--", "grammar", "tr"]);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["tree-sitter"]);
}

// ── `check` (moved 2026-09-05) ─────────────────────────────────────────────────────────
//
// `missing_file.rs` and `cli_failure_reporting.rs` already drive the boundary guards
// (missing file, directory) and `complete.rs` the `.blsp` positional; `check_cache_mode.rs`
// the strict/plain cache split. What is pinned here is the rest of the surface the Rust arm
// had: the clap constraints, the project guard, the exit codes and the global `-j`.

#[test]
fn check_needs_a_project_or_files_and_says_which() {
    let dir = scratch("check-guard");
    let (code, _, err) = nest_in(&dir, &["check"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("nest check: no project.blsp in") && err.contains("nest check <file>.blsp"),
        "{err}"
    );
    let (code, out, _) = nest_in(&dir, &["check", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest check [OPTIONS] [FILE]...") && out.contains("--fix-renames"),
        "{out}"
    );
}

#[test]
fn check_constraints_are_clap_shaped_usage_errors() {
    let dir = scratch("check-constraints");
    let (code, _, err) = nest_in(&dir, &["check", "--dry-run"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("required arguments were not provided") && err.contains("--fix-renames"),
        "{err}"
    );
    let (code, _, err) = nest_in(&dir, &["check", "--suggest-sigs", "--fix-renames"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("'--suggest-sigs' cannot be used with '--fix-renames'"),
        "{err}"
    );
    let (code, _, err) = nest_in(&dir, &["check", "--fix-renames", "a.blsp"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("'--fix-renames' cannot be used with '[FILE]...'"),
        "{err}"
    );
}

#[test]
fn check_exit_code_is_the_warning_verdict_for_both_forms() {
    let dir = scratch("check-proj");
    let (code, _, err) = nest_in(&dir, &["new", "demo"]);
    assert_eq!(code, 0, "scaffold:\n{err}");
    let proj = dir.join("demo");
    let (code, out, err) = nest_in(&proj, &["check"]);
    assert_eq!(code, 0, "a fresh scaffold is clean:\n{out}\n{err}");
    let (code, _, err) = nest_in(&proj, &["check", "--strict", "src/main.blsp"]);
    assert_eq!(code, 0, "strict, one file:\n{err}");
    std::fs::write(
        proj.join("src").join("bad.blsp"),
        "(defmodule demo/bad)\n(defn f ()\n  (no-such-function 1))\n",
    )
    .expect("write bad");
    let (code, _, err) = nest_in(&proj, &["check"]);
    assert_eq!(code, 1, "an unbound symbol is a warning, exit 1:\n{err}");
    assert!(
        err.contains("warning:") && err.contains("no-such-function"),
        "the warning names the symbol:\n{err}"
    );
    // A FILE list is variadic, and the verdict covers every file named.
    let (code, _, err) = nest_in(&proj, &["check", "src/main.blsp", "src/bad.blsp"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no-such-function"), "{err}");
    let (code, _, err) = nest_in(&proj, &["check", "src/main.blsp"]);
    assert_eq!(code, 0, "the clean file alone is clean:\n{err}");
}

#[test]
fn the_global_jobs_option_still_reaches_a_brood_subcommand() {
    let dir = scratch("check-jobs");
    for args in [
        &["-j", "2", "check", "--help"][..],
        &["--max-parallel", "2", "check", "--help"][..],
        &["--jobs=2", "check", "--help"][..],
        &["-j2", "check", "--help"][..],
    ] {
        let (code, out, err) = nest_in(&dir, args);
        assert_eq!(code, 0, "{args:?}:\n{err}");
        assert!(out.contains("Usage: nest check"), "{args:?}:\n{out}");
    }
    let (_, out, _) = nest_in(&dir, &["complete", "--", "check", "--fix"]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.contains(&"--fix-renames") && lines.contains(&"--fix-sigs"),
        "{lines:?}"
    );
}
