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

// ── `test` (moved 2026-09-05) ──────────────────────────────────────────────────────────
//
// The suite-level behaviour (`--stale`, `--cover-lines`, `--trace`, the missing-file
// boundary, completion of selectors and test files) is pinned by `stale.rs`,
// `coverage_lines.rs`, `file_boundary_quiesce.rs`, `missing_file.rs` and `complete.rs`,
// which now run through the Brood arm. Pinned here: the typed flags, the shard guard, the
// FILE:LINE selector, the silent exit 1 on a failing suite, and the global `-j`.

fn scaffolded(tag: &str) -> std::path::PathBuf {
    let dir = scratch(tag);
    let (code, _, err) = nest_in(&dir, &["new", "demo"]);
    assert_eq!(code, 0, "scaffold:\n{err}");
    dir.join("demo")
}

#[test]
fn test_runs_the_scaffolded_suite_and_a_named_file_with_a_line_selector() {
    let proj = scaffolded("test-proj");
    let (code, out, err) = nest_in(&proj, &["test"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("tests, ") && out.contains(" passed"),
        "a summary line:\n{out}"
    );
    let (code, out, err) = nest_in(&proj, &["-j", "2", "test", "tests/main_test.blsp"]);
    assert_eq!(code, 0, "named file (with the global -j):\n{out}\n{err}");
    assert!(out.contains(" passed"), "{out}");
    // A `FILE:LINE` selector narrows to the test at that line; a line with no test
    // selects nothing, which the runner reports as zero tests rather than an error.
    let (code, out, err) = nest_in(&proj, &["test", "tests/main_test.blsp:1"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("0 tests"), "line 1 holds no test:\n{out}");
    let (code, out, _) = nest_in(&proj, &["test", "--formatter", "tap"]);
    assert_eq!(code, 0);
    assert!(out.contains("TAP version 13"), "{out}");
}

#[test]
fn a_failing_suite_exits_1_without_reporting_the_runner_internals() {
    let proj = scaffolded("test-fail");
    std::fs::write(
        proj.join("tests").join("bad_test.blsp"),
        "(defmodule demo/bad-test (:use test))\n(describe \"bad\"\n  (test \"lies\" (assert= 1 2)))\n",
    )
    .expect("write bad test");
    let (code, out, err) = nest_in(&proj, &["test"]);
    assert_eq!(code, 1, "{out}\n{err}");
    assert!(
        out.contains("1 failed") || err.contains("1 failed"),
        "{out}\n{err}"
    );
    let all = format!("{out}{err}");
    assert!(
        !all.contains("at project/run-tests") && !all.contains("test(s) failed\n    at "),
        "the failure signal must not be reported as an error:\n{all}"
    );
}

#[test]
fn test_typed_flags_and_the_shard_guard_are_usage_errors() {
    let proj = scaffolded("test-flags");
    for (args, needle) in [
        (
            &["test", "--max-failures", "0"][..],
            "invalid value '0' for '--max-failures <N>'",
        ),
        (
            &["test", "--seed", "abc"][..],
            "invalid value 'abc' for '--seed <N>'",
        ),
        (&["test", "--cover-min", "101"][..], "101 is not in 0..=100"),
        (
            &["test", "--shard", "1"][..],
            "--shard 1 needs --partitions N",
        ),
        (
            &["test", "--partitions", "2", "--shard", "5"][..],
            "out of range for --partitions 2",
        ),
        (&["test", "--nope"][..], "unexpected argument '--nope'"),
    ] {
        let (code, out, err) = nest_in(&proj, args);
        assert_eq!(code, 2, "{args:?}:\n{out}\n{err}");
        assert!(
            err.contains(needle),
            "{args:?}: expected {needle:?} in:\n{err}"
        );
        assert!(!out.contains("tests,"), "{args:?} must run nothing:\n{out}");
    }
    let (code, out, _) = nest_in(&proj, &["test", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest test [OPTIONS] [FILE]...") && out.contains("--only <SELECTOR>"),
        "{out}"
    );
}

// ── `run` (moved 2026-09-05) ───────────────────────────────────────────────────────────
//
// `run_main.rs` (`--main`, the entry override), `cli_failure_reporting.rs` (`--for` with a
// crashing/finishing/spinning program and the exit codes), `boot_check_and_renames.rs`
// (`--check-boot`, the ADR-304 launch gate, `--no-check`), `missing_file.rs` (the
// boundary guards and the document argument) and `complete.rs` (`--main` offers modules)
// now all run through the Brood arm. Pinned here: the trailing arguments, a document
// argument leading them, the clap constraints, and a bad `--for`.

#[test]
fn run_hands_trailing_words_to_the_entry_point_hyphens_included() {
    let proj = scaffolded("run-args");
    std::fs::write(
        proj.join("src").join("main.blsp"),
        "(defmodule main)\n(defn main (& args) (io/puts (str \"ARGS \" args)))\n",
    )
    .expect("write main");
    let (code, out, err) = nest_in(&proj, &["run", "notes.txt", "--verbose", "-x", "7"]);
    assert_eq!(code, 0, "{out}\n{err}");
    // The document leads; everything after the first positional is the program's.
    assert!(
        out.contains("ARGS (notes.txt --verbose -x 7)"),
        "trailing words:\n{out}"
    );
    let (code, out, err) = nest_in(&proj, &["run"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("ARGS nil"),
        "no trailing words: the empty list prints as nil:\n{out}"
    );
}

#[test]
fn run_constraints_and_a_bad_duration_are_usage_errors() {
    let proj = scaffolded("run-constraints");
    let (code, _, err) = nest_in(&proj, &["run", "--check-boot", "--no-check"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("'--check-boot' cannot be used with '--no-check'"),
        "{err}"
    );
    let (code, _, err) = nest_in(&proj, &["run", "--check-boot", "src/main.blsp"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("'--check-boot' cannot be used with '[FILE]...'"),
        "{err}"
    );
    let (code, _, err) = nest_in(&proj, &["run", "--for", "2x", "src/main.blsp"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("invalid --for duration '2x'"), "{err}");
    let (code, out, _) = nest_in(&proj, &["run", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest run [OPTIONS] [FILE]...") && out.contains("--for <DURATION>"),
        "{out}"
    );
}

// ── `new`, `update-tooling`, `stdimage`, `rename` (moved 2026-09-05) ───────────────────
//
// `scaffold_quality.rs` exercises `new` (every template), `update_tooling.rs` the tooling
// refresh in and outside a project, `complete.rs` the template completion. Pinned here: the
// fixed-arity positionals in clap's words, `stdimage`'s report, and `rename` end to end.

#[test]
fn fixed_arity_positionals_are_required_and_bounded() {
    let dir = scratch("arity");
    let (code, _, err) = nest_in(&dir, &["new"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("required arguments were not provided") && err.contains("<NAME>"),
        "{err}"
    );
    let (code, _, err) = nest_in(&dir, &["rename", "only-old"]);
    assert_eq!(code, 2, "{err}");
    // The missing list names only NEW; OLD appears in the usage line beneath, so check the
    // list's own lines.
    assert!(
        err.contains("not provided:\n  <NEW>\n") && !err.contains("  <OLD>\n"),
        "only the missing one:\n{err}"
    );
    let (code, _, err) = nest_in(&dir, &["rename", "a", "b", "c"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("unexpected argument 'c'"), "{err}");
    let (code, out, _) = nest_in(&dir, &["rename", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest rename [OPTIONS] <OLD> <NEW>"),
        "{out}"
    );
    let (code, _, err) = nest_in(&dir, &["rename", "--swap", "--refs-only", "a", "b"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn stdimage_reports_what_it_built() {
    let dir = scratch("stdimage");
    let (code, out, err) = nest_in(&dir, &["stdimage"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("bindings ->") || out.contains("no cache directory"),
        "{out}"
    );
}

#[test]
fn rename_rewrites_references_and_definition_in_a_project() {
    let proj = scaffolded("rename");
    let (code, out, err) = nest_in(&proj, &["rename", "hello", "greet"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let demo = std::fs::read_to_string(proj.join("src").join("demo.blsp")).expect("demo");
    let main = std::fs::read_to_string(proj.join("src").join("main.blsp")).expect("main");
    assert!(demo.contains("(defn greet"), "the definition:\n{demo}");
    assert!(
        !main.contains("(hello") && main.contains("(greet"),
        "the reference:\n{main}"
    );
    let (code, out, err) = nest_in(&proj, &["test"]);
    assert_eq!(code, 0, "the renamed project still passes:\n{out}\n{err}");
}

/// KI-112. `nest stdimage` is dispatched from `std/tool/nest.blsp`, whose load pulls the
/// toolchain in before the build runs. The build attributed a module's ROOT globals by
/// loading it and diffing, and `require-one` is a no-op for a module already loaded — so
/// `project`'s 31 root globals went unclaimed and the image restored `project` without
/// `*ns-package*`. This drives the real entry point with a private cache directory: build
/// the image through the dispatcher, then run the command that first showed the hole.
#[test]
fn an_image_built_through_the_dispatcher_restores_every_root_global() {
    let dir = scratch("stdimage-cache");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).expect("cache dir");
    let with_cache = |cwd: &std::path::Path, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_nest"))
            .current_dir(cwd)
            .env_remove("BROOD_NO_STDIMAGE")
            .env("XDG_CACHE_HOME", &cache)
            .args(args)
            .output()
            .expect("run nest");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, out, err) = with_cache(&dir, &["stdimage"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("bindings ->"), "{out}");
    let (code, _, err) = nest_in(&dir, &["new", "demo"]);
    assert_eq!(code, 0, "scaffold:\n{err}");
    // `check` loads `project` FROM THE IMAGE and reaches `record-ns-packages`, which reads
    // the root dynamic `*ns-package*` — unbound, before the fix.
    let (code, out, err) = with_cache(&dir.join("demo"), &["check"]);
    assert_eq!(
        code, 0,
        "check against the dispatcher-built image:\n{out}\n{err}"
    );
    assert!(
        !err.contains("unbound symbol"),
        "a root global was dropped from the image:\n{err}"
    );
}

// ── the package manager: `fetch`/`update`/`tree`/`add`/`remove`/`publish`/`search`/`key`/`ws`
// (moved 2026-09-05). `manifest_race.rs` and `scaffold_quality.rs` drive `add`/`remove`/`tree`
// against real sibling projects and `complete.rs` the dependency-name completion. Pinned
// here: the project guard on every project command, the arity ranges, and `tree`.

#[test]
fn every_package_command_needs_a_project_and_says_so() {
    let dir = scratch("pkg-guard");
    for args in [
        &["fetch"][..],
        &["update"][..],
        &["tree"][..],
        &["add", "x", ":path", "../x"][..],
        &["remove", "x"][..],
        &["publish"][..],
        &["search", "json"][..],
    ] {
        let (code, _, err) = nest_in(&dir, args);
        assert_eq!(code, 2, "{args:?}:\n{err}");
        assert!(
            err.contains(&format!("nest {}: no project.blsp in", args[0])),
            "{args:?}:\n{err}"
        );
    }
}

#[test]
fn package_arity_ranges_are_clap_shaped() {
    let dir = scratch("pkg-arity");
    let (code, _, err) = nest_in(&dir, &["search"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("not provided:\n  <QUERY>\n"), "{err}");
    let (code, _, err) = nest_in(&dir, &["search", "a", "b", "c"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("unexpected argument 'c'"), "{err}");
    let (code, out, _) = nest_in(&dir, &["search", "--help"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("Usage: nest search [OPTIONS] <QUERY> [INDEX]"),
        "{out}"
    );
    let (code, out, _) = nest_in(&dir, &["add", "--help"]);
    assert_eq!(code, 0);
    assert!(out.contains("Usage: nest add <NAME> [SPEC]..."), "{out}");
    let (code, _, err) = nest_in(&dir, &["key", "bogus"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("invalid value 'bogus' for <ACTION>") && err.contains("gen"),
        "{err}"
    );
    let (code, _, err) = nest_in(&dir, &["ws"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("<ACTION>"), "{err}");
    let (code, _, err) = nest_in(&dir, &["publish", "--bump"]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("a value is required for '--bump <LEVEL>'"),
        "{err}"
    );
}

#[test]
fn tree_prints_the_scaffolded_project() {
    let proj = scaffolded("pkg-tree");
    let (code, out, err) = nest_in(&proj, &["tree"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("demo"), "{out}");
    let (_, out, _) = nest_in(&proj, &["complete", "--", "remove", ""]);
    // No declared dependencies, so nothing is offered — and nothing fails.
    assert!(out.trim().is_empty(), "{out}");
}

// ── `repl` (moved 2026-09-05). Piped stdin keeps the plain `read-line` path, so the loop is
// testable end to end: a form in, its value out, and the project bootstrap message on stderr.

fn nest_repl(dir: &std::path::Path, input: &str) -> (i32, String, String) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .env("BROOD_NO_STDIMAGE", "1")
        .arg("repl")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn nest repl");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn repl_evaluates_piped_forms_outside_and_inside_a_project() {
    let dir = scratch("repl");
    let (code, out, err) = nest_repl(&dir, "(+ 1 2)\n");
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains('3'), "the value:\n{out}");
    assert!(err.contains("plain REPL"), "{err}");
    let proj = scaffolded("repl-proj");
    // Inside the project the prompt starts in `main`, so its `:use`d `demo/hello` is
    // reachable bare through the module's own imports.
    let (code, out, err) = nest_repl(&proj, "(+ 40 2)\n(hello)\n");
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(out.contains("42"), "{out}");
    assert!(err.contains("project sources loaded"), "{err}");
    assert!(
        !out.contains("unbound"),
        "a bare project name resolves at the prompt:\n{out}"
    );
}
