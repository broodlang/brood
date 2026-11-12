//! The project image's partial rebuild (ADR-419): after an edit, only the edited modules and
//! the modules that depend on them load from source; the rest materialise from the image.
//!
//! What can go wrong is silent — a module reused from the image while something it was built
//! from moved answers with the OLD value, and nothing raises. So every case here asserts two
//! things after each edit: WHICH modules loaded from source (each prints a `SOURCE-LOAD:`
//! marker at top level, which a materialised module cannot), and that the program's answers
//! equal those of a from-scratch build of the same tree.
//!
//! The fixture covers each way a module can depend on another: a header `(:use …)`, a
//! qualified reference evaluated at load time with no `:use`, a macro whose expansion names a
//! third module, a load that evaluates a COMPUTED string (pinned: always reloads), an ability
//! impl and a registry entry the edited module stops making, and a module nothing depends on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-partial-image-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

/// `nest run` in `dir`: (stdout+stderr, the modules that loaded from source, the answer line).
fn nest_run(dir: &Path) -> (String, BTreeSet<String>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .args(["run", "--no-check"])
        .current_dir(dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .output()
        .expect("spawn nest");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "nest run failed:\n{combined}");
    let loaded = combined
        .lines()
        .filter_map(|l| l.strip_prefix("SOURCE-LOAD: "))
        .map(str::to_string)
        .collect();
    let answer = combined
        .lines()
        .find(|l| l.starts_with("ANSWER "))
        .unwrap_or_else(|| panic!("no ANSWER line:\n{combined}"))
        .to_string();
    (combined, loaded, answer)
}

/// The answer a from-scratch build of `dir`'s sources gives: a copy of `src/` and the
/// manifest with no `.brood/`, so nothing can be reused.
fn answer_from_scratch(dir: &Path, tag: &str) -> String {
    let fresh = scratch(tag);
    std::fs::copy(dir.join("project.blsp"), fresh.join("project.blsp")).unwrap();
    for entry in std::fs::read_dir(dir.join("src")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), fresh.join("src").join(entry.file_name())).unwrap();
    }
    let (out, _, answer) = nest_run(&fresh);
    assert!(
        out.contains("Building "),
        "the oracle must build from source:\n{out}"
    );
    let _ = std::fs::remove_dir_all(&fresh);
    answer
}

/// Rewrite a source file. The image's key is size + mtime, and an edit inside one clock tick
/// that kept the size would not move it; real edits get the tick, these force it.
fn edit(dir: &Path, rel: &str, src: &str) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(dir, rel, src);
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// A module that prints its marker when it loads from source. Each defines a global (`here`):
/// a module with NO globals — only an `impl` or a registry row — has no image section, so it
/// loads from source on every run, warm or not, and its marker would say nothing about reuse.
fn module(name: &str, header: &str, body: &str) -> String {
    format!(
        "(defmodule {name}{header})\n(io/puts \"SOURCE-LOAD: {name}\")\n(def here true)\n{body}\n"
    )
}

fn fixture(dir: &Path) {
    write(
        dir,
        "project.blsp",
        "(project\n  :name pfx\n  :main main)\n",
    );
    write(
        dir,
        "src/geom.blsp",
        &module("geom", "", "(def k 10)\n(defn sq (x) (* x x))"),
    );
    // A load-time value through a qualified reference, with no `(:use geom)`.
    write(
        dir,
        "src/shapes.blsp",
        &module("shapes", "", "(def area-k (geom/sq geom/k))"),
    );
    // A macro whose EXPANSION names `geom`; `usemac` never mentions geom itself.
    write(
        dir,
        "src/mac.blsp",
        &module("mac", "", "(defmacro scaled (x) `(* ~x geom/k))"),
    );
    write(
        dir,
        "src/usemac.blsp",
        &module("usemac", " (:use mac)", "(def m (scaled 3))"),
    );
    // A load that evaluates a string built at run time: no scan can see it names `geom`.
    write(
        dir,
        "src/dyn.blsp",
        &module(
            "dyn",
            "",
            "(def d (reflect/eval-string (str \"(\" \"geo\" \"m/sq 3)\")))",
        ),
    );
    write(dir, "src/other.blsp", &module("other", "", "(def v 1)"));
    write(
        dir,
        "src/sizes.blsp",
        &module(
            "sizes",
            "",
            "(defability Size (size (self) :-> int))\n(impl Size :int (size (n) n))",
        ),
    );
    write(
        dir,
        "src/strsize.blsp",
        &module(
            "strsize",
            " (:use sizes)",
            "(impl Size :string (size (s) (count s)))",
        ),
    );
    write(
        dir,
        "src/hooks.blsp",
        &module(
            "hooks",
            "",
            "(registry/put :pfx/hooks :save 1)\n(registry/put :pfx/hooks :load 2)",
        ),
    );
    write(
        dir,
        "src/main.blsp",
        "(defmodule main (:use shapes) (:use usemac) (:use dyn) (:use other) (:use sizes) (:use strsize) (:use hooks))\n\
         (io/puts \"SOURCE-LOAD: main\")\n\
         (defn main ()\n\
           (io/puts (str \"ANSWER area-k=\" shapes/area-k \" m=\" usemac/m \" d=\" dyn/d \" v=\" other/v\n\
                      \" int=\" (size 4) \" str=\" (try (size \"abc\") (catch e \"no-impl\"))\n\
                      \" hooks=\" (sort (keys (or (registry/entries :pfx/hooks) {}))))))\n",
    );
}

#[test]
fn an_edit_reloads_the_edited_module_and_its_dependents_and_nothing_else() {
    let dir = scratch("edit");
    fixture(&dir);
    let (cold, _, first) = nest_run(&dir);
    assert!(
        cold.contains("Building pfx"),
        "the first run builds:\n{cold}"
    );
    let (warm, loaded, answer) = nest_run(&dir);
    assert!(
        loaded.is_empty(),
        "an unchanged project loads nothing from source:\n{warm}"
    );
    assert_eq!(answer, first);

    // `geom` moves: everything that read it at load time — by a qualified reference, through
    // a macro's expansion, and the pinned string evaluation — reloads. `other`, `sizes`,
    // `strsize` and `hooks` never touched it and stay imaged.
    edit(
        &dir,
        "src/geom.blsp",
        &module("geom", "", "(def k 20)\n(defn sq (x) (* x x))"),
    );
    let (out, loaded, answer) = nest_run(&dir);
    assert!(out.contains("Rebuilt pfx: 6 modules"), "{out}");
    assert_eq!(
        loaded,
        set(&["dyn", "geom", "mac", "main", "shapes", "usemac"]),
        "{out}"
    );
    assert_eq!(answer, answer_from_scratch(&dir, "edit-oracle-1"));
    assert!(answer.contains("area-k=400 m=60"), "{answer}");

    // The rewritten image serves the next run whole.
    let (out, loaded, again) = nest_run(&dir);
    assert!(loaded.is_empty(), "the rebuilt image is current:\n{out}");
    assert_eq!(again, answer);

    // A module nothing reads reloads alone, with `main` (which `:use`s it) and the pin.
    edit(&dir, "src/other.blsp", &module("other", "", "(def v 2)"));
    let (out, loaded, answer) = nest_run(&dir);
    assert_eq!(loaded, set(&["dyn", "main", "other"]), "{out}");
    assert_eq!(answer, answer_from_scratch(&dir, "edit-oracle-2"));
}

#[test]
fn a_registration_the_edited_module_stops_making_is_gone() {
    let dir = scratch("regs");
    fixture(&dir);
    nest_run(&dir);
    let (_, _, before) = nest_run(&dir);
    assert!(
        before.contains("str=3") && before.contains("hooks=(:load :save)"),
        "{before}"
    );

    // The impl and one registry row go; the image's registry VALUES still hold them, so the
    // partial rebuild must take them out before `strsize` and `hooks` load again.
    edit(
        &dir,
        "src/strsize.blsp",
        &module("strsize", " (:use sizes)", "(def nothing 0)"),
    );
    edit(
        &dir,
        "src/hooks.blsp",
        &module("hooks", "", "(registry/put :pfx/hooks :save 1)"),
    );
    let (out, loaded, answer) = nest_run(&dir);
    assert!(out.contains("Rebuilt pfx"), "{out}");
    assert!(
        !loaded.contains("geom") && !loaded.contains("sizes"),
        "{out}"
    );
    assert!(
        answer.contains("str=no-impl") && answer.contains("hooks=(:save)"),
        "{answer}"
    );
    assert_eq!(answer, answer_from_scratch(&dir, "regs-oracle-1"));

    // A second, unrelated edit: `hooks` is materialised this time, and the ownership of its
    // remaining row has to survive in the record for the edit after.
    edit(&dir, "src/other.blsp", &module("other", "", "(def v 2)"));
    let (out, loaded, _) = nest_run(&dir);
    assert!(!loaded.contains("hooks"), "{out}");
    edit(&dir, "src/hooks.blsp", &module("hooks", "", "(def none 0)"));
    let (out, _, answer) = nest_run(&dir);
    assert!(!answer.contains(":save"), "{out}");
    assert_eq!(answer, answer_from_scratch(&dir, "regs-oracle-2"));
}

#[test]
fn a_headerless_or_deleted_file_rebuilds_the_image_whole() {
    let dir = scratch("whole");
    fixture(&dir);
    nest_run(&dir);
    write(&dir, "src/loose.blsp", "(def top-level 1)\n");
    let (out, _, _) = nest_run(&dir);
    assert!(
        out.contains("Building pfx"),
        "a new headerless file rebuilds whole:\n{out}"
    );
    edit(&dir, "src/loose.blsp", "(def top-level 2)\n");
    let (out, _, _) = nest_run(&dir);
    assert!(
        out.contains("Building pfx"),
        "an edited headerless file rebuilds whole:\n{out}"
    );
    std::fs::remove_file(dir.join("src/loose.blsp")).unwrap();
    let (out, _, answer) = nest_run(&dir);
    assert!(
        out.contains("Building pfx"),
        "a deleted file rebuilds whole:\n{out}"
    );
    assert_eq!(answer, answer_from_scratch(&dir, "whole-oracle"));
}

/// `nest <args>` in `dir`, asserting success; the combined output.
fn nest_ok(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .args(args)
        .current_dir(dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .output()
        .expect("spawn nest");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "nest {args:?} failed:\n{combined}");
    combined
}

/// A `nest run` after an edit writes the image with every unchanged section COPIED from the
/// image before it, not re-encoded. So after a chain of edits most sections are copies of
/// copies — and the tools that load the whole project (`--check-boot` here, as `nest test` and
/// `nest check` do) must read that image exactly as they read a fresh build.
#[test]
fn a_chain_of_copied_images_serves_every_consumer() {
    let dir = scratch("chain");
    fixture(&dir);
    nest_run(&dir);
    for (step, (rel, src)) in [
        (
            "src/geom.blsp",
            module("geom", "", "(def k 11)\n(defn sq (x) (* x x))"),
        ),
        ("src/other.blsp", module("other", "", "(def v 5)")),
        (
            "src/shapes.blsp",
            module("shapes", "", "(def area-k (+ 1 (geom/sq geom/k)))"),
        ),
        (
            "src/geom.blsp",
            module("geom", "", "(def k 12)\n(defn sq (x) (* x x))"),
        ),
    ]
    .iter()
    .enumerate()
    {
        edit(&dir, rel, src);
        let (out, loaded, answer) = nest_run(&dir);
        assert!(
            out.contains("Rebuilt pfx"),
            "step {step}: a partial rebuild:\n{out}"
        );
        assert!(
            !loaded.contains("sizes") && !loaded.contains("hooks"),
            "step {step}: an untouched module is reused:\n{out}"
        );
        assert_eq!(
            answer,
            answer_from_scratch(&dir, &format!("chain-oracle-{step}")),
            "step {step}"
        );
    }
    // The whole-project load, over the copied image: every module, no rebuild, no error.
    let out = nest_ok(&dir, &["run", "--check-boot"]);
    assert!(
        !out.contains("Building") && !out.contains("Rebuilt"),
        "the image is current for a whole-project load:\n{out}"
    );
    let (out, loaded, answer) = nest_run(&dir);
    assert!(loaded.is_empty(), "{out}");
    assert!(answer.contains("area-k=145"), "{answer}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Several `nest run`s of one project right after an edit each make the partial rebuild and
/// each write the image (and the record beside it). The writes are atomic renames, and a copy
/// reads from the image file the run itself opened, so one run renaming a new image over the
/// path cannot change the bytes another is copying. Every run must answer like a fresh build,
/// and whichever image wins must serve the next run without a rebuild.
#[test]
fn concurrent_runs_after_an_edit_agree_and_leave_a_current_image() {
    let dir = scratch("concurrent");
    fixture(&dir);
    nest_run(&dir);
    for round in 0..3 {
        edit(
            &dir,
            "src/geom.blsp",
            &module(
                "geom",
                "",
                &format!("(def k {})\n(defn sq (x) (* x x))", 30 + round),
            ),
        );
        let oracle = answer_from_scratch(&dir, &format!("concurrent-oracle-{round}"));
        let answers: Vec<String> = std::thread::scope(|s| {
            let runs: Vec<_> = (0..4).map(|_| s.spawn(|| nest_run(&dir).2)).collect();
            runs.into_iter().map(|r| r.join().unwrap()).collect()
        });
        for answer in &answers {
            assert_eq!(
                answer, &oracle,
                "round {round}: a concurrent run answered differently"
            );
        }
        let (out, loaded, answer) = nest_run(&dir);
        assert!(
            loaded.is_empty(),
            "round {round}: the image a concurrent run left is current:\n{out}"
        );
        assert_eq!(answer, oracle);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
