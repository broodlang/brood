//! A thin wrapper whose inner head is a not-yet-loaded qualified name must not stale its
//! caller's arguments.
//!
//! The tree-walker's thin-wrapper elision (`passthrough_arm`) resolves the wrapper's inner
//! head and forwards the already-evaluated `argv` to it. The resolution was a full `eval`,
//! under a comment saying a symbol lookup "cannot GC" — true until lazy module loading
//! (ADR-335) made an unbound qualified name a module LOAD, i.e. arbitrary evaluation and a
//! collection. The arguments were not rooted across it, so the redirect bound relocated
//! handles into the inner call's frame. Found as two `nest` tests failing intermittently
//! under the tree-walker suite (`BROOD_VM=0`, a `bytes` argument reaching `seq`); with
//! `BROOD_GC_STRESS=1` it is deterministic, which is what this pins.
//!
//! Sabotage-verified: with the rooted slow path removed, this reports
//! `use-after-GC: pair handle … held across a collection` (debug-assertion builds).

use std::process::Command;

#[test]
fn a_lazily_loaded_passthrough_head_keeps_the_arguments_rooted() {
    let dir = std::env::temp_dir().join(format!("brood-passthrough-lazy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("pt.blsp");
    // `wrap-encode` is a pure pass-through, so the tree-walker elides it; `json` is not
    // loaded until that elision resolves `json/encode`. The argument is freshly consed
    // LOCAL data, which the load's collection relocates.
    std::fs::write(
        &file,
        "(defn wrap-encode (x) (json/encode x))\n\
         (io/puts (wrap-encode (list (str \"a\" 1) (str \"b\" 2) [1 2 {:k (str \"v\" 3)}])))\n",
    )
    .expect("write probe");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg(&file)
        .env("BROOD_VM", "0")
        .env("BROOD_GC_STRESS", "1")
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains(r#"["a1","b2",[1,2,{"k":"v3"}]]"#),
        "the elided call must see its arguments intact\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}
