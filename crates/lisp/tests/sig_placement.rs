//! A `(sig …)` must not sit above the definition it describes.
//!
//! Why this exists. `sig` is a pure declaration by default, so above-the-defn reads fine
//! and *is* fine — until `BROOD_CONTRACTS=1`, where the same form becomes an **action**:
//! `sig!` rebinds the name to a checking wrapper, so the definition has to exist already.
//! A forward `sig` then takes the whole module load down with a "not defined yet" error.
//!
//! That is KI-81's shape, and it came back. KI-81 fixed the two *prelude* causes on
//! 2026-08-29 and `cli::contracts_mode` has guarded them since — but that test only proves
//! the prelude boots. The signature-adoption sweep then put 211 sigs above their `defn`
//! across `std/`, and every one of them was a module that could no longer be loaded under
//! contracts. The existing test could not see it: its program declares its own sigs
//! correctly, and it only reached `std/string.blsp` at all through an auto-derived import.
//!
//! So this asserts the placement rule directly, over every `.blsp` in the tree. It is a
//! *textual* check on purpose: cheap, no interpreter, and it fails on the line you have to
//! move. Only a definer in the SAME file counts — a sig for a name defined elsewhere (or
//! generated, like a `defrecord` accessor) is not a forward reference and is left alone.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn blsp_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("std"), root.join("tests"), root.join("examples")];
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

/// The name a `(sig NAME …)` line declares, if the line is one.
fn sig_name(line: &str) -> Option<&str> {
    line.strip_prefix("(sig ")?.split_whitespace().next()
}

/// The name a definition form binds, if this line opens one. Matched anywhere in the line,
/// not just at column 0, so a `defn` nested in a `(check-allow …)` wrapper still counts —
/// `std/datetime.blsp`'s `dt-zero-pad` is exactly that shape.
fn def_name(line: &str) -> Option<&str> {
    for opener in ["(defn- ", "(defn ", "(defmacro ", "(defdyn ", "(def "] {
        if let Some(at) = line.find(opener) {
            let rest = &line[at + opener.len()..];
            return rest.split_whitespace().next();
        }
    }
    None
}

#[test]
fn no_sig_precedes_the_definition_it_describes() {
    let root = workspace_root();
    let mut offenders: Vec<String> = Vec::new();

    for path in blsp_files(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();

        // First definition line for each name in this file.
        let mut first_def: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(name) = def_name(line) {
                first_def.entry(name).or_insert(i);
            }
        }

        for (i, line) in lines.iter().enumerate() {
            let Some(name) = sig_name(line) else { continue };
            if let Some(&def_line) = first_def.get(name) {
                if i < def_line {
                    let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                    offenders.push(format!(
                        "{rel}:{} — (sig {name} …) is above its definition at line {}",
                        i + 1,
                        def_line + 1
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "a `(sig …)` above its own definition breaks the module under BROOD_CONTRACTS=1 \
         (`sig` installs a contract there, which needs the name to exist) — move each one \
         BELOW the form it describes:\n  {}",
        offenders.join("\n  ")
    );
}
