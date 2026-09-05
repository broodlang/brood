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
