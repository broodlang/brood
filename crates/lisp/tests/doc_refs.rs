//! Every `ADR-NNN` / `KI-N` cross-reference in the tree must resolve to a real entry.
//!
//! Why this exists. The docs are the load-bearing part of this repo — `decisions.md` is
//! cited from source comments, `known-issues.md` is cited from tests and commit messages,
//! and a reader (or an assistant) follows those pointers to find out *why* the code is
//! shaped the way it is. A pointer to nothing is worse than no pointer: it reads as
//! authoritative and costs a search to disprove.
//!
//! It has already happened. Found 2026-08-17:
//!   * `docs/handoff.md` cited **ADR-202** for the atomic-registry work — a number the
//!     sequence skips entirely (200, 201, 203, …). The work is real and documented in
//!     KI-22/23; the ADR was never written.
//!   * **KI-41** had an index row but no section, while that file's own header tells the
//!     reader to "⌘F the `KI-N` to jump".
//!
//! Both are the same class as the stale `length` sig and the 13 phantom doc-catalogue
//! entries fixed the same day: a name that refers to something which does not exist, with
//! nothing checking. Cheap to assert, so assert it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

/// Every file we scan for *references*: the docs themselves plus the source that cites them.
fn scanned_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("docs"), root.join("crates"), root.join("std")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target/` holds generated copies and is huge; `fuzz/` holds corpora.
                if path
                    .file_name()
                    .is_some_and(|n| n == "target" || n == "corpus" || n == "artifacts")
                {
                    continue;
                }
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e == "md" || e == "rs" || e == "blsp")
            {
                // Skip this file: it names the very dangling references it exists to
                // prevent, so including it would make the test fail on its own docstring.
                if path.file_name().is_some_and(|n| n == "doc_refs.rs") {
                    continue;
                }
                out.push(path);
            }
        }
    }
    out
}

/// `ADR-123` / `KI-7` occurrences in `text`, as (prefix, number).
fn refs_in(text: &str, prefix: &str) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    for (idx, _) in text.match_indices(prefix) {
        let rest = &text[idx + prefix.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        // A longer number must not be truncated (`ADR-2270` is not `ADR-227`).
        if let Ok(n) = digits.parse::<u32>() {
            found.insert(n);
        }
    }
    found
}

/// The numbers that actually have an entry, from `## ADR-N` / `## KI-N` headings.
fn defined(root: &Path, files: &[&str], prefix: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for rel in files {
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("## ") {
                if let Some(num) = rest.strip_prefix(prefix) {
                    let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = digits.parse::<u32>() {
                        out.insert(n);
                    }
                }
            }
        }
    }
    out
}

/// Every `## KI-N` / `## ADR-N` heading claims a DISTINCT number.
///
/// `defined()` collects into a set, so two entries sharing a number collapse into one and
/// every reference to it still "resolves" — the collision is invisible to the two tests
/// below. That is not hypothetical: on 2026-08-27 two sessions numbered different issues
/// KI-70 within minutes of each other (the checker's literal-walk gap, and a note on
/// reversed-args renames), and nothing caught it. A duplicate is worse than a dangling
/// reference, because every later citation of that number is ambiguous forever — including
/// in commit messages and release tags, which cannot be corrected.
///
/// Fix a collision by renumbering the NEWER entry to the next free number.
#[test]
fn no_two_entries_claim_the_same_number() {
    let root = workspace_root();
    for (files, prefix) in [
        (&["docs/known-issues.md"][..], "KI-"),
        (
            &["docs/decisions.md", "docs/archive/decisions-superseded.md"][..],
            "ADR-",
        ),
    ] {
        let mut seen: BTreeMap<u32, usize> = BTreeMap::new();
        for rel in files {
            let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
                continue;
            };
            for line in text.lines() {
                if let Some(rest) = line.trim_start().strip_prefix("## ") {
                    if let Some(num) = rest.strip_prefix(prefix) {
                        let digits: String =
                            num.chars().take_while(|c| c.is_ascii_digit()).collect();
                        if let Ok(n) = digits.parse::<u32>() {
                            *seen.entry(n).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
        assert!(
            seen.len() > 20,
            "only {} {prefix} headings parsed — the format probably changed, which would \
             make this test vacuous rather than passing",
            seen.len()
        );
        let dupes: Vec<String> = seen
            .iter()
            .filter(|(_, &count)| count > 1)
            .map(|(n, count)| format!("  {prefix}{n} has {count} sections"))
            .collect();
        assert!(
            dupes.is_empty(),
            "two entries claim the same number — renumber the NEWER one to the next free \
             number, since every citation of a duplicated number is ambiguous forever:\n{}",
            dupes.join("\n")
        );
    }
}

#[test]
fn every_adr_reference_resolves_to_an_adr() {
    let root = workspace_root();
    // Superseded ADRs keep their entries in the archive, and `decisions.md` links there.
    let known = defined(
        &root,
        &["docs/decisions.md", "docs/archive/decisions-superseded.md"],
        "ADR-",
    );
    assert!(
        known.len() > 100,
        "only {} ADRs parsed — the heading format probably changed, which would make this \
         test vacuous rather than passing",
        known.len()
    );

    let mut dangling: Vec<String> = Vec::new();
    for path in scanned_files(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for n in refs_in(&text, "ADR-") {
            // ADR-000 is an idiom ("greenfield, ADR-000 spirit"), never a real entry.
            if n == 0 || known.contains(&n) {
                continue;
            }
            dangling.push(format!(
                "{}: ADR-{:03}",
                path.strip_prefix(&root).unwrap_or(&path).display(),
                n
            ));
        }
    }
    dangling.sort();
    dangling.dedup();
    assert!(
        dangling.is_empty(),
        "these cite an ADR that has no entry in decisions.md or the archive — write the ADR \
         or fix the citation:\n  {}",
        dangling.join("\n  ")
    );
}

#[test]
fn every_ki_reference_resolves_to_a_known_issue() {
    let root = workspace_root();
    let known = defined(&root, &["docs/known-issues.md"], "KI-");
    assert!(
        known.len() > 20,
        "only {} KI entries parsed — heading format changed?",
        known.len()
    );

    let mut dangling: Vec<String> = Vec::new();
    for path in scanned_files(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for n in refs_in(&text, "KI-") {
            if known.contains(&n) {
                continue;
            }
            dangling.push(format!(
                "{}: KI-{}",
                path.strip_prefix(&root).unwrap_or(&path).display(),
                n
            ));
        }
    }
    dangling.sort();
    dangling.dedup();
    assert!(
        dangling.is_empty(),
        "these cite a KI with no `## KI-N` section (an index row alone is not enough — the \
         file's header tells the reader to jump to the entry):\n  {}",
        dangling.join("\n  ")
    );
}
