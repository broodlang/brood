//! **The benchmark corpus is found by looking, not by assuming its directory name.**
//!
//! `ab-bench.sh`, `tier-audit.sh` and `jit-lower-witness.sh` each defaulted to
//! `../brood-benchmarks` — the *upstream repository's* name, which a clone is under no
//! obligation to use. On this machine it is `../brood-benchmark` (singular), so every one of
//! them reported the corpus missing. That is not a loud failure by design: a tool that needs
//! the corpus treats its absence as a **skip**, so `make green-all` still runs on a machine
//! without it. The two facts together mean **a directory name silently deleted a whole gate**
//! — `make tier-audit` printed "no benchmark checkout — skipped", the handoff recorded that as
//! "the one `make green-all` component with no local verdict", and a perf task was deferred
//! off-box, for weeks, over a trailing `s`. (Found 2026-09-20; `tier-audit` was green on all
//! 29 rows the first time it was pointed at the corpus that was there all along.)
//!
//! So this pins both halves of the fix:
//! - `scripts/bench-dir.sh` resolves EITHER spelling, prefers the canonical one, and names the
//!   canonical one when there is nothing to find (so a caller's "no checkout at …" message
//!   still tells the reader what to clone);
//! - and no consumer re-introduces a hard-coded default, which is how the class would re-open:
//!   the next tool copies the line it sees in the tool beside it.
//!
//! Deliberately NOT a test that the corpus exists — that would fail on a machine that has
//! legitimately not cloned it, which is the thing the skip exists for.
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn resolve(root: &Path) -> String {
    let out = Command::new(repo_root().join("scripts/bench-dir.sh"))
        .arg(root)
        .output()
        .expect("run bench-dir.sh");
    assert!(out.status.success(), "bench-dir.sh exited nonzero");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A sandbox holding `<work>/brood/` (the pretend repo root) plus whichever sibling
/// checkouts the case asks for.
fn sandbox(tag: &str, siblings: &[&str]) -> PathBuf {
    let work = std::env::temp_dir().join(format!("brood-bench-dir-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("brood")).expect("sandbox root");
    for s in siblings {
        std::fs::create_dir_all(work.join(s).join("bench/brood")).expect("sibling checkout");
    }
    work
}

#[test]
fn either_spelling_of_the_checkout_is_found_and_the_canonical_one_wins() {
    // Singular only — the spelling that was invisible to every tool.
    let work = sandbox("singular", &["brood-benchmark"]);
    let want = std::fs::canonicalize(work.join("brood-benchmark")).unwrap();
    assert_eq!(
        resolve(&work.join("brood")),
        want.display().to_string(),
        "a checkout named brood-benchmark must be found"
    );

    // Plural only — the name the tools used to hard-code; it must still resolve.
    let work = sandbox("plural", &["brood-benchmarks"]);
    let want = std::fs::canonicalize(work.join("brood-benchmarks")).unwrap();
    assert_eq!(resolve(&work.join("brood")), want.display().to_string());

    // Both present: prefer the canonical (plural) one, so a machine that has cloned it
    // properly is never quietly measured against some other directory.
    let work = sandbox("both", &["brood-benchmark", "brood-benchmarks"]);
    let want = std::fs::canonicalize(work.join("brood-benchmarks")).unwrap();
    assert_eq!(
        resolve(&work.join("brood")),
        want.display().to_string(),
        "with both checked out the canonical name wins"
    );

    // Neither: name the canonical location rather than printing nothing, so the caller's
    // own "no benchmark checkout at <x>" tells the reader what to clone and where.
    let work = sandbox("neither", &[]);
    let got = resolve(&work.join("brood"));
    assert!(
        got.ends_with("brood-benchmarks"),
        "with no checkout the canonical path is named, got {got}"
    );
    // A directory that exists but holds no rows is not a checkout.
    let work = sandbox("empty", &[]);
    std::fs::create_dir_all(work.join("brood-benchmark")).unwrap();
    assert!(resolve(&work.join("brood")).ends_with("brood-benchmarks"));
}

#[test]
fn no_benchmark_tool_hard_codes_the_checkout_name() {
    let root = repo_root();
    // Each tool, and the variable whose DEFAULT is the thing that must come from the
    // resolver. A hard-coded default is what made the corpus invisible; a tool that spells
    // the name in a comment, a usage line or an error message is fine and expected.
    let tools = [
        ("scripts/ab-bench.sh", "bench_dir="),
        ("scripts/tier-audit.sh", "BENCH="),
        ("scripts/jit-lower-witness.sh", "ROWS_DIR="),
    ];
    for (tool, assignment) in tools {
        let text = std::fs::read_to_string(root.join(tool)).expect(tool);
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with(assignment))
            .unwrap_or_else(|| panic!("{tool} no longer assigns {assignment}"));
        assert!(
            line.contains("bench-dir.sh"),
            "{tool} must take its default from scripts/bench-dir.sh, or a checkout under \
             another name is invisible to it and it SKIPS rather than failing:\n  {line}"
        );
    }
}
