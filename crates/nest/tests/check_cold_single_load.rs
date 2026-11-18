//! A cold `nest check` loads each project module once.
//!
//! The check's pre-flight (`project-preload!`) requires every module the files' headers name,
//! as the header spells it (`tool`). A named project's own modules are registered rooted
//! (`single/tool`), so the short name was never found loaded and the module was loaded a
//! second time — on every cold check, as in each package's CI run — and its `defmacro` printed
//! the hot-reload "macro … redefined" notice. The pre-flight now roots each name as the header
//! clauses do (`%root-module-name`).

mod support;
use std::process::Command;
use support::temp_dir;

const TOOL: &str = "(defmodule tool)\n\
                    (defmacro twice (form) `(do ~form ~form))\n\
                    (defn run () (twice 1))\n";

const TOOL_TEST: &str = "(defmodule tool-test\n  (:use test)\n  (:alias tool))\n\
                         (describe \"tool\" (test \"runs\" (assert= 1 (tool/run))))\n";

#[test]
fn a_cold_check_defines_a_project_macro_once() {
    let temp = temp_dir("check-cold-single-load");
    let root = &temp.path;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"single\" :version \"0.1.0\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(root.join("src/tool.blsp"), TOOL).unwrap();
    std::fs::write(root.join("tests/tool_test.blsp"), TOOL_TEST).unwrap();

    // No `.brood/` image yet (a fresh project) and no check cache: the cold path CI takes.
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
    assert!(
        text.contains("Building single"),
        "the check did not build the project cold:\n{text}"
    );
    assert!(
        !text.contains("[reload]"),
        "a cold check loaded a project module twice:\n{text}"
    );
    assert!(output.status.success(), "nest check failed:\n{text}");
}
