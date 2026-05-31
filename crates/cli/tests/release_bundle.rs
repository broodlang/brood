//! End-to-end release-bundle test (ADR-038): append an archive to the prebuilt
//! `brood` binary, then run the result as its own process and assert it boots
//! the *embedded* `:main` — from a cwd with no project and no sources on disk.
//!
//! This exercises the `brood`-side half of `nest release`: footer detection on
//! `current_exe`, mounting the archive, resolving an app module from the bundle
//! through `require` (via the extended `%builtin-module`), and dispatching to
//! `project/run-bundle`. The `nest`-side collection is covered by the unit tests
//! in `crates/lisp/src/bundle.rs` plus this manual archive construction.

use std::process::Command;

/// Build `[brood][archive][footer]` for a two-module app and return its path,
/// alongside a separate empty directory to run it from.
fn write_app(tag: &str, manifest: &str, modules: &[(&str, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
    let brood = env!("CARGO_BIN_EXE_brood");
    let base = std::fs::read(brood).expect("read brood binary");
    let owned: Vec<(String, String)> = modules
        .iter()
        .map(|(n, s)| (n.to_string(), s.to_string()))
        .collect();
    let archive = brood::bundle::serialize(manifest, &owned);

    let dir = std::env::temp_dir().join(format!("brood-release-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let app = dir.join("app");
    brood::bundle::write_release(&base, &archive, &app).expect("write release binary");

    // Run from an empty subdir: proves nothing is read from the project tree.
    let run_cwd = dir.join("clean");
    std::fs::create_dir_all(&run_cwd).unwrap();
    (app, run_cwd)
}

#[test]
fn bundled_brood_boots_embedded_main_with_cross_module_use() {
    let (app, cwd) = write_app(
        "main",
        "(project :name \"t\" :version \"0\")",
        &[
            // `main` uses `lib` — proves cross-module `require`/`:use` resolves
            // out of the embedded archive, not the disk load-path.
            ("main", "(defmodule main (:use lib))\n(defn main () (println (greet)))"),
            ("lib", "(defmodule lib)\n(defn greet () \"embedded-ok\")"),
        ],
    );
    let out = Command::new(&app)
        .current_dir(&cwd)
        .output()
        .expect("run bundled app");
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "embedded-ok");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn bundled_app_receives_argv() {
    let (app, cwd) = write_app(
        "argv",
        "(project :name \"t\" :version \"0\")",
        &[(
            "main",
            "(defmodule main)\n(defn main (& args) (println (str \"argv:\" args)))",
        )],
    );
    let out = Command::new(&app)
        .args(["alpha", "beta"])
        .current_dir(&cwd)
        .output()
        .expect("run bundled app");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "argv:(alpha beta)"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}
