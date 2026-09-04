//! **A test that calls `%isolate` must be marked `:isolated`.**
//!
//! `%isolate` snapshots the global binding table, runs a thunk, and swaps the table back. Its
//! own documentation states the condition: it "is sound only with no other process mutating
//! globals concurrently, which the runner ensures by running isolated tests alone." A test
//! that calls it while the runner has parallel workers in flight breaks that condition —
//! every sibling worker's `def`s are rolled away underneath it, and a `defrecord` caught
//! mid-flight leaves its `%record-register` (taken under the registry lock, so it survives
//! the swap) pointing at a constructor that does not. Those are KI-89's orphaned record ids.
//!
//! Eight tests were violating it, in two files, and nothing said so — the framework has no
//! way to know that a test body reaches `%isolate`, and the damage lands in *other* files, so
//! the failures never named the cause. This is that missing signal, as a static check,
//! because the runtime one would have to fire on the very interleaving it is trying to
//! prevent.
//!
//! **String literals do not count, and that is not a loophole.** A test may *write* the text
//! `%isolate` into a program it hands to a child process — `stdimage_test`'s attribution case
//! does exactly that, because "the names a require introduces" is only measurable in a runtime
//! where the module is not loaded yet. That isolate runs in another process and cannot touch
//! this runner's globals, so counting it would force an `:isolated` marker that buys nothing
//! and serialises the suite for no reason.
//!
//! **Known limit, deliberate.** It is textual, so a test that reaches `%isolate` through a
//! helper defined in another file is not caught. Every in-tree call is written inline today,
//! and a check that catches the ordinary case beats one that is never written; the ADR-006
//! alternative — teaching the runner to detect it at run time — is the KI-89 design question
//! this file exists to keep narrow in the meantime.

use std::path::Path;

/// The `tests/` directory of the repo, from this crate's manifest.
fn tests_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests")
}

/// `line` with every double-quoted string literal removed, so text *about* `%isolate` — or a
/// program built as a string for a child process — is not read as a call. Escapes are honoured
/// so an embedded `\"` does not end the literal early.
fn strip_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// True when `line` opens a test block, and whether it carries `:isolated`.
fn test_header(line: &str) -> Option<bool> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix(":isolated ") {
        return rest.starts_with("(test ").then_some(true);
    }
    t.starts_with("(test ").then_some(false)
}

#[test]
fn every_test_that_calls_isolate_is_marked_isolated() {
    let dir = tests_dir();
    let mut offenders: Vec<String> = Vec::new();

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("read tests/")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "blsp"))
        .collect();
    files.sort();
    assert!(
        files.len() > 50,
        "expected to find the test suite in {dir:?}, found {} file(s) — a check that scans \
         nothing passes vacuously",
        files.len()
    );

    for path in files {
        let text = std::fs::read_to_string(&path).expect("read a test file");
        // Walk test blocks: a block runs from its `(test …` header to the next header (or a
        // top-level `(describe`), which is close enough on this suite's formatting and errs
        // toward reporting rather than missing.
        let mut current: Option<(bool, String, usize)> = None;
        let flush = |cur: Option<(bool, String, usize)>, offenders: &mut Vec<String>| {
            if let Some((isolated, body, line_no)) = cur {
                if !isolated && body.contains("(%isolate") {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.file_name().unwrap().to_string_lossy(),
                        line_no,
                        body.lines().next().unwrap_or("").trim()
                    ));
                }
            }
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(isolated) = test_header(line) {
                flush(current.take(), &mut offenders);
                current = Some((isolated, String::new(), i + 1));
            } else if line.starts_with("(describe") || line.starts_with("(defn") {
                flush(current.take(), &mut offenders);
            }
            if let Some((_, body, _)) = current.as_mut() {
                body.push_str(&strip_string_literals(line));
                body.push('\n');
            }
        }
        flush(current.take(), &mut offenders);
    }

    assert!(
        offenders.is_empty(),
        "these tests call `%isolate` but are not marked `:isolated`, so they roll the global \
         table back while the runner's parallel workers are still running (KI-89):\n  {}",
        offenders.join("\n  ")
    );
}
