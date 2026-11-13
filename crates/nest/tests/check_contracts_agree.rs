//! **`nest check` answers the same with contracts armed (KI-384).** Under
//! `BROOD_CONTRACTS=1` a `sig`'d global holds its contract shim, whose body only checks and
//! forwards. The checker inferred a function's result from whatever closure the global held,
//! so with contracts armed it read `os/shell-spawn-pty` (typed parameter, open return) as
//! returning `any` instead of a subprocess. A `vector?` test narrowed that to
//! `(not vector)`, and once the value travelled through a map built by a same-file entry
//! point, `os/write` reported `argument 1 expects subprocess, got (not (nil | vector |
//! false))`. Unarmed, the same file was clean. bedit's `terminal.blsp` showed four of these.
//!
//! The project below is that shape reduced: `connect` builds the state, `boot` narrows the
//! spawn result, `write` reads it back out of the map.

use std::path::Path;
use std::process::Command;

mod support;
use support::temp_dir;

fn check(dir: &Path, cache: &Path, contracts: &str) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(["check", "src/app.blsp"])
        .env("XDG_CACHE_HOME", cache)
        .env("BROOD_CONTRACTS", contracts)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest check");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

#[test]
fn a_shimmed_callee_is_inferred_from_its_original_body() {
    let dir = temp_dir("check-contracts-agree");
    let root = &dir.path;
    let cache = root.join("cache");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"kfour\" :source-paths [\"src\"])\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/app.blsp"),
        "(defmodule app)\n\
         (defn- write (state bytes)\n\
         \x20\x20(let (proc (:proc state)) (when proc (os/write proc bytes))))\n\
         (defn- serve (state)\n\
         \x20\x20(receive\n\
         \x20\x20\x20\x20([:closed] (serve (assoc state :proc nil)))\n\
         \x20\x20\x20\x20([:key k] (do (write state k) (serve state)))))\n\
         (defn- boot (state)\n\
         \x20\x20(let (spawned (try (os/shell-spawn-pty (:command state) {}) (catch e [:refused (str e)])))\n\
         \x20\x20\x20\x20(if (vector? spawned) nil (serve (assoc state :proc spawned)))))\n\
         (defn connect (command) (spawn (boot {:command command})))\n",
    )
    .unwrap();
    for contracts in ["0", "1"] {
        let (out, ok) = check(root, &cache, contracts);
        assert!(
            ok && !out.contains("expects subprocess"),
            "BROOD_CONTRACTS={contracts}: nest check must be clean, as it is unarmed:\n{out}"
        );
    }
}
