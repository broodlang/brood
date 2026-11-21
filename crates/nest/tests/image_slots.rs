//! A project keeps its startup image and its check verdicts per BUILD: `.brood/image-<slot>.*`
//! and `<manifest>-<slot>.blsp` under the check cache, the `*slots-kept*` (3) most recently
//! USED of each, so a checkout used from two toolchains stays warm for both.
//!
//! With one file of each, an installed `nest` and one built from a working tree (or a
//! profiling build, or a walk flag such as `BROOD_TIER=1`) each threw the other's away: every
//! switch rebuilt bedit's image and re-checked its entry closure, 4 s and 26 s of CPU, and a
//! profile taken with a second binary measured that rebuild instead of the start.
//!
//! A build here is one copy of `nest` whose mtime the test sets before each run: the build id
//! is `version+sha+binary mtime`, read once per process. Four builds against three slots:
//!
//! 1. A, B, C each start cold and each write a slot of their own;
//! 2. A again is served from its slot — the image file is not rewritten (same inode) — and
//!    writes no new verdict manifest;
//! 3. D evicts B, the least recently USED: A's slot and manifest survive because the warm run
//!    touched them, though A WROTE first.
//!
//! **Sabotage-verified**: a single image path fails step 1; dropping either touch (the image's
//! or the manifest's) evicts A instead of B in step 3.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

mod support;
use support::temp_dir;

fn fixture(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project\n  :name slots\n  :main main)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/helper.blsp"),
        "(defmodule helper)\n(defn base () 40)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/main.blsp"),
        "(defmodule main (:use helper))\n(defn main () (io/puts (str \"ANSWER \" (+ (base) 2))))\n",
    )
    .unwrap();
}

/// Run `nest` as build `stamp` (the copy's mtime, in seconds) in `project`.
fn run_as(binary: &Path, stamp: u64, project: &Path, cache: &Path) {
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(stamp);
    std::fs::File::options()
        .write(true)
        .open(binary)
        .and_then(|file| file.set_modified(when))
        .expect("stamp the build");
    let out = Command::new(binary)
        .arg("run")
        .current_dir(project)
        .env("XDG_CACHE_HOME", cache)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .output()
        .expect("spawn nest");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && text.contains("ANSWER 42"),
        "build {stamp} did not run the project:\n{text}"
    );
}

/// The file names in `dir` that start with `prefix` and end with `suffix`.
fn names(dir: &Path, prefix: &str, suffix: &str) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(prefix) && name.ends_with(suffix))
                .collect()
        })
        .unwrap_or_default()
}

/// The one name `after` has that `before` did not.
fn added(before: &BTreeSet<String>, after: &BTreeSet<String>) -> String {
    let new: Vec<&String> = after.difference(before).collect();
    assert_eq!(
        new.len(),
        1,
        "expected one new file: {before:?} -> {after:?}"
    );
    new[0].clone()
}

/// The project's check-cache directory (one per root, `check/<sha256 of root>`).
fn check_dir(cache: &Path) -> PathBuf {
    let checks = cache.join("brood").join("check");
    std::fs::read_dir(&checks)
        .expect("the check cache")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .expect("the project's check cache")
}

#[test]
fn each_build_keeps_its_own_image_and_verdicts_and_the_least_recently_used_is_evicted() {
    let tmp = temp_dir("image-slots");
    let project = tmp.path.join("proj");
    let cache = tmp.path.join("cache");
    let binary = tmp.path.join("nest");
    fixture(&project);
    std::fs::copy(env!("CARGO_BIN_EXE_nest"), &binary).expect("copy the binary");
    let images = project.join(".brood");
    let (build_a, build_b, build_c, build_d) =
        (1_700_000_000, 1_700_000_100, 1_700_000_200, 1_700_000_300);

    // 1. Three builds, three slots of each kind.
    let mut slot_of = Vec::new();
    let mut manifest_of = Vec::new();
    for stamp in [build_a, build_b, build_c] {
        let images_before = names(&images, "image-", ".bin");
        let manifests_before = if cache.join("brood").join("check").exists() {
            names(&check_dir(&cache), "checks-run-", ".blsp")
        } else {
            BTreeSet::new()
        };
        run_as(&binary, stamp, &project, &cache);
        slot_of.push(added(&images_before, &names(&images, "image-", ".bin")));
        manifest_of.push(added(
            &manifests_before,
            &names(&check_dir(&cache), "checks-run-", ".blsp"),
        ));
    }
    let checks = check_dir(&cache);

    // 2. Build A again: served from its own slot, nothing rewritten or added.
    let inode = |name: &str| {
        std::fs::metadata(images.join(name))
            .expect("the slot")
            .ino()
    };
    let a_inode = inode(&slot_of[0]);
    let manifests_before = names(&checks, "checks-run-", ".blsp");
    run_as(&binary, build_a, &project, &cache);
    assert_eq!(
        inode(&slot_of[0]),
        a_inode,
        "build A rebuilt its image although its slot was there — B or C replaced it"
    );
    assert_eq!(
        names(&checks, "checks-run-", ".blsp"),
        manifests_before,
        "build A wrote a new verdict manifest instead of reading its own"
    );

    // 3. A fourth build evicts B, the least recently used, not A, the oldest written.
    run_as(&binary, build_d, &project, &cache);
    let slots = names(&images, "image-", ".bin");
    assert_eq!(slots.len(), 3, "three slots are kept: {slots:?}");
    assert!(
        slots.contains(&slot_of[0]),
        "A's slot was evicted although A ran last but one: {slots:?}"
    );
    assert!(
        !slots.contains(&slot_of[1]),
        "B's slot, the least recently used, survived: {slots:?}"
    );
    let b_stem = slot_of[1].trim_end_matches(".bin");
    for suffix in [".modules", ".includes"] {
        assert!(
            !images.join(format!("{b_stem}{suffix}")).exists(),
            "B's {suffix} file outlived its image"
        );
    }
    let manifests = names(&checks, "checks-run-", ".blsp");
    assert_eq!(
        manifests.len(),
        3,
        "three verdict manifests are kept: {manifests:?}"
    );
    assert!(
        manifests.contains(&manifest_of[0]),
        "A's manifest was evicted: {manifests:?}"
    );
    assert!(
        !manifests.contains(&manifest_of[1]),
        "B's manifest survived: {manifests:?}"
    );
}
