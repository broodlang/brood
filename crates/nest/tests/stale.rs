//! `nest test --stale` end to end.
//!
//! `--stale` skips a test file whose transitive dependencies are unchanged since it last
//! ran, and re-runs it when it — or a source file it requires — changes. That is a
//! cross-invocation property (the record lives in the project cache dir), so it can only
//! be pinned from separate processes: an integration test.

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

fn tempdir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("brood-stale-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::create_dir_all(path.join("tests")).unwrap();
    TempDir { path }
}

/// A project with one source module `cov` and one test file that `(:use cov)` — so the
/// test's require-closure includes `cov`, and a change to `src/cov.blsp` makes the test
/// file stale.
fn fixture(tag: &str) -> TempDir {
    let dir = tempdir(tag);
    let root = &dir.path;
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"cov\" :version \"0.1.0\" \
         :source-paths [\"src\"] :test-paths [\"tests\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/cov.blsp"),
        "(defmodule cov)\n\n(defn live (x) (+ x 1))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/cov_test.blsp"),
        "(defmodule cov-test (:use test) (:use cov))\n\n\
         (describe \"cov\"\n  (test \"live\" (assert= (live 1) 2)))\n",
    )
    .unwrap();
    dir
}

struct Out {
    text: String,
    ok: bool,
}

fn nest(dir: &Path, args: &[&str]) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

#[test]
fn stale_skips_unchanged_then_reruns_on_a_source_change() {
    let dir = fixture("cycle");

    // Cold: no record yet, so the test runs.
    let r1 = nest(&dir.path, &["test", "--stale"]);
    assert!(r1.ok, "cold --stale run should pass:\n{}", r1.text);
    assert!(
        r1.text.contains("1 tests, 1 passed"),
        "a cold --stale run runs the test:\n{}",
        r1.text
    );

    // Nothing changed: the test file is skipped.
    let r2 = nest(&dir.path, &["test", "--stale"]);
    assert!(r2.ok, "warm --stale run should pass:\n{}", r2.text);
    assert!(
        r2.text.contains("unchanged, skipped"),
        "a second --stale run skips the unchanged file:\n{}",
        r2.text
    );
    assert!(
        r2.text.contains("0 tests"),
        "nothing should run when nothing changed:\n{}",
        r2.text
    );

    // Change the SOURCE the test transitively requires (not the test file itself). The
    // sleep guarantees a newer mtime regardless of filesystem timestamp resolution.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(
        dir.path.join("src/cov.blsp"),
        "(defmodule cov)\n\n(defn live (x) (+ x 2))\n",
    )
    .unwrap();
    // The test now expects the OLD behaviour, so it should FAIL — proving it actually ran.
    let r3 = nest(&dir.path, &["test", "--stale"]);
    assert!(
        !r3.ok,
        "changing a dependency must re-run the test (which now fails):\n{}",
        r3.text
    );
    assert!(
        r3.text.contains("1 tests") && !r3.text.contains("unchanged, skipped"),
        "a source change re-runs the dependent test:\n{}",
        r3.text
    );
}

#[test]
fn a_plain_run_ignores_the_stale_record() {
    let dir = fixture("plain");
    // Establish a record so everything would be "unchanged"…
    let _ = nest(&dir.path, &["test", "--stale"]);
    // …then a run WITHOUT --stale must still run every test and print no stale note.
    let plain = nest(&dir.path, &["test"]);
    assert!(
        plain.text.contains("1 tests, 1 passed"),
        "a plain run always runs the suite:\n{}",
        plain.text
    );
    assert!(
        !plain.text.contains("unchanged, skipped"),
        "a plain run must not mention staleness:\n{}",
        plain.text
    );
}
