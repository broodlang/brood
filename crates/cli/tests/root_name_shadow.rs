//! A module defining a name the language ships (ADR-784): the module's own definition wins
//! inside it, with no declaration, and `/name` reaches the root one. A function and a macro
//! are held to the same rule. Asserted on the real binary, since what matters is that the
//! program compiles and runs.

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
fn a_module_definition_of_a_root_name_shadows_it_with_no_declaration() {
    let dir = std::env::temp_dir().join(format!("brood-root-shadow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // A root macro's name and a root function's name, defined with no header clause: the
    // module's own definitions win a bare call, `/name` reaches the root ones.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user)\n\
         (defn- doto (x) [:mine x])\n\
         (defn get (m k) :mine)\n\
         (io/puts (pr-str [(doto 1) (/doto 3) (get {:a 1} :a) (/get {:a 1} :a)]))\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ok,
        "a module's definition of a root name must compile:\n{stderr}"
    );
    assert!(
        stdout.contains("[[:mine 1] 3 :mine 1]"),
        "the module's own definitions must win a bare call, `/name` reach the root:\n{stdout}\n{stderr}"
    );
    assert!(
        !stderr.contains("shadow"),
        "nothing to say about a definition in plain sight:\n{stderr}"
    );
}
