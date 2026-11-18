//! `nest check` says the same thing on a cold project as on a warm one.
//!
//! A cold check (no `.brood/` image: a fresh clone, and every CI run) loads the project's
//! modules from source and writes the image; a warm one materialises them from it. Two bugs
//! lived in that difference on one day, each invisible from the other side: the cold path
//! loaded every module twice and printed `[reload] macro … redefined` (CI only), and the warm
//! path lost every `(meta …)` deprecation, because an image section did not carry it (local
//! runs only). So this runs the same check both ways over a project that uses what a
//! materialised module has historically come back without — a macro, a private helper, a
//! declared sig, a deprecation — and requires the same findings and the same exit code.

mod support;
use std::path::Path;
use std::process::Command;
use support::temp_dir;

const TOOL: &str = "(defmodule tool)\n\
                    (defmacro twice (form) `(do ~form ~form))\n\
                    (defn- helper () 1)\n\
                    (defn run () (twice (helper)))\n\
                    (defn size (text) (count text))\n\
                    (sig size (string -> int))\n\
                    (defn old-way () 1)\n\
                    (meta old-way :deprecated \"0.1.0\" :use 'run)\n";

const MAIN: &str = "(defmodule main (:use tool))\n\
                    (defn uses-old () (old-way))\n\
                    (defn uses-private () (helper))\n\
                    (defn wrong-size () (size 3))\n";

const TOOL_TEST: &str = "(defmodule tool-test\n  (:use test)\n  (:alias tool))\n\
                         (describe \"tool\" (test \"runs\" (assert= 1 (tool/run))))\n";

/// `nest check` in `root` with the check cache off: its findings (the build's own progress
/// lines left out) and whether it passed.
fn check(root: &Path) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("check")
        .current_dir(root)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let findings = text
        .lines()
        .filter(|line| !line.starts_with("Building ") && !line.starts_with("Built "))
        .collect::<Vec<_>>()
        .join("\n");
    (text, findings, output.status.success())
}

#[test]
fn a_cold_check_and_a_warm_check_report_the_same_findings() {
    let temp = temp_dir("check-cold-matches-warm");
    let root = &temp.path;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"twoway\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(root.join("src/tool.blsp"), TOOL).unwrap();
    std::fs::write(root.join("src/main.blsp"), MAIN).unwrap();
    std::fs::write(root.join("tests/tool_test.blsp"), TOOL_TEST).unwrap();

    let (cold_text, cold, cold_passed) = check(root);
    assert!(
        cold_text.contains("Building twoway"),
        "the first check did not build the project cold:\n{cold_text}"
    );
    // Not vacuous: the cold check reports each fact the image must carry.
    for expected in ["is deprecated", "helper", "size"] {
        assert!(
            cold.contains(expected),
            "the cold check reported nothing about {expected:?}:\n{cold}"
        );
    }
    assert!(
        !cold.contains("[reload]"),
        "a cold check reloaded a module:\n{cold}"
    );

    let (warm_text, warm, warm_passed) = check(root);
    assert!(
        !warm_text.contains("Building twoway"),
        "the second check built the project again instead of reading its image:\n{warm_text}"
    );
    assert_eq!(
        cold, warm,
        "a warm check reported differently from a cold one"
    );
    assert_eq!(cold_passed, warm_passed);
}
