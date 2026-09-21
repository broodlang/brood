//! A `(sig …)` must not hide inside a docstring, where it declares nothing.
//!
//! This file was `sig_placement.rs`: it also policed that no `(sig …)` sat ABOVE the
//! definition it describes, because under `BROOD_CONTRACTS=1` the form used to become a
//! rebinding and a forward `sig` took the whole module load down (KI-81, and its return
//! through the adoption sweep's 211 forward sigs). ADR-381 made enforcement a property of
//! the BINDING — installed whenever the definition lands, whichever side the declaration is
//! on — so that rule has no failure left to catch and was removed; `tests/contract_test.blsp`
//! enforces a `sig` on either side. What remains is the other slip the same sweep made.

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

/// A `(sig …)` line INSIDE a docstring is not a declaration at all. The adoption sweep of
/// 2026-08-29 inserted `math/pow`'s sig after the first line of its `defn` — which was the
/// first line of its docstring — and for nineteen days `pow` declared nothing while the
/// file read as if it did: no gate fails on a missing declaration, and the docstring
/// printed the sig as prose. Read with the real reader (a docstring's escaped quotes fool
/// a textual quote count), and only a DOCSTRING is judged — a test that embeds a whole
/// program in a string is meant to.
#[test]
fn no_sig_hides_inside_a_docstring() {
    use brood::core::value::{symbol_name_ref, Value};
    let root = workspace_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut interp = brood::Interp::new();
    for path in blsp_files(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(forms) = brood::syntax::reader::read_all(&mut interp.heap, &text) else {
            continue;
        };
        for form in forms {
            let Ok(items) = interp.heap.list_to_vec(form) else {
                continue;
            };
            // `(defn NAME (params) "doc" …)` / `(defmacro …)`: the docstring is item 3.
            let definer = matches!(items.first(), Some(Value::Sym(s))
                if matches!(symbol_name_ref(*s), "defn" | "defn-" | "defmacro"));
            let (Some(Value::Sym(name)), Some(Value::Str(doc))) = (items.get(1), items.get(3))
            else {
                continue;
            };
            if !definer {
                continue;
            }
            let doc = interp.heap.string(*doc).to_string();
            // At column 0 only: an INDENTED `(sig …)` is a doc example (`sig`'s own
            // docstring shows its spelling that way), a slipped declaration never is.
            if doc.lines().any(|line| line.starts_with("(sig ")) {
                let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                offenders.push(format!(
                    "{rel} — the docstring of `{}`",
                    symbol_name_ref(*name)
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a `(sig …)` inside a docstring declares nothing — move each one BELOW the \
         definition's closing paren:\n  {}",
        offenders.join("\n  ")
    );
}
