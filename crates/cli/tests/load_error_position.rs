//! An error inside a module that loads on first use is reported where it IS, in that module's
//! file — not at the call that asked for the load.
//!
//! Compiling `(other/thing 1)` asks whether `other/thing` is a macro, which loads `other`, and
//! that load runs inside the macro-expansion boundary. The boundary moves an expander's errors
//! to the call (ADR-618), and it moved the broken module's too: a type error on line 5 of
//! `other.blsp` was reported as `caller.blsp:3:3`, pointing at a line with nothing wrong on
//! it. A load's error is now pinned (`LispErrorData::pos_pinned`), and the checker's warning
//! about the calling form names the file and line the error is in.

use std::process::Command;

fn run(dir: &std::path::Path, other: &str) -> String {
    std::fs::write(dir.join("other.blsp"), other).unwrap();
    std::fs::write(
        dir.join("caller.blsp"),
        "(defn go ()\n  (io/puts \"before\")\n  (other/thing 1))\n\n(go)\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(dir)
        .arg("caller.blsp")
        .output()
        .expect("run brood");
    String::from_utf8_lossy_owned(output.stderr)
}

fn module_with(line5: &str) -> String {
    format!("(defmodule other)\n\n(def table {{:a 1}})\n\n{line5}\n\n(defn thing (x) x)\n")
}

#[test]
fn an_error_in_a_lazily_loaded_module_names_that_module_s_line() {
    let dir = std::env::temp_dir().join(format!("brood-load-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // a runtime error, a compile error and a parse error, each on line 5 of other.blsp
    let cases = [
        ("(def broken (+ 1 \"two\"))", "other.blsp:5:13:"),
        ("(def broken (let (a) a))", "other.blsp:5:13:"),
        ("(def broken (+ 1 2)))", "other.blsp:5:21:"),
    ];
    let reports: Vec<(String, &str)> = cases
        .iter()
        .map(|(line5, expected)| (run(&dir, &module_with(line5)), *expected))
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    for (stderr, expected) in reports {
        assert!(
            stderr.contains(expected),
            "the error must name the broken module's line ({expected}):\n{stderr}"
        );
        assert!(
            !stderr.contains("caller.blsp:3:3"),
            "the call that loaded the module is not where the error is:\n{stderr}"
        );
    }
}
