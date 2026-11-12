//! **Every baked-in std module loads, one after another, in one process, on the native tier.**
//!
//! Nothing else asserts it. Each place that loads the whole library swallows a module that
//! fails — `stdimage/build` (a module that will not load stays out of the image), the
//! surface ledger, the image-vs-source differential — so a module that stops loading reads
//! as a smaller library, not as a failure. The first cut of the optional-argument fast frame
//! (2026-11-12, caught before it was pushed) did exactly that: thirteen modules raised
//! `<=: expected number, got pair …` while loading, a local image build dropped from 8660
//! bindings to 7411, and every test still passed.
//!
//! Loaded from SOURCE (`BROOD_NO_STDIMAGE=1`), so each module's top-level forms actually
//! run, in a fresh cache, at `BROOD_TIER=2`: the load sequence is long enough for the
//! loader's own helpers to go native and to take the native call paths between them.

use std::process::Command;

mod support;
use support::{repo_root, temp_dir};

#[test]
fn every_builtin_module_loads_in_one_process_on_the_native_tier() {
    let dir = temp_dir("every-module-loads");
    // The one program CI's release job also runs (see its header).
    let prog = repo_root().join("scripts/load-every-module.blsp");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", dir.path.join("cache"))
        .env("BROOD_TIER", "2")
        .env("BROOD_NO_STDIMAGE", "1")
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env_remove("BROOD_CONTRACTS")
        .arg(&prog)
        .output()
        .expect("run brood");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The summary line PRESENT, not failures absent: a runner that died prints neither.
    assert!(
        stdout.contains(" modules, "),
        "the load loop did not finish:\n{stdout}\n{stderr}"
    );
    assert!(
        out.status.success() && stdout.contains(", 0 failed"),
        "std modules failed to load:\n{stdout}\n{stderr}"
    );
}
