//! **A test run must say whether it used the stdlib image.**
//!
//! The image is default-on and its fallback is silent by design — with none on disk
//! `require` simply reads source, which is correct. The cost of that silence is what this
//! test protects: the source path is ~5x slower on the full suite (91 s → 470 s measured)
//! and is a documented amplifier for the KI-89 isolate race, where deleting the images took
//! a green 5514-test run to 106 failures. So a run can be slow and red for a reason that has
//! nothing to do with the change under test, and until 2026-09-04 nothing printed it — the
//! question "did this run use the image?" cost a full day to answer by experiment.
//!
//! What is asserted is the line in the suite summary, end to end through a real `brood`
//! process, in both directions. Both matter: a line that only ever says "none" is as useless
//! as no line, and one that only ever says "N sections" is worse, because it would read as
//! reassurance on exactly the runs that need the warning.
//!
//! Each case gets its own `XDG_CACHE_HOME`, so it is testing the state it set up rather than
//! whatever this developer's machine happens to have cached.

use std::path::PathBuf;
use std::process::Command;

mod support;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

/// A one-assertion suite file — the summary line is what is under test, not the tests.
const SUITE: &str = "(defmodule stdimage-line-test (:use test))\n\
(describe \"trivial\"\n\
\x20\x20(test \"passes\" (assert= 1 1)))\n";

/// Run `brood --test` in `dir` against `cache`, returning stdout+stderr.
fn run_suite(dir: &TempDir, cache: &std::path::Path, no_image: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("--test")
        .arg("suite.blsp")
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", cache);
    if no_image {
        cmd.env("BROOD_NO_STDIMAGE", "1");
    } else {
        cmd.env_remove("BROOD_NO_STDIMAGE");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood --test");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Write this binary's stdlib image into `cache`. The RUNTIME never builds one (~1 s, which
/// would land on exactly the short-lived runs the image exists to speed up), but the builder
/// is reachable in-language, which is what lets this test set up the positive case without
/// needing `nest`.
fn build_image(dir: &TempDir, cache: &std::path::Path) {
    let prog = dir.path.join("build-image.blsp");
    std::fs::write(&prog, "(require-one 'stdimage) (stdimage/build)\n").expect("write builder");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("build-image.blsp")
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", cache);
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run the image builder");
    assert!(
        out.status.success(),
        "building the image should succeed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn the_suite_summary_says_whether_this_run_used_the_stdlib_image() {
    let dir = temp_dir("stdimage-line");
    std::fs::write(dir.path.join("suite.blsp"), SUITE).expect("write suite");

    // 1. No image anywhere. The line must say so, and say the run read source — this is the
    //    state a developer is in immediately after any `std/` edit.
    let cold = dir.path.join("cache-cold");
    std::fs::create_dir_all(&cold).expect("create cache");
    let text = run_suite(&dir, &cold, false);
    assert!(
        text.contains("stdlib image:"),
        "the summary must report the image state:\n{text}"
    );
    assert!(
        text.contains("SOURCE"),
        "with no image on disk the run loads std/ from source and must say so:\n{text}"
    );

    // 2. Same cache, now with an image in it. The line must flip, and carry the section
    //    count — a bare "yes" would not distinguish a full image from a truncated one.
    build_image(&dir, &cold);
    let text = run_suite(&dir, &cold, false);
    assert!(
        !text.contains("SOURCE"),
        "with a current image the run must NOT report the source path:\n{text}"
    );
    assert!(
        text.contains("stdlib image:") && text.contains("sections"),
        "an imaged run must report how many sections it installed:\n{text}"
    );

    // 3. The image is still on disk and current; this run declines it. This is the case the
    //    on-disk state cannot answer — `(stdimage/status)`'s `:state` reads `:live` here —
    //    and getting it wrong is how a measurement gets attributed to the wrong arm.
    let text = run_suite(&dir, &cold, true);
    assert!(
        text.contains("SOURCE"),
        "BROOD_NO_STDIMAGE=1 must report the source path even with a live image on disk:\n{text}"
    );
}
