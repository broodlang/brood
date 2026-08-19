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

mod support;

/// Build `[brood][archive][footer]` for a two-module app and return its path,
/// alongside a separate empty directory to run it from.
fn write_app(
    tag: &str,
    manifest: &str,
    modules: &[(&str, &str)],
) -> (std::path::PathBuf, std::path::PathBuf) {
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
            (
                "main",
                "(defmodule main (:use lib))\n(defn main () (println (greet)))",
            ),
            ("lib", "(defmodule lib)\n(defn greet () \"embedded-ok\")"),
        ],
    );
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run bundled app");
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
fn bundled_deps_with_same_module_name_coexist_rooted() {
    // Dev/release parity for package-rooting (ADR-070): two dependencies each provide a
    // `parser` module (and a `util` module). At dev time they are distinct globals
    // `alpha/parser` / `beta/parser`; the bundle must keep them apart too. Each dep module
    // is embedded under its ROOTED key, and `main` calls both — proving neither clobbered
    // the other and that each `parser`'s intra-package `(:use util)` rooted to its OWN
    // `util` (alpha→"A", beta→"B"), not the other dep's.
    let (app, cwd) = write_app(
        "same-name-deps",
        "(project :name \"t\" :version \"0\" :dependencies [[alpha :path \"a\"] [beta :path \"b\"]])",
        &[
            (
                "main",
                "(defmodule main)\n(defn main () (println (str (alpha/parser/tag) \"|\" (beta/parser/tag))))",
            ),
            // `parser` in each dep uses its OWN `util` via a bare intra-package `(:use util)`,
            // which must root to `alpha/util` / `beta/util` under each dep's package context.
            (
                "alpha/parser",
                "(defmodule parser (:use util))\n(defn tag () (str \"alpha:\" (util-tag)))",
            ),
            ("alpha/util", "(defmodule util)\n(defn util-tag () \"A\")"),
            (
                "beta/parser",
                "(defmodule parser (:use util))\n(defn tag () (str \"beta:\" (util-tag)))",
            ),
            ("beta/util", "(defmodule util)\n(defn util-tag () \"B\")"),
        ],
    );
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run bundled app");
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "alpha:A|beta:B"
    );
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
    let mut cmd = Command::new(&app);
    cmd.args(["alpha", "beta"]).current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run bundled app");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "argv:(alpha beta)"
    );
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}

#[test]
fn bundled_bare_reference_to_unique_dep_module_resolves() {
    // A dependency's uniquely-named module referenced by its BARE `(defmodule)` name — the idiom
    // every real app uses (hive's `(:alias repo)`), and the way a dev run binds it (found by
    // basename on the load-path, never rooted). The bundle embeds it under its rooted key
    // `store/repo`; `:bundled-packages` records that `store` ships it (bundle-collect bakes that
    // set, since a transitive dependency is absent from the manifest's `:dependencies`). With a
    // unique name it must bind BARE so the bare reference resolves — this crash-looped with
    // `require: cannot find module 'repo'` before rooting became collision-only (ADR-070).
    let (app, cwd) = write_app(
        "bare-dep-ref",
        "(project :name \"t\" :version \"0\" :bundled-packages [\"store\"])",
        &[
            (
                "main",
                "(defmodule main (:alias repo))\n(defn main () (println (repo/tag)))",
            ),
            (
                "store/repo",
                "(defmodule repo)\n(defn tag () \"bare-dep-ok\")",
            ),
        ],
    );
    let mut cmd = Command::new(&app);
    cmd.current_dir(&cwd);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run bundled app");
    assert!(
        out.status.success(),
        "exit: {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "bare-dep-ok");
    let _ = std::fs::remove_dir_all(app.parent().unwrap());
}
