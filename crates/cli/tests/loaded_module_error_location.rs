//! A compile error inside a module that a qualified reference loads is reported in THAT
//! module, at its line. The load runs while the referencing form expands, and the expander
//! boundary moves every position an expansion picks up to the call (ADR-618), so the error
//! used to name the caller's `(lazylib/ok 1)` with a message about a `let` it did not write.
//! The pre-flight check reports at the referencing form (its findings are places in the file
//! it checks) and names the module's line in the message.

use std::process::Command;

fn run(dir: &std::path::Path, file: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(dir)
        .arg(file)
        .output()
        .expect("run brood");
    assert!(
        !output.status.success(),
        "a broken module must fail the run"
    );
    String::from_utf8_lossy_owned(output.stderr)
}

#[test]
fn a_loaded_modules_compile_error_names_the_module_line() {
    let dir = std::env::temp_dir().join(format!("brood-loaded-error-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("lazylib.blsp"),
        "(defmodule lazylib)\n\n(defn ok (x) x)\n\n(defn broken (x)\n  (let (if 1) x))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("top.blsp"),
        "(defmodule user)\n(io/puts \"start\")\n(io/puts (lazylib/ok 1))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("body.blsp"),
        "(defmodule user)\n(defn go () (lazylib/ok 1))\n(io/puts (go))\n",
    )
    .unwrap();

    let top = run(&dir, "top.blsp");
    let body = run(&dir, "body.blsp");
    let _ = std::fs::remove_dir_all(&dir);
    for (name, stderr) in [("top level", &top), ("a function body", &body)] {
        assert!(
            stderr.contains(
                "lazylib.blsp:6:8: runtime error: `if` is one of the evaluator's own forms"
            ),
            "from {name}, the error must name the module's line:\n{stderr}"
        );
    }
    assert!(
        top.contains("top.blsp:3:1: warning: does not compile: ./lazylib.blsp:6:8: `if`"),
        "the pre-flight reports at the referencing form and names the module's line:\n{top}"
    );
}
