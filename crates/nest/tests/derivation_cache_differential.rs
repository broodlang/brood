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
        // Pin the ceiling rather than inherit the `differential (tree-walker)` job's
        // `BROOD_VM=0`: whether a memo changes an answer is engine-independent. And say
        // the size, since the other tree-walker overrides in `.config/nextest.toml`
        // describe 5-10x ratios and this is not one: measured 2026-09-17 on a 28-core
        // box, one whole-tree check is 9.1 s (`std/`) and 12.1 s (`tests/`) under the VM
        // against 9.1 s and 14.2 s under the tree-walker — ~17% at most. So the engine is
        // NOT why this binary timed out at the 120 s cap; four whole-tree checks in one
        // case is. The per-tree split below is the fix, this is the cheap 17% beside it.
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

// One tree per CASE, not both in one, because each whole-tree check is 9-12 s and nextest's
// cap is per case: as one case this did four of them and timed out at 120 s on the 2-core
// `differential (tree-walker)` runner (2026-09-17). Split, each case does two and the two
// run in parallel — the same coverage, both trees, no budget raised.

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
