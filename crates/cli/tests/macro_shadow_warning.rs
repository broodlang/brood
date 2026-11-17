//! A module defining a name the language ships (ADR-423, ADR-735): declared with
//! `(:shadow name)` in the header, the module's own definition wins inside it and `/name`
//! reaches the root one; undeclared, the definition fails compilation, naming the clause to
//! add. A function and a macro are held to the same rule. Asserted on the real binary,
//! because a refusal a program never sees run is the whole point.

use std::process::Command;

fn run(dir: &std::path::Path, main: &str) -> (bool, String, String) {
    std::fs::write(dir.join("main.blsp"), main).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(dir)
        .arg("main.blsp")
        .output()
        .expect("run brood");
    (
        output.status.success(),
        String::from_utf8_lossy_owned(output.stdout),
        String::from_utf8_lossy_owned(output.stderr),
    )
}

#[test]
fn a_root_name_shadow_compiles_only_when_declared_and_then_the_module_definition_wins() {
    let dir = std::env::temp_dir().join(format!("brood-root-shadow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Undeclared, a root FUNCTION's name: refused before anything runs, naming the clause.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user)\n\
         (defn get (m k) :mine)\n\
         (io/puts \"ran\")\n",
    );
    assert!(
        !ok && !stdout.contains("ran"),
        "an undeclared shadow must not run:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("`user` defines `get`, a name the language ships")
            && stderr.contains("(:shadow get)"),
        "the refusal must name the definition and the clause to add:\n{stderr}"
    );

    // Undeclared, a root MACRO's name: the same refusal.
    let (ok, _, stderr) = run(&dir, "(defmodule user)\n(defn- doto (x) [:mine x])\n");
    assert!(
        !ok && stderr.contains("(:shadow doto)"),
        "a macro's name is held to the same rule:\n{stderr}"
    );

    // Declared: the module's own definitions win a bare call, `/name` reaches the root ones.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user (:shadow doto get))\n\
         (defn- doto (x) [:mine x])\n\
         (defn get (m k) :mine)\n\
         (io/puts (pr-str [(doto 1) (/doto 3) (get {:a 1} :a) (/get {:a 1} :a)]))\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "a declared shadow must compile:\n{stderr}");
    assert!(
        stdout.contains("[[:mine 1] 3 :mine 1]"),
        "the module's own definitions must win a bare call, `/name` reach the root:\n{stdout}\n{stderr}"
    );
}
