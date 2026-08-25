//! `brood --check` must resolve a `(:use …)`-imported ability op that lives in a
//! *loose disk* module — one found on the load-path but not embedded in the binary
//! and not part of a project.
//!
//! The gap this guards (filed 2026-07-28 on the `show` protocol work): a `(:use greet)`
//! consumer calling `greet`'s ability op `to-line` *ran* fine — the header's
//! `(require 'greet)` loads the module and registers the op as a global — but the
//! advisory checker was reported to flag the bare `to-line` (and the record constructor
//! `person`) as `unbound symbol`. It was closed as a side effect of the subsequent
//! type/checker/resolver work (the checker evaluating the `(defmodule … (:use …))`
//! header so its binding view matches the runtime image; reading the live ability
//! registries, ADR-186; the KI-24 resolver hardening) rather than by one targeted fix.
//! This test pins the now-correct behavior: an imported ability op resolves, not unbound.
//!
//! The property asserted: checking the consumer emits **no `unbound symbol`** for the
//! imported op or the imported record constructor. The two files are written to a temp
//! dir and `--check`ed from inside it, so the loader finds `greet` on the default
//! load-path (`.`) — exactly the "loose disk, not a project" shape that failed.

use std::path::PathBuf;
use std::process::Command;

mod support;

/// A temp dir holding the two `.blsp` files, removed on drop.
struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn write_fixture() -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("brood-xmod-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // The provider: a loose-disk module defining an ability, a record, and an impl.
    std::fs::write(
        dir.join("greet.blsp"),
        "(defmodule greet)\n\
         (defability Greeter\n\
         \x20\x20(to-line [self] :-> string))\n\
         (defrecord person (name))\n\
         (impl Greeter person\n\
         \x20\x20(to-line [self] (str \"hi \" (person-name self))))\n",
    )
    .unwrap();

    // The consumer: imports the provider and calls its ability op on its record.
    std::fs::write(
        dir.join("main.blsp"),
        "(defmodule main (:use greet))\n\
         (defn go () (to-line (person \"ada\")))\n",
    )
    .unwrap();

    // A tiny runner that exercises the consumer at runtime — proves the op is a real,
    // callable global before we assert the checker agrees.
    std::fs::write(
        dir.join("run.blsp"),
        "(defmodule runner (:use main))\n(io/puts (go))\n",
    )
    .unwrap();

    TempDir { path: dir }
}

/// Run `brood <args>` from inside `dir` (so the default load-path `.` finds `greet`)
/// and return combined stdout+stderr plus success.
fn run_brood(dir: &std::path::Path, args: &[&str]) -> (String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.args(args).current_dir(dir);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

#[test]
fn check_resolves_a_used_ability_op_from_a_loose_disk_module() {
    let dir = write_fixture();

    // Sanity: the program actually runs — the op is a real, callable global. If this
    // failed, "runs but checker flags it" would be untestable (both would be broken).
    let (run_text, run_ok) = run_brood(&dir.path, &["run.blsp"]);
    assert!(
        run_ok && run_text.contains("hi ada"),
        "the loose-disk ability op should run:\n{run_text}"
    );

    // The property under test: checking the consumer flags no unbound symbol for the
    // imported op or the imported record constructor.
    let (check_text, check_ok) = run_brood(&dir.path, &["--check", "main.blsp"]);
    assert!(
        check_ok,
        "`brood --check` should exit cleanly:\n{check_text}"
    );
    assert!(
        !check_text.contains("unbound symbol: to-line"),
        "the imported ability op `to-line` was flagged unbound by the checker:\n{check_text}"
    );
    assert!(
        !check_text.contains("unbound symbol: person"),
        "the imported record constructor `person` was flagged unbound by the checker:\n{check_text}"
    );
}
