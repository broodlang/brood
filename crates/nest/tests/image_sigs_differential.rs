//! The stdlib image's signature footer (ADR-370) is inert to the checker's VERDICTS: `nest
//! check` reading a callee's type from the image must say the same thing about every file in
//! the tree as `nest check` loading the callee's module and reading its declaration — every
//! warning AND every inferred signature (`--suggest-sigs`), byte for byte. The fallback
//! (`BROOD_NO_IMAGE_SIGS=1`) is the path the checker took before the footer existed.
//!
//! Why the tree rather than a fixture: what the footer may carry is a claim about every
//! signature shape std declares, and the first two runs of this gate (2026-09-18) each
//! found a shape it could not: an INFERRED signature re-rendered from source read wider
//! than the live inference (eleven new `nil | string` warnings), and a declared `-> any`
//! is "no declaration" to the call-site typing, which re-types the loaded body — so with
//! the module unloaded `observer/observe-order` read `-> any` where the body says `(list
//! any)`. Both are now excluded at write time (`check::image_carried_sig`); this is what
//! keeps every such case excluded. Two whole-tree checks per case, one case per tree, as
//! `derivation_cache_differential` does and for the same reason (nextest's per-case cap).

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
/// together, with the image's signatures read (`image_sigs`) or ignored.
fn check(root: &Path, files: &[PathBuf], image_sigs: bool) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nest"));
    command
        .current_dir(root)
        .arg("check")
        .arg("--strict")
        .arg("--suggest-sigs")
        .args(files)
        .env("BROOD_NO_CHECK_CACHE", "1")
        // Engine-independent question; do not inherit the tree-walker job's `BROOD_VM=0`.
        .env("BROOD_TIER", "2");
    if !image_sigs {
        command.env("BROOD_NO_IMAGE_SIGS", "1");
    }
    let out = command.output().expect("run nest");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // The first `nest` in a fresh checkout builds the project image and says so — a one-time
    // line that is about the build, not the check.
    text.lines()
        .filter(|l| !l.starts_with("Building brood") && !l.starts_with("Built brood"))
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn image_signatures_change_no_verdict_over_std() {
    image_signatures_change_no_verdict_over("std");
}

#[test]
fn image_signatures_change_no_verdict_over_tests() {
    image_signatures_change_no_verdict_over("tests");
}

fn image_signatures_change_no_verdict_over(tree: &str) {
    let root = workspace_root();
    let files = blsp_files(&root.join(tree));
    assert!(!files.is_empty(), "no .blsp under {tree}/");
    let with_image = check(&root, &files, true);
    let loaded = check(&root, &files, false);
    assert!(
        with_image.contains("(sig "),
        "{tree}: the suggestion listing is missing — the run did not reach the checker:\n{with_image}"
    );
    if with_image != loaded {
        let first = with_image
            .lines()
            .zip(loaded.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        let show = |text: &str| {
            text.lines()
                .skip(first.saturating_sub(1))
                .take(8)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "{tree}: reading signatures from the image changed a verdict (first difference at \
             line {}).\n--- image signatures read:\n{}\n--- modules loaded (BROOD_NO_IMAGE_SIGS=1):\n{}",
            first + 1,
            show(&with_image),
            show(&loaded)
        );
    }
}
