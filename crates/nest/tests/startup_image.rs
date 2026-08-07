//! End-to-end tests for the startup image (ADR-218) — that a second run of the same
//! project is served *from the image* and produces the same answers as the first.
//!
//! These exist because the image was written on every cold start and then never read
//! from, and nothing failed: the modules simply loaded from source again, so every
//! observable answer stayed right while the whole mechanism was dead. Two independent
//! defects, both invisible to a unit test of the `%image-*` primitives:
//!
//! 1. `project-install-image` ran `(def *image-sections* …)` inside module `project`,
//!    which binds `project/*image-sections*` — while `require-force`, root code, kept
//!    reading the empty root global.
//! 2. `require-force` consulted `*package-module-files*` before `*image-sections*`, and
//!    a project roots its OWN modules (ADR-070), so the package branch matched first for
//!    every module of every named project.
//!
//! The only thing that catches either is running a project twice and looking at what the
//! second run *loads*, which is what these do: a `println` at a module's top level is
//! evaluated by a source load and absent from an imaged one.

use std::path::Path;
use std::process::Command;

/// A project directory of its own, so cases can run concurrently under nextest.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-startup-image-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

/// Run `nest run` in `dir`, returning stdout+stderr together (the loader's "Building …"
/// line goes to stderr, the program's output to stdout, and cases assert on both).
fn nest_run(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .current_dir(dir)
        .output()
        .expect("spawn nest");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "nest run failed: status={:?}\n{combined}",
        out.status
    );
    combined
}

/// The second run of an unchanged project must materialise its modules from the image
/// instead of evaluating their sources — the whole point of ADR-218.
#[test]
fn a_second_run_loads_modules_from_the_image_not_from_source() {
    let dir = scratch("lazy");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    // The marker is a TOP-LEVEL side effect: an image carries the module's bindings, so
    // materialising it cannot print this. A source load must.
    write(
        &dir,
        "src/lib.blsp",
        "(defmodule lib)\n\
         (println \"SOURCE-LOAD: lib\")\n\
         (defn twice (x) (* 2 x))\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use lib))\n\
         (defn main () (println (str \"ANSWER: \" (twice 21))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: lib"), "cold run:\n{cold}");
    assert!(cold.contains("ANSWER: 42"), "cold run:\n{cold}");

    let warm = nest_run(&dir);
    assert!(warm.contains("ANSWER: 42"), "warm run:\n{warm}");
    assert!(
        !warm.contains("SOURCE-LOAD: lib"),
        "the second run re-evaluated the module's source, so the image was not used:\n{warm}"
    );
    // …and it did not silently rebuild the image either (that would also skip the marker
    // on a later run while doing all the work again).
    assert!(
        !warm.contains("Building demo"),
        "the second run rebuilt from source:\n{warm}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An edit must invalidate the image — the staleness key is what makes serving an image
/// safe at all, and its failure mode is running the *previous* version of the code.
#[test]
fn an_edited_source_file_invalidates_the_image() {
    let dir = scratch("stale");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n(defn main () (println \"ANSWER: 1\"))\n",
    );
    assert!(nest_run(&dir).contains("ANSWER: 1"));

    // A second file's edit must invalidate too, not just the entry point's.
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app)\n(defn main () (println \"ANSWER: 2\"))\n",
    );
    let after = nest_run(&dir);
    assert!(
        after.contains("ANSWER: 2"),
        "an imaged start ran the pre-edit code:\n{after}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A dependency's modules are imaged like the project's own (ADR-070/218) — and because they
/// live outside `:source-paths` (`_deps/`, or anywhere at all for a `:path` dep), their files
/// have to be in the staleness key or an imaged start would serve the dep's old code forever.
/// Both halves are asserted here: served from the image, and invalidated by an edit.
///
/// The two are one test on purpose — "imaged" without "invalidated" is the dangerous state,
/// and it is only dangerous in combination.
#[test]
fn a_path_dependency_is_imaged_and_its_edits_invalidate() {
    let dep = scratch("dep-lib");
    let dir = scratch("dep-app");
    write(&dep, "project.blsp", "(project :name libdep)\n");
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util)\n\
         (println \"SOURCE-LOAD: libdep/util\")\n\
         (defn double (x) (* 2 x))\n",
    );
    // `:path` is resolved against the project root, so it has to be relative — both scratch
    // dirs are siblings under the temp dir.
    write(
        &dir,
        "project.blsp",
        &format!(
            "(project\n  :name depdemo\n  :main app\n  :dependencies [[libdep :path \"../{}\"]])\n",
            dep.file_name().unwrap().to_string_lossy()
        ),
    );
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use libdep/util))\n\
         (defn main () (println (str \"ANSWER: \" (double 21))))\n",
    );

    let cold = nest_run(&dir);
    assert!(cold.contains("SOURCE-LOAD: libdep/util"), "cold:\n{cold}");
    assert!(cold.contains("ANSWER: 42"), "cold:\n{cold}");

    let warm = nest_run(&dir);
    assert!(warm.contains("ANSWER: 42"), "warm:\n{warm}");
    assert!(
        !warm.contains("SOURCE-LOAD: libdep/util"),
        "the dependency was re-evaluated instead of materialised from the image:\n{warm}"
    );

    // Editing the DEP — outside this project's source paths entirely — must invalidate.
    write(
        &dep,
        "src/util.blsp",
        "(defmodule util)\n\
         (println \"SOURCE-LOAD: libdep/util\")\n\
         (defn double (x) (* 3 x))\n",
    );
    let edited = nest_run(&dir);
    assert!(
        edited.contains("ANSWER: 63"),
        "an imaged start ran the dependency's pre-edit code:\n{edited}"
    );
    // …and the rebuilt image serves the NEW dep code, rather than rebuilding every run.
    let after = nest_run(&dir);
    assert!(after.contains("ANSWER: 63"), "after:\n{after}");
    assert!(
        !after.contains("SOURCE-LOAD: libdep/util"),
        "the dependency stopped being imaged after its edit:\n{after}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dep);
}

/// Loading MUTATES registries (`*impls*`, `*methods*`, `*method-from*`, `*module-docs*`, …)
/// rather than creating them, so the `(global-names)` diff an image is built from cannot
/// see them and each has to be carried deliberately. They were named by hand and the list
/// went stale repeatedly — silently, because a lost registry is a wrong answer with no
/// error. The set is derived from the registry funnel now (`%registry-names`), and this is
/// the case that fails if the derivation stops reaching one.
#[test]
fn an_imaged_start_keeps_what_loading_registered() {
    let dir = scratch("registries");
    write(
        &dir,
        "project.blsp",
        "(project\n  :name demo\n  :main app)\n",
    );
    write(
        &dir,
        "src/shapes.blsp",
        "(defmodule shapes \"Docstring for the module registry.\")\n\
         (println \"SOURCE-LOAD: shapes\")\n\
         (defmulti combine)\n\
         (defmethod combine [:int :int] (a b) (+ a b))\n\
         (defability Sized (size [self] :-> int))\n\
         (defrecord box (w h))\n\
         (impl Sized shapes/box (size [s] (* (get s :w) (get s :h))))\n",
    );
    // Each line reads a different registry: dispatch through `*methods*`, ability dispatch
    // through `*impls*`/`*abilities*`, the record's nominal id through `*record-ids*`,
    // conflict provenance through `*method-from*`, and the docstring through `*module-docs*`.
    write(
        &dir,
        "src/app.blsp",
        "(defmodule app (:use shapes))\n\
         (defn main ()\n\
        \x20 (println (str \"multi: \" (combine 2 3)))\n\
        \x20 (println (str \"ability: \" (size (box 3 4))))\n\
        \x20 (println (str \"provenance: \" (count *method-from*)))\n\
        \x20 (println (str \"docs: \" (contains? *module-docs* \"demo/shapes\"))))\n",
    );

    let cold = nest_run(&dir);
    let warm = nest_run(&dir);
    assert!(
        !warm.contains("SOURCE-LOAD"),
        "precondition: the warm run should be imaged:\n{warm}"
    );
    for expected in ["multi: 5", "ability: 12", "provenance: 1", "docs: true"] {
        assert!(
            cold.contains(expected),
            "cold run lacks {expected}:\n{cold}"
        );
        assert!(
            warm.contains(expected),
            "an imaged start lost a registry — {expected} did not survive:\n{warm}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
