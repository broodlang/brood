//! `nest check-syntax` and `nest form` on disk (ADR-789): the reader alone over files or a
//! Claude Code hook payload, and edits by whole top-level form. What the text-level tests in
//! `tests/form_edit_test.blsp` cannot see is the CLI: exit codes, stdin, stdout against
//! stderr, and that a refusal leaves the file as it was.

mod support;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use support::temp_dir;

/// Run `nest ARGS…` in `dir` with `input` on stdin.
fn nest_with_stdin(dir: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run nest");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for nest")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

const MISSING_CLOSER: &str = "(defn a ()\n  (let (x 1)\n    (foo x))\n\n(defn b ()\n  2)\n";
const SHAPES: &str =
    "(defmodule shapes)\n\n(defn area (w h)\n  (* w h))\n\n(defn perimeter (w h) (* 2 (+ w h)))\n";

#[test]
fn check_syntax_is_silent_on_a_clean_file_and_says_where_on_a_broken_one() {
    let dir = temp_dir("check-syntax");
    std::fs::write(dir.path.join("good.blsp"), SHAPES).unwrap();
    std::fs::write(dir.path.join("bad.blsp"), MISSING_CLOSER).unwrap();

    let clean = nest_with_stdin(&dir.path, &["check-syntax", "good.blsp"], "");
    assert_eq!(clean.status.code(), Some(0), "{clean:?}");
    assert_eq!(stdout(&clean), "");

    let broken = nest_with_stdin(&dir.path, &["check-syntax", "good.blsp", "bad.blsp"], "");
    assert_eq!(broken.status.code(), Some(1), "{broken:?}");
    assert!(
        stdout(&broken).starts_with(
            "bad.blsp:1:1: unclosed list: `(defn a` opened at 1:1 is still open when line 5 \
             starts `(defn b` at column 1 — a `)` is missing before line 5\n    (defn a ()\n    ^"
        ),
        "{}",
        stdout(&broken)
    );
}

#[test]
fn the_claude_hook_reports_a_broken_blsp_with_exit_2_and_passes_everything_else() {
    let dir = temp_dir("claude-hook");
    std::fs::write(dir.path.join("good.blsp"), SHAPES).unwrap();
    std::fs::write(dir.path.join("bad.blsp"), MISSING_CLOSER).unwrap();
    std::fs::write(dir.path.join("notes.md"), "(unbalanced").unwrap();
    let payload = |file: &str| {
        format!(
            "{{\"tool_name\": \"Edit\", \"tool_input\": {{\"file_path\": \"{}\"}}}}",
            dir.path.join(file).display()
        )
    };
    let hook = ["check-syntax", "--claude-hook"];

    let broken = nest_with_stdin(&dir.path, &hook, &payload("bad.blsp"));
    assert_eq!(broken.status.code(), Some(2), "{broken:?}");
    assert!(
        stderr(&broken).contains("a `)` is missing before line 5"),
        "{broken:?}"
    );
    assert!(stderr(&broken).contains("nest form replace"), "{broken:?}");

    for quiet in [
        payload("good.blsp"),
        payload("notes.md"),
        "not json".to_string(),
    ] {
        let output = nest_with_stdin(&dir.path, &hook, &quiet);
        assert_eq!(output.status.code(), Some(0), "{quiet}: {output:?}");
        assert_eq!(stderr(&output), "", "{quiet}");
    }

    // A relative path is the payload's `cwd`'s.
    let relative = format!(
        "{{\"cwd\": \"{}\", \"tool_input\": {{\"file_path\": \"bad.blsp\"}}}}",
        dir.path.display()
    );
    let output = nest_with_stdin(Path::new("/"), &hook, &relative);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn nest_form_lists_reads_replaces_inserts_and_deletes_whole_forms() {
    let dir = temp_dir("nest-form");
    let file = dir.path.join("shapes.blsp");
    std::fs::write(&file, SHAPES).unwrap();
    let run = |args: &[&str], input: &str| {
        let mut words = vec!["form"];
        words.extend_from_slice(args);
        nest_with_stdin(&dir.path, &words, input)
    };

    let listed = run(&["list", "shapes.blsp"], "");
    assert_eq!(
        stdout(&listed),
        "1-1 defmodule shapes\n3-4 defn area\n6-6 defn perimeter\n"
    );
    let read = run(&["read", "shapes.blsp", "area"], "");
    assert_eq!(stdout(&read), "(defn area (w h)\n  (* w h))\n");

    let replaced = run(
        &["replace", "shapes.blsp", "area"],
        "(defn area (w h)   (* w h 2))\n",
    );
    assert!(replaced.status.success(), "{replaced:?}");
    let inserted = run(
        &["insert", "shapes.blsp", "--after", "area"],
        "(defn square (side) (area side side))",
    );
    assert!(inserted.status.success(), "{inserted:?}");
    let deleted = run(&["delete", "shapes.blsp", "perimeter"], "");
    assert!(deleted.status.success(), "{deleted:?}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "(defmodule shapes)\n\n(defn area (w h) (* w h 2))\n\n(defn square (side) (area side side))\n"
    );
}

#[test]
fn a_refused_form_edit_writes_nothing_and_exits_1() {
    let dir = temp_dir("nest-form-refused");
    let file = dir.path.join("shapes.blsp");
    std::fs::write(&file, SHAPES).unwrap();

    let unbalanced = nest_with_stdin(
        &dir.path,
        &["form", "replace", "shapes.blsp", "area"],
        "(defn area (w h)\n  (* w h)",
    );
    assert_eq!(unbalanced.status.code(), Some(1), "{unbalanced:?}");
    assert!(
        stderr(&unbalanced).contains("the new form:1:1: unclosed list"),
        "{unbalanced:?}"
    );
    let missing = nest_with_stdin(&dir.path, &["form", "read", "shapes.blsp", "volume"], "");
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(stderr(&missing).contains("3-4 defn area"), "{missing:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), SHAPES);
}
