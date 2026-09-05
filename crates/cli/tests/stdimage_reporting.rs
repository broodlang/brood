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
    run_suite_env(dir, cache, no_image, &[])
}

/// `run_suite` with extra environment — the prelude-boot cases need to turn the prelude
/// image off without disturbing the stdlib-image cases beside them.
fn run_suite_env(
    dir: &TempDir,
    cache: &std::path::Path,
    no_image: bool,
    extra: &[(&str, &str)],
) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("--test")
        .arg("suite.blsp")
        .current_dir(&dir.path)
        .env("XDG_CACHE_HOME", cache)
        // Clear every artifact switch, in BOTH spellings, before applying this case's own.
        // An inherited one decides the answer otherwise, and CI is where that bites: the
        // tree-walker job sets `BROOD_NO_PRELUDE_IMAGE=1` and `BROOD_NO_STDIMAGE=1` for the
        // whole run, so a child that inherits them takes the text-cache path and a case
        // asserting "this run used the image" fails for a reason that has nothing to do
        // with the code. Owning `XDG_CACHE_HOME` is only half of owning the state; the
        // other half is the environment, and the prelude differential beside this one
        // already says so in as many words.
        .env_remove("BROOD_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_PRELUDE_IMAGE")
        .env_remove("BROOD_NO_BOOT_CACHE");
    for (k, v) in extra {
        cmd.env(k, v);
    }
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

/// **A run must also say how its PRELUDE arrived**, which is the other half of "which
/// artifacts did this run use?" and the half that has cost the most.
///
/// The prelude has three boot paths — the image (ADR-314), the expanded-text cache
/// (ADR-138), and a cold source boot that writes both — and which one runs is decided by
/// whether artifacts keyed on `build-id` already exist. Since `build-id` embeds the
/// binary's mtime, **the first run after any rebuild is a source boot and every run after
/// it is not**. That is precisely the moment someone is checking whether an image change
/// worked, so the un-imaged path gets read as evidence about the imaged one: three separate
/// "it is fixed" readings during KI-106 were cold boots, and ADR-314 records the same trap
/// corrupting a diagnosis in a session that had already been caught by it twice.
///
/// All three states are asserted. A line that can only ever print one of them would be
/// worse than none, because it would read as an answer.
#[test]
fn the_suite_summary_says_how_the_prelude_arrived() {
    let dir = temp_dir("prelude-line");
    std::fs::write(dir.path.join("suite.blsp"), SUITE).expect("write suite");
    let cache = temp_dir("prelude-line-cache");

    // 1. Nothing cached for this binary yet, so the prelude is read and evaluated — and
    //    this run is what WRITES the two artifacts the next one will use.
    let cold = run_suite(&dir, &cache.path, false);
    assert!(
        cold.contains("(prelude: SOURCE"),
        "a first run against an empty cache is a cold boot and must say so:\n{cold}"
    );

    // 2. Same binary, same cache, second run: the image written above is now current.
    let warm = run_suite(&dir, &cache.path, false);
    assert!(
        warm.contains("(prelude: image)"),
        "a second run must report the prelude image it just gained:\n{warm}"
    );

    // 3. The image declined, so the boot falls back to the expanded-text cache the cold
    //    run also wrote. Distinguishing these two is the point: both are "warm", and only
    //    one of them is exercising ADR-314.
    let text = run_suite_env(&dir, &cache.path, false, &[("BROOD_NO_PRELUDE_IMAGE", "1")]);
    assert!(
        text.contains("(prelude: expanded-text cache)"),
        "with the prelude image off the boot must name the text cache, not the image:\n{text}"
    );
}
