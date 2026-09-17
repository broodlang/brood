//! The checker's site-walk cache (`sigs::SiteCache`) is inert: `nest check` with it and
//! without it must say the same thing about every file in the tree — every warning AND
//! every inferred signature (`--suggest-sigs`), since a derivation that reads a stale
//! scope would show first as a return that moved, not as a warning.
//!
//! Why the tree rather than a fixture: the cache's premise — a form's walk depends only
//! on its own derived parameters and on what its reference closure reaches — is a claim
//! about every shape the walker meets, and the first differential (2026-09-17) found the
//! joint fixpoint was not even a function of its inputs: the specialization memo outlived
//! the round it was typed under, and the returns were re-read in hash order, so the SAME
//! file settled on `(int 0 2)` five runs out of six and `0 | 1 | 2` on the sixth. Both
//! fixed; this is what keeps them fixed. Runs the checker four times in all — two per tree,
//! one case per tree — and is worth it: a cache that changes a verdict is a checker bug
//! nothing else would catch.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn blsp_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "blsp") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// `nest check --strict --suggest-sigs FILES…` from the workspace root, stdout and stderr
/// together (the cache changes no exit code either, but the text is the finer assertion).
fn check(root: &Path, files: &[PathBuf], cache: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nest"));
    command
        .current_dir(root)
        .arg("check")
        .arg("--strict")
        .arg("--suggest-sigs")
        .args(files)
        .env("BROOD_NO_CHECK_CACHE", "1")
        // Engine-independent question, so the child does not inherit the tree-walker job's
        // `BROOD_VM=0` (four whole-tree checks: 52 s solo on the VM, ~1.6x tree-walked).
        // That 1.6x is this test's child, the `[profile.test]` `nest`; the same check on
        // the RELEASE binary is ~17% (9.1/12.1 s VM against 9.1/14.2 s tree-walked), since
        // the release JIT is the other side of the comparison. Read a ratio with its binary.
        .env("BROOD_TIER", "2");
    if !cache {
        command.env("BROOD_NO_DERIVE_CACHE", "1");
    }
    let out = command.output().expect("run nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// One tree per CASE, not both in one, because nextest's cap is per case and this did four
// whole-tree checks in one — which is what put it over the 120 s default (2026-09-17). Each
// case now does two, and the two run in parallel: 21 s and 29 s under the tree-walker job's
// own environment, against 51 s for the single case. The `.config/nextest.toml` budget is
// the backstop and was sized before this split, so it now has about twice the headroom its
// comment claims. Coverage is identical — both trees, both cache modes.

#[test]
fn the_site_walk_cache_changes_no_verdict_over_std() {
    the_site_walk_cache_changes_no_verdict_over("std");
}

#[test]
fn the_site_walk_cache_changes_no_verdict_over_tests() {
    the_site_walk_cache_changes_no_verdict_over("tests");
}

fn the_site_walk_cache_changes_no_verdict_over(tree: &str) {
    let root = workspace_root();
    let files = blsp_files(&root.join(tree));
    assert!(!files.is_empty(), "no .blsp under {tree}/");
    let cached = check(&root, &files, true);
    let uncached = check(&root, &files, false);
    assert!(
        cached.contains("(sig "),
        "{tree}: the suggestion listing is missing — the run did not reach the checker:\n{cached}"
    );
    if cached != uncached {
        let first = cached
            .lines()
            .zip(uncached.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let show = |text: &str| {
            text.lines()
                .skip(first.saturating_sub(1))
                .take(4)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "{tree}: the site-walk cache changed the checker's answer at line {}:\n--- cached\n{}\n--- uncached\n{}",
            first + 1,
            show(&cached),
            show(&uncached)
        );
    }
}
