//! `(:use m)` warns when an import shadows a root/prelude global a program could reach —
//! and only then. A module-PRIVATE root name (the prelude's `def-` `*live-registries*`) is
//! not reachable outside its module, so importing a public name spelled the same shadows
//! nothing: hatch exports a `*live-registries*` and every `(:use hatch/web/live)` printed a
//! warning about a name its user could never have referred to.

use std::process::Command;

#[test]
fn an_import_warns_for_a_public_root_name_and_not_for_a_private_one() {
    let dir = std::env::temp_dir().join(format!("brood-import-shadow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shadowlib.blsp"),
        "(defmodule shadowlib (:shadow identity))\n(def *live-registries* 1)\n(defn identity (x) x)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.blsp"),
        "(defmodule user (:use shadowlib))\n(io/puts \"ran\")\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(&dir)
        .arg("main.blsp")
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("ran"), "the program did not run:\n{stderr}");
    assert!(
        stderr.contains("refers `identity`, which shadows"),
        "a public prelude name must still warn:\n{stderr}"
    );
    assert!(
        !stderr.contains("`*live-registries*`, which shadows"),
        "a private prelude name is unreachable, so nothing is shadowed:\n{stderr}"
    );
}
