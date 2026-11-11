//! **Every file a crate compiles in is tracked.**
//!
//! `include_str!` / `include_bytes!` and `embedded_module!` (an `include_str!` of a
//! repository-root path) read a file at build time. A commit that drops such a file still
//! builds on its author's machine, where the file is on disk, and breaks every clean checkout:
//! 2b21345d1 swept up a staged deletion of `std/tool/migrate.blsp` while
//! `crates/lisp/src/builtins/modules.rs` still embedded it, and `main` stopped building
//! (2026-11-11).
//!
//! The same rule runs as `scripts/embedded-paths.sh` in the pre-push hook, over the pushed
//! tree; this is it as a test, over the working tree against git's index. Outside a git
//! checkout (a source tarball) the file existing is all that can be asked.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

mod support;
use support::{rust_files, workspace_root};

/// One literal path a source line embeds, as written, and whether it is relative to the
/// repository root (`embedded_module!`) rather than to the file (`include_*!`).
#[derive(Debug, PartialEq)]
struct Embed {
    path: String,
    from_root: bool,
}

/// The string literals at the start of a call's arguments `text`, up to `wanted` of them,
/// stopping at the call's closing parenthesis.
fn literals_after(text: &str, wanted: usize) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while found.len() < wanted {
        let Some(open) = rest.find(['"', ')']) else {
            break;
        };
        if rest[open..].starts_with(')') {
            break;
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else {
            break;
        };
        found.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    found
}

/// Every literal embed on `line`. A call whose argument is not a literal (`concat!(env!(…))`)
/// is not a path in the tree and is skipped.
fn embeds_in(line: &str) -> Vec<Embed> {
    let mut out = Vec::new();
    for (name, from_root) in [
        ("include_str!(", false),
        ("include_bytes!(", false),
        ("embedded_module!(", true),
    ] {
        let mut rest = line;
        while let Some(at) = rest.find(name) {
            let call = &rest[at + name.len()..];
            if call.starts_with('"') {
                let literals = literals_after(call, if from_root { 2 } else { 1 });
                let path = if from_root {
                    literals.get(1)
                } else {
                    literals.first()
                };
                if let Some(path) = path {
                    out.push(Embed {
                        path: path.clone(),
                        from_root,
                    });
                }
            }
            rest = call;
        }
    }
    out
}

/// `path` with its `.` and `..` segments collapsed, without touching the disk (the file may
/// not exist — that is what is being asked).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// The repository's tracked files, relative to its root — or `None` outside a git checkout.
fn tracked_files(root: &Path) -> Option<HashSet<String>> {
    let output = Command::new("git")
        .arg("ls-files")
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

#[test]
fn every_embedded_file_is_tracked() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    let tracked = tracked_files(&root);
    let mut checked = 0;
    let mut missing = Vec::new();
    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        for (index, line) in source.lines().enumerate() {
            for embed in embeds_in(line) {
                let base = if embed.from_root {
                    root.clone()
                } else {
                    file.parent().expect("a file's directory").to_path_buf()
                };
                let target = normalize(&base.join(&embed.path));
                let relative = target
                    .strip_prefix(&root)
                    .unwrap_or(&target)
                    .to_string_lossy()
                    .replace('\\', "/");
                let present = match &tracked {
                    Some(tracked) => tracked.contains(&relative),
                    None => target.exists(),
                };
                checked += 1;
                if !present {
                    let site = file.strip_prefix(&root).unwrap_or(file);
                    missing.push(format!(
                        "{}:{} embeds {relative}",
                        site.display(),
                        index + 1
                    ));
                }
            }
        }
    }
    // The scan has to find the embeds at all, or an empty list passes vacuously.
    assert!(
        checked > 100,
        "only {checked} embeds found under crates/ — the scanner has stopped seeing them"
    );
    assert!(
        missing.is_empty(),
        "a crate compiles in a file that is not tracked — git add it, or drop the embed:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn the_scanner_reads_each_shape_and_skips_a_computed_path() {
    let quote = '"';
    let module_line = format!("    embedded_module!({quote}io{quote}, {quote}std/io.blsp{quote}),");
    assert_eq!(
        embeds_in(&module_line),
        vec![Embed {
            path: "std/io.blsp".to_string(),
            from_root: true
        }]
    );
    let include_line = format!("const A: &str = include_str!({quote}../../docs/a.md{quote});");
    assert_eq!(
        embeds_in(&include_line),
        vec![Embed {
            path: "../../docs/a.md".to_string(),
            from_root: false
        }]
    );
    let computed =
        format!("include_bytes!(concat!(env!({quote}OUT_DIR{quote}), {quote}/x{quote}))");
    assert!(embeds_in(&computed).is_empty());
    assert_eq!(
        normalize(Path::new("/r/crates/lisp/src/../../../docs/a.md")),
        PathBuf::from("/r/docs/a.md")
    );
}
