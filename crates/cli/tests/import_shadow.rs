//! An import spelled like a name the language ships (ADR-750, ADR-784): a bare use of it fails
//! compilation unless the importer names it in `:only`, the error naming both meanings and
//! the way out; an importer that never writes the name bare compiles. A module-PRIVATE
//! root name (the prelude's `def-` `*live-registries*`) is unreachable outside its module, so
//! a public name spelled the same shadows nothing: hatch exports a `*live-registries*`, and
//! its users write it bare. Asserted on the real binary.

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
fn a_bare_use_of_an_import_spelled_like_a_root_name_needs_it_named_in_only() {
    let dir = std::env::temp_dir().join(format!("brood-import-shadow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shadowlib.blsp"),
        "(defmodule shadowlib)\n(def *live-registries* 1)\n(defn identity (x) [:mine x])\n",
    )
    .unwrap();

    // Imported, never written bare: compiles and runs, and says nothing.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user (:use shadowlib))\n(io/puts \"ran\")\n",
    );
    assert!(
        ok && stdout.contains("ran"),
        "an unused shadowing import must compile:\n{stderr}"
    );
    assert!(
        !stderr.contains("identity"),
        "nothing to say about an unused import:\n{stderr}"
    );

    // Written bare, undeclared: refused before anything runs, naming both and the clause.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user (:use shadowlib))\n(io/puts (pr-str (identity 1)))\n(io/puts \"ran\")\n",
    );
    assert!(
        !ok && !stdout.contains("ran"),
        "a bare use not named in :only must not run:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("`user` uses `identity` bare, and both an import (`shadowlib/identity`)")
            && stderr.contains(":only [identity]"),
        "the refusal must name both meanings and the clause:\n{stderr}"
    );

    // Named in `:only`: the import wins, `/identity` reaches the root one; a private root name's
    // public namesake needs nothing.
    let (ok, stdout, stderr) = run(
        &dir,
        "(defmodule user (:use shadowlib :only [identity *live-registries*]))\n\
         (io/puts (pr-str [(identity 1) (/identity 2) *live-registries*]))\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "an import named in :only must compile:\n{stderr}");
    assert!(
        stdout.contains("[[:mine 1] 2 1]"),
        "the import must win, `/name` reach root:\n{stdout}\n{stderr}"
    );
}
