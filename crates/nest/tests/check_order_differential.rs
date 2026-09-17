//! The checker's answer for a file is a function of the file (B8, 2026-09-17): neither the
//! ORDER the files were given in nor the PROCESS the checker ran in may change a warning
//! or an inferred signature.
//!
//! Two channels can break that and neither shows in a unit test. A `HashMap`/`HashSet`
//! iterated where its order reaches a verdict — which name a warning picks, the order two
//! findings print in, the order modules load in — varies with the process (`RandomState`
//! is seeded per process). And the order files are checked in decides the order symbols
//! are interned in, which is the order the symbol-keyed tables iterate in, and what the
//! memos and the module-load side effects hold when a later file is reached. KI-158 found
//! both channels by accident; this is the deliberate gate. Same tree, two processes, one of
//! them in reverse order: every file's lines must agree, in sequence.

use std::collections::BTreeMap;
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

fn check(root: &Path, files: &[PathBuf]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(root)
        .arg("check")
        .arg("--strict")
        .arg("--suggest-sigs")
        .args(files)
        .env("BROOD_NO_CHECK_CACHE", "1")
        .output()
        .expect("run nest");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The output split per file, each file's lines in the order they were printed. A
/// `--suggest-sigs` block is a bare path line followed by indented `(sig …)` lines; a
/// warning is `path:line:col: warning: …` (or `path: note: …`). Anything else — the
/// summary counts, a load-time message, a blank — belongs to no file and is compared as a
/// set (it follows whichever file came last, which the order changes by design).
fn per_file(text: &str, files: &[PathBuf]) -> BTreeMap<String, Vec<String>> {
    let paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current = String::from("<no file>");
    for line in text.lines() {
        if let Some(path) = paths.iter().find(|p| line == *p) {
            current = path.clone();
            continue;
        }
        if let Some(path) = paths.iter().find(|p| line.starts_with(&format!("{p}:"))) {
            out.entry(path.clone()).or_default().push(line.to_string());
            continue;
        }
        let owner = if line.starts_with("  ") {
            current.clone()
        } else {
            String::from("<no file>")
        };
        out.entry(owner).or_default().push(line.to_string());
    }
    out
}

fn assert_same_per_file(what: &str, a: &str, b: &str, files: &[PathBuf]) {
    let a = per_file(a, files);
    let b = per_file(b, files);
    for path in a.keys().chain(b.keys()) {
        let (left, right) = (a.get(path), b.get(path));
        if path == "<no file>" {
            // Global lines (the file count, the image line): order-free.
            let mut left: Vec<&String> = left.into_iter().flatten().collect();
            let mut right: Vec<&String> = right.into_iter().flatten().collect();
            left.sort();
            right.sort();
            assert_eq!(left, right, "{what}: the lines belonging to no file differ");
            continue;
        }
        if left != right {
            let first = left
                .into_iter()
                .flatten()
                .zip(right.into_iter().flatten())
                .position(|(x, y)| x != y)
                .unwrap_or(0);
            let show = |lines: Option<&Vec<String>>| {
                lines
                    .into_iter()
                    .flatten()
                    .skip(first.saturating_sub(1))
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            panic!(
                "{what}: {path} answered differently at its line {}:\n--- first\n{}\n--- second\n{}",
                first + 1,
                show(left),
                show(right)
            );
        }
    }
}

#[test]
fn a_files_verdict_depends_on_neither_the_list_order_nor_the_process() {
    let root = workspace_root();
    let files = blsp_files(&root.join("tests"));
    assert!(!files.is_empty(), "no .blsp under tests/");
    let forward = check(&root, &files);
    assert!(
        forward.contains("(sig "),
        "the suggestion listing is missing — the run did not reach the checker:\n{forward}"
    );
    let again = check(&root, &files);
    assert_same_per_file("two processes, same order", &forward, &again, &files);
    let mut reversed = files.clone();
    reversed.reverse();
    let backward = check(&root, &reversed);
    assert_same_per_file("forward vs reversed order", &forward, &backward, &files);
}
