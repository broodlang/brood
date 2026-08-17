//! Package-rooted namespaces end to end (ADR-070).
//!
//! A dependency `foo` providing a module `b` (`(defmodule b)`) is loaded as the global
//! namespace `foo/b`, so two dependencies can both provide a module of the *same* short
//! name with no collision — Rust's `crate::mod` model. This is the property that only a
//! real multi-package project can exercise, so it lives in an integration test rather
//! than the in-language suite: `nest test` resolves the deps, the package manager
//! registers each dep's modules rooted, and the loader roots each dep's `defmodule` and
//! its intra-package `(:use …)` targets.
//!
//! The fixture is the headline case: two deps, `liba` and `libb`, each providing a
//! `parser` module with a `parse` fn. Before rooting this was a hard collision the
//! package manager rejected; now both coexist as `liba/parser` and `libb/parser`, and an
//! app that depends on both reaches each by its rooted name — with no bare `parser/parse`
//! leaking into the flat namespace.

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

fn parent_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("brood-rooted-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

/// A dependency project `name` whose `parser` module's `parse` prepends `label`.
fn write_dep(parent: &Path, name: &str, label: &str) {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        format!("(project :name \"{name}\" :version \"0.1.0\")\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("src/parser.blsp"),
        format!("(defmodule parser)\n(defn parse (s) (str \"{label} parsed \" s))\n"),
    )
    .unwrap();
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
fn two_deps_with_the_same_module_name_coexist_rooted() {
    let parent = parent_dir("coexist");
    // Two deps that, pre-rooting, both declared `(defmodule parser)` — a hard collision.
    write_dep(&parent.path, "liba", "liba");
    write_dep(&parent.path, "libb", "libb");

    // The app depends on both. It reaches each dep's `parse` by the dep's rooted name
    // (both export `parse`, so a bare `(:use)` of both would clash — ADR-205 §7b — the
    // idiomatic resolution is `require` + qualified calls).
    let app = parent.path.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(app.join("tests")).unwrap();
    std::fs::write(
        app.join("project.blsp"),
        "(project\n \
         :name \"app\"\n \
         :version \"0.1.0\"\n \
         :dependencies\n \
         [[liba :path \"../liba\"]\n  [libb :path \"../libb\"]])\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src/main.blsp"),
        "(defmodule main)\n\
         (defn from-a (s) (liba/parser/parse s))\n\
         (defn from-b (s) (libb/parser/parse s))\n",
    )
    .unwrap();
    std::fs::write(
        app.join("tests/main_test.blsp"),
        "(defmodule main-test (:use test) (:use main))\n\
         (describe \"package-rooted deps coexist\"\n\
         \x20 (test \"liba/parser resolves distinctly\"\n\
         \x20   (assert= (from-a \"z\") \"liba parsed z\"))\n\
         \x20 (test \"libb/parser resolves distinctly\"\n\
         \x20   (assert= (from-b \"z\") \"libb parsed z\"))\n\
         \x20 (test \"both rooted globals exist and nothing leaked bare\"\n\
         \x20   (is (and (bound? 'liba/parser/parse)\n\
         \x20            (bound? 'libb/parser/parse)\n\
         \x20            (not (bound? 'parser/parse))))))\n",
    )
    .unwrap();

    let (text, ok) = nest(&app, &["test"]);
    assert!(
        ok,
        "the rooted two-dep project should build and pass:\n{text}"
    );
    assert!(
        text.contains("3 tests, 3 passed"),
        "all three rooting assertions should pass:\n{text}"
    );
}

#[test]
fn intra_package_use_roots_to_the_dependency() {
    // A dependency whose own modules reference each other by SHORT names (`(:use util)`)
    // must have those refs rooted to `dep/util` at load — intra-package refs stay short
    // in source, root at load (ADR-070). Proven by a dep `libc` whose `api` module
    // `(:use util)`s a sibling `util` module and the app calling through it.
    let parent = parent_dir("intra");
    let dep = parent.path.join("libc");
    std::fs::create_dir_all(dep.join("src")).unwrap();
    std::fs::write(
        dep.join("project.blsp"),
        "(project :name \"libc\" :version \"0.1.0\")\n",
    )
    .unwrap();
    std::fs::write(
        dep.join("src/util.blsp"),
        "(defmodule util)\n(defn shout (s) (str s \"!\"))\n",
    )
    .unwrap();
    // `api` refers its sibling `util` by short name — this must root to `libc/util`.
    std::fs::write(
        dep.join("src/api.blsp"),
        "(defmodule api (:use util))\n(defn loud (s) (shout s))\n",
    )
    .unwrap();

    let app = parent.path.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(app.join("tests")).unwrap();
    std::fs::write(
        app.join("project.blsp"),
        "(project :name \"app\" :version \"0.1.0\" \
         :dependencies [[libc :path \"../libc\"]])\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src/main.blsp"),
        "(defmodule main (:use libc/api))\n(defn go (s) (loud s))\n",
    )
    .unwrap();
    std::fs::write(
        app.join("tests/main_test.blsp"),
        "(defmodule main-test (:use test) (:use main))\n\
         (describe \"intra-package use roots\"\n\
         \x20 (test \"api's (:use util) reached libc/util\"\n\
         \x20   (assert= (go \"hi\") \"hi!\"))\n\
         \x20 (test \"the sibling rooted, not bare\"\n\
         \x20   (is (and (bound? 'libc/util/shout) (not (bound? 'util/shout))))))\n",
    )
    .unwrap();

    let (text, ok) = nest(&app, &["test"]);
    assert!(ok, "the intra-package-use project should pass:\n{text}");
    assert!(
        text.contains("2 tests, 2 passed"),
        "both intra-package assertions should pass:\n{text}"
    );
}

#[test]
fn the_root_project_roots_its_own_modules_under_its_name() {
    // The Elixir-uniform model (ADR-070): the ROOT project's own modules root under its
    // `:name`, with the prefix implied — `(defmodule greeter)` in project `myapp` is the
    // global `myapp/greeter`, and an intra-project `(:use greeter)` stays short in source
    // but roots at load. Verified through `nest test` end to end, and that `nest check`
    // stays clean (the checker roots the same way the loader does).
    let parent = parent_dir("rootproj");
    let app = parent.path.join("myapp");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(app.join("tests")).unwrap();
    std::fs::write(
        app.join("project.blsp"),
        "(project :name myapp :version \"0.1.0\")\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src/greeter.blsp"),
        "(defmodule greeter)\n(defn hi () \"hi from greeter\")\n",
    )
    .unwrap();
    // `main` refers its sibling `greeter` by short name — must root to `myapp/greeter`.
    std::fs::write(
        app.join("src/main.blsp"),
        "(defmodule main (:use greeter))\n(defn go () (hi))\n",
    )
    .unwrap();
    std::fs::write(
        app.join("tests/main_test.blsp"),
        "(defmodule main-test (:use test) (:use main))\n\
         (describe \"the root project roots its own modules\"\n\
         \x20 (test \"intra-project (:use greeter) resolved\"\n\
         \x20   (assert= (go) \"hi from greeter\"))\n\
         \x20 (test \"the REAL globals are the rooted names, not the bare ones\"\n\
         \x20   ;; `global-names` lists actual bindings; the bare `main/go` is only\n\
         \x20   ;; REACHABLE via root_qualified_ref's alias (ADR-070), not a binding.\n\
         \x20   (let (globals (map name (global-names)))\n\
         \x20     (is (includes? globals \"myapp/main/go\"))\n\
         \x20     (is (includes? globals \"myapp/greeter/hi\"))\n\
         \x20     (is (not (includes? globals \"main/go\")))\n\
         \x20     (is (not (includes? globals \"greeter/hi\"))))))\n",
    )
    .unwrap();

    let (test_text, test_ok) = nest(&app, &["test"]);
    assert!(test_ok, "the root-rooted project should pass:\n{test_text}");
    assert!(
        test_text.contains("2 tests, 2 passed"),
        "both root-rooting assertions should pass:\n{test_text}"
    );

    // The checker must root intra-project `(:use)` the same way — no false "unbound".
    let (check_text, check_ok) = nest(&app, &["check"]);
    assert!(check_ok, "nest check should exit cleanly:\n{check_text}");
    assert!(
        !check_text.contains("unbound symbol"),
        "the checker should resolve rooted intra-project imports, not flag them:\n{check_text}"
    );
}
