//! What a newer Brood's files do to this one (road-to-1.0 §B1, §B2; ADR-673 §3).
//!
//! - A `project.blsp` key this build does not know WARNS and the project still loads: a later
//!   minor may add keys, and a manifest written for it must work here. Said once per command.
//! - A lock of a newer format is REFUSED, naming both formats, never treated as absent — which
//!   would re-resolve and overwrite the newer Brood's lock. A lock with no `:version` is
//!   format 1 and still reads. A file that is not a `(lock …)` form is refused too.

mod support;
use std::path::Path;
use support::{TempDir, nest, temp_dir};

/// An app with one `:path` dependency (so a command reads the lock) and an unknown key.
fn project() -> TempDir {
    let temp = temp_dir("lockfmt");
    let root = &temp.path;
    std::fs::create_dir_all(root.join("dep/src")).unwrap();
    std::fs::write(
        root.join("dep/project.blsp"),
        "(project :name \"dep\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("dep/src/dep.blsp"),
        "(defmodule dep \"d\")\n(defn hello () 1)\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::write(
        root.join("app/project.blsp"),
        "(project :name \"app\" :version \"0.1.0\" :source-paths [\"src\"] \
         :frobnicate 1 :dependencies [[dep :path \"../dep\"]])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/src/main.blsp"),
        "(defmodule main \"d\" (:use dep))\n(defn main () (hello))\n",
    )
    .unwrap();
    temp
}

fn lock(app: &Path) -> String {
    std::fs::read_to_string(app.join("project.lock.blsp")).unwrap()
}

#[test]
fn an_unknown_manifest_key_warns_once_and_the_project_still_loads() {
    let proj = project();
    let app = proj.path.join("app");
    let (output, passed) = nest(&app, &["check"]);
    assert!(passed, "an unknown key must not fail:\n{output}");
    assert_eq!(
        output.matches("unknown key :frobnicate").count(),
        1,
        "said exactly once:\n{output}"
    );
}

#[test]
fn a_newer_lock_format_is_refused_and_an_unversioned_one_reads() {
    let proj = project();
    let app = proj.path.join("app");
    let (output, passed) = nest(&app, &["fetch"]);
    assert!(passed, "{output}");
    let written = lock(&app);
    assert!(written.contains(":version 1"), "{written}");

    // Written by a newer Brood: refused, by both a reader and a writer, and left alone.
    let newer = written.replace(":version 1", ":version 99");
    std::fs::write(app.join("project.lock.blsp"), &newer).unwrap();
    for command in ["check", "fetch"] {
        let (output, passed) = nest(&app, &[command]);
        assert!(
            !passed && output.contains("lock format 99") && output.contains("formats 1 to 1"),
            "`nest {command}` must refuse a newer lock, naming both formats:\n{output}"
        );
        assert_eq!(lock(&app), newer, "`nest {command}` must not rewrite it");
    }

    // A lock from before the version was written reads as format 1.
    std::fs::write(
        app.join("project.lock.blsp"),
        written.replace(":version 1 ", ""),
    )
    .unwrap();
    let (output, passed) = nest(&app, &["fetch"]);
    assert!(passed, "an unversioned lock is format 1:\n{output}");

    // Not a lock at all.
    std::fs::write(app.join("project.lock.blsp"), "(project :x 1)\n").unwrap();
    let (output, passed) = nest(&app, &["check"]);
    assert!(!passed && output.contains("is not a lock file"), "{output}");
}
