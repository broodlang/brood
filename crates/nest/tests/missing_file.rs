//! A named file that isn't readable must be reported the same way by every
//! subcommand that takes one.
//!
//! `nest test FILE` always did this properly (`nest test: cannot read x.blsp: …`).
//! `check` and `run` did not — they handed the path to Brood, which surfaced the
//! failure from whichever internal function read it first, so the user saw
//! `check-file-deps: cannot read …` plus a trace through `project-pfold-files`
//! for what is simply a mistyped filename. Same mistake, same message.
//!
//! Also pins the one path that must NOT be validated: `nest run <doc>` hands a
//! non-`.blsp` path to the entry point, and opening a file that does not exist yet
//! is the normal editor case.

use std::path::Path;
use std::process::Command;

struct TempDir {
    path: std::path::PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn project() -> TempDir {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-missing-{}-{n}", std::process::id()));
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::create_dir_all(path.join("tests")).unwrap();
    std::fs::write(
        path.join("project.blsp"),
        "(project :name \"mf\" :version \"0.1.0\" :source-paths [\"src\"] :test-paths [\"tests\"])\n",
    )
    .unwrap();
    std::fs::write(
        path.join("src/main.blsp"),
        "(defmodule main \"d\")\n\n(defn main () (io/print \"ran\"))\n",
    )
    .unwrap();
    TempDir { path }
}

fn nest(dir: &Path, args: &[&str]) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

#[test]
fn every_file_taking_subcommand_reports_a_missing_file_the_same_way() {
    let proj = project();
    for command in ["test", "check", "run"] {
        let (out, ok) = nest(&proj.path, &[command, "nosuchfile.blsp"]);
        assert!(!ok, "{command} should fail on a missing file:\n{out}");
        assert!(
            out.starts_with(&format!("nest {command}: cannot read nosuchfile.blsp")),
            "{command} should report the missing file at the CLI boundary, got:\n{out}"
        );
        // The tell-tale of the old behaviour: an internal function name or a Brood
        // stack frame surfacing for a mistyped filename.
        assert!(
            !out.contains("check-file") && !out.contains("    at "),
            "{command} leaked internals for a missing file:\n{out}"
        );
    }
}

#[test]
fn a_readable_file_is_still_accepted() {
    let proj = project();
    let (out, ok) = nest(&proj.path, &["check", "src/main.blsp"]);
    assert!(ok, "checking a real file should succeed:\n{out}");
    let (out, ok) = nest(&proj.path, &["run", "src/main.blsp"]);
    assert!(ok, "running a real file should succeed:\n{out}");
}

/// `nest run notes.txt` inside a project routes the path to `:main` as a document
/// argument rather than running it as Brood. That file legitimately need not exist —
/// creating a new file is the ordinary editor case — so it must not be rejected.
#[test]
fn a_document_argument_is_not_required_to_exist() {
    let proj = project();
    let (out, _) = nest(&proj.path, &["run", "notes-that-do-not-exist.txt"]);
    assert!(
        !out.contains("cannot read notes-that-do-not-exist.txt"),
        "a document argument must not be existence-checked:\n{out}"
    );
}

/// `nest observe` / `nest attach` draw a full-screen view. Piped or redirected, the
/// terminal primitives used to fail deep in the render loop —
/// `runtime error: terminal: No such device or address (os error 6)` with an
/// `at editor/ui/ui-run` frame — which is true and useless. They now say what is
/// actually wrong, before starting anything.
#[test]
fn tui_subcommands_explain_that_they_need_a_terminal() {
    let proj = project();
    for args in [vec!["observe"], vec!["attach", "somenode"]] {
        let (out, ok) = nest(&proj.path, &args);
        let command = args[0];
        assert!(!ok, "{command} should fail without a tty:\n{out}");
        assert!(
            out.contains("needs an interactive terminal"),
            "{command} should say it needs a terminal, got:\n{out}"
        );
        // The old symptom, and the internal frame that came with it.
        assert!(
            !out.contains("os error 6") && !out.contains("ui-run"),
            "{command} still leaks the raw terminal failure:\n{out}"
        );
        // And it should point at the way to run it under test.
        assert!(
            out.contains("script -qec"),
            "{command} should suggest a pty:\n{out}"
        );
    }
}
