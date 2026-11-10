//! `docs/source-map.md` is generated, and this test is the generator and the freshness gate.
//!
//! The map lists every Rust module two levels deep under `crates/lisp/src` and every std
//! module, each with the first sentence of its own documentation: a `.rs` file's leading
//! `//!` paragraph, a `.blsp` file's `(defmodule name "…")` docstring, or, for a prelude file
//! with no `defmodule`, its opening `;;` header. A hand-kept index (the old CLAUDE.md layout
//! tree) drifted silently as modules landed; this one cannot, because a module added or
//! moved without regenerating fails here, and a module with no documentation fails too.
//!
//! Regenerate after adding, moving or re-documenting a module:
//! `BLESS=1 cargo test -p brood --test source_map`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The first sentence of `text`: up to the first `. ` (or a final `.`), whitespace collapsed.
fn first_sentence(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.find(". ") {
        Some(end) => collapsed[..=end].to_string(),
        None => collapsed,
    }
}

/// A Rust file's leading `//!` paragraph (until the first blank `//!` line).
fn rust_summary(source: &str) -> Option<String> {
    let mut paragraph = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("//!") else {
            if paragraph.is_empty() && line.trim().is_empty() {
                continue;
            }
            break;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        paragraph.push(rest);
    }
    (!paragraph.is_empty()).then(|| first_sentence(&paragraph.join(" ")))
}

/// A Brood file's `(defmodule name "doc…")` name and docstring, or its opening `;;` header.
/// Only a `defmodule` at the start of a line counts: the words also appear in comments and
/// docstrings. A `defmodule` without a docstring keeps its name and takes the header.
fn brood_summary(source: &str) -> Option<(Option<String>, String)> {
    let mut module_name = None;
    let defmodule_start = source
        .match_indices("(defmodule ")
        .map(|(start, _)| start)
        .find(|start| *start == 0 || source.as_bytes()[start - 1] == b'\n');
    if let Some(start) = defmodule_start {
        let after = &source[start + "(defmodule ".len()..];
        let name: String = after
            .chars()
            .take_while(|character| !character.is_whitespace() && *character != ')')
            .collect();
        let rest = after[name.len()..].trim_start();
        if let Some(body) = rest.strip_prefix('"') {
            let mut docstring = String::new();
            let mut characters = body.chars();
            while let Some(character) = characters.next() {
                match character {
                    '\\' => docstring.extend(characters.next()),
                    '"' => break,
                    other => docstring.push(other),
                }
            }
            return Some((Some(name), first_sentence(&docstring)));
        }
        module_name = Some(name);
    }
    // A `;; ---- title ----` rule opens most prelude files: it is a title, not the start of
    // the first sentence, so it is set off from the paragraph under it.
    let mut title = None;
    let mut header = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix(";;") else {
            break;
        };
        let text = rest.trim().trim_matches('-').trim();
        if text.is_empty() {
            if header.is_empty() && title.is_none() {
                continue;
            }
            break;
        }
        if header.is_empty() && title.is_none() && rest.trim().starts_with("--") {
            title = Some(text);
            continue;
        }
        header.push(text);
    }
    let paragraph = (!header.is_empty()).then(|| first_sentence(&header.join(" ")));
    let summary = match (title, paragraph) {
        (Some(title), Some(paragraph)) => format!("{title} — {paragraph}"),
        (Some(title), None) => title.to_string(),
        (None, Some(paragraph)) => paragraph,
        (None, None) => return None,
    };
    Some((module_name, summary))
}

fn files_under(directory: &Path, extension: &str, depth: usize, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    // A parent module `foo.rs` before the `foo/` directory of its children.
    entries.sort_by_key(|path| {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        (stem, path.is_dir())
    });
    for path in entries {
        if path.is_dir() {
            if depth > 1 {
                files_under(&path, extension, depth - 1, found);
            }
        } else if path
            .extension()
            .is_some_and(|found_extension| found_extension == extension)
        {
            found.push(path);
        }
    }
}

fn is_test_module(path: &Path) -> bool {
    let stem = path.file_stem().unwrap().to_string_lossy();
    stem == "tests" || stem.ends_with("_tests")
}

/// The map's full text, and the files that had nothing to summarise.
fn generate(root: &Path) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut undocumented = Vec::new();
    out.push_str(
        "# Source map\n\n\
         Every Rust module two levels deep under `crates/lisp/src`, and every std module, with the\n\
         first sentence of its own documentation. **Generated** by `crates/lisp/tests/source_map.rs`\n\
         from each file's `//!` paragraph or `defmodule` docstring, so do not edit it by hand:\n\
         change the module's documentation and run `BLESS=1 cargo test -p brood --test source_map`.\n\
         The test fails when a module is added, moved or re-documented without regenerating.\n\n\
         A parent module is `foo.rs` beside its `foo/` directory (never `foo/mod.rs`). On disk std\n\
         groups modules under `proc/`, `net/` and `tool/`, but their names stay bare (`gen/spawn-server`,\n\
         `http/get`, `test/run`); `editor/*` is the one prefix that is part of the name.\n\n\
         ## Rust kernel — `crates/lisp/src`\n\n",
    );
    let source_root = root.join("crates/lisp/src");
    let mut rust_files = Vec::new();
    files_under(&source_root, "rs", 2, &mut rust_files);
    for path in rust_files.iter().filter(|path| !is_test_module(path)) {
        let relative = path
            .strip_prefix(&source_root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let indent = if relative.contains('/') { "  " } else { "" };
        match rust_summary(&std::fs::read_to_string(path).unwrap()) {
            Some(summary) => writeln!(out, "{indent}- `{relative}` — {summary}").unwrap(),
            None => undocumented.push(format!("crates/lisp/src/{relative}")),
        }
    }
    out.push_str("\n## Standard library — `std/`\n\n");
    let std_root = root.join("std");
    let mut brood_files = Vec::new();
    files_under(&std_root, "blsp", 8, &mut brood_files);
    for path in &brood_files {
        let relative = path
            .strip_prefix(&std_root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        match brood_summary(&std::fs::read_to_string(path).unwrap()) {
            Some((Some(name), summary)) => {
                writeln!(out, "- `{relative}` (`{name}`) — {summary}").unwrap()
            }
            Some((None, summary)) => writeln!(out, "- `{relative}` — {summary}").unwrap(),
            None => undocumented.push(format!("std/{relative}")),
        }
    }
    (out, undocumented)
}

#[test]
fn the_source_map_matches_the_tree() {
    let root = repository_root();
    let (expected, undocumented) = generate(&root);
    assert!(
        undocumented.is_empty(),
        "these modules have no documentation to put in docs/source-map.md — give each a \
         leading `//!` paragraph (Rust) or a `defmodule` docstring (Brood):\n  {}",
        undocumented.join("\n  ")
    );
    let map_path = root.join("docs/source-map.md");
    if std::env::var_os("BLESS").is_some() {
        std::fs::write(&map_path, &expected).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&map_path).unwrap_or_default();
    if committed != expected {
        let first_difference = committed
            .lines()
            .zip(expected.lines())
            .position(|(committed_line, expected_line)| committed_line != expected_line)
            .unwrap_or_else(|| committed.lines().count().min(expected.lines().count()));
        panic!(
            "docs/source-map.md is stale (first difference at line {}: expected {:?}).\n\
             Regenerate: BLESS=1 cargo test -p brood --test source_map",
            first_difference + 1,
            expected
                .lines()
                .nth(first_difference)
                .unwrap_or("<end of file>")
        );
    }
}

/// Sabotage for the gate itself: a module added without regenerating must be reported.
#[test]
fn a_new_module_changes_the_map() {
    let root = repository_root();
    let (before, _) = generate(&root);
    let mut with_extra = before.clone();
    with_extra.push_str("- `new.rs` — A module nobody listed.\n");
    assert_ne!(before, with_extra);
    assert_eq!(
        rust_summary("//! First sentence. Second sentence.\n//! More.\n\nuse x;"),
        Some("First sentence.".to_string())
    );
    assert_eq!(rust_summary("use x;\n"), None);
    assert_eq!(
        brood_summary("(defmodule seq\n  \"Sequences. More \\\"text\\\".\")"),
        Some((Some("seq".to_string()), "Sequences.".to_string()))
    );
    assert_eq!(
        brood_summary(";; ---- pattern matching ----\n;; The matcher.\n\n(defn x ())"),
        Some((None, "pattern matching — The matcher.".to_string()))
    );
}
