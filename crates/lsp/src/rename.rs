//! `textDocument/rename` (and `prepareRename`): rename every occurrence of the
//! symbol under the cursor. Built on the same [`ScopeTree::references`] engine as
//! find-references, so a local is renamed only within its scope and a document
//! global across its whole file. **Single-file**: occurrences in other modules
//! aren't touched (the server has no faithful cross-file reference index — ADR-031,
//! docs/lsp.md §Cross-file), so renaming an exported name is incomplete by
//! design; we rename what we can see.

use std::collections::HashMap;

use brood::core::heap::Heap;
use brood::core::value::{symbol_name, Value};
use brood::syntax::cst::Node;
use brood::syntax::printer::symbol_needs_bars;
use brood::syntax::reader;
use brood::syntax::scope::ScopeTree;
use lsp_types::{Range, TextEdit, Uri, WorkspaceEdit};

use crate::line_index::LineIndex;

/// The range to rename — the symbol token under `offset`, if there is one and it
/// has at least one occurrence we can edit. Returned for `prepareRename`, so the
/// editor highlights the right span before prompting.
pub fn prepare_rename(
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Option<Range> {
    let node = root.node_at(offset)?;
    if node.kind != brood::syntax::cst::NodeKind::Symbol {
        return None;
    }
    // Only offer rename when there's something to rename.
    if tree.references(root, text, offset).is_empty() {
        return None;
    }
    Some(index.range(text, node.span))
}

/// A [`WorkspaceEdit`] replacing every in-file occurrence of the symbol at
/// `offset` with `new_name`. `None` if the cursor isn't on a renameable symbol,
/// there are no occurrences, or `new_name` isn't a valid Brood symbol (so we
/// never produce an edit that wouldn't parse).
pub fn rename(
    uri: &Uri,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !is_valid_symbol(new_name) {
        return None;
    }
    let spans = tree.references(root, text, offset);
    if spans.is_empty() {
        return None;
    }
    let edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|s| TextEdit {
            range: index.range(text, s),
            new_text: new_name.to_string(),
        })
        .collect();
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// Whether `name`, written bare, reads back as exactly the symbol `name` — the only
/// spelling a rename may write, since every edit splices the bare text in. The
/// printer's own round-trip predicate ([`symbol_needs_bars`]) refuses what it would
/// bar-quote (a delimiter, a control character, a dispatching `#`/`^` first
/// character — `^foo` reads as `(%pin foo)`, a different program — a number,
/// `nil`/`true`/`false`, a keyword); the read-back then confirms it against the
/// reader itself, so no spelling the two disagree on can slip through. Shared with
/// the cross-file rename path ([`workspace`](crate::workspace)).
pub(crate) fn is_valid_symbol(name: &str) -> bool {
    if symbol_needs_bars(name, false) {
        return false;
    }
    let mut heap = Heap::new();
    match reader::read_one_complete(&mut heap, name) {
        Ok(Value::Sym(symbol)) => symbol_name(symbol) == name,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn rename_at(src: &str, needle: &str, to: &str) -> Option<usize> {
        let uri: Uri = "file:///t.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        rename(&uri, src, &root, &tree, &index, at, to)
            .and_then(|w| w.changes)
            .map(|c| c.values().next().unwrap().len())
    }

    #[test]
    fn renames_all_occurrences_of_a_global() {
        // def + two calls = three edits.
        assert_eq!(
            rename_at("(defn f (x) x)\n(f 1)\n(f 2)", "f (x)", "g"),
            Some(3)
        );
    }

    #[test]
    fn rejects_an_invalid_new_name() {
        assert_eq!(rename_at("(defn f (x) x)", "defn f", "(bad)"), None);
        assert_eq!(rename_at("(defn f (x) x)", "defn f", "42"), None);
        assert_eq!(rename_at("(defn f (x) x)", "defn f", ":kw"), None);
    }

    #[test]
    fn rejects_a_name_that_does_not_read_back_as_itself() {
        // `^foo` reads as `(%pin foo)` — a rename to it changes the program's meaning;
        // `#foo` is a dispatch error, `|foo` an unterminated bar symbol, and a
        // control character is refused in a bare token.
        for bad in [
            "^foo", "#foo", "|foo", "a\u{1}b", "a\u{7f}", "nil", "1/2", ".",
        ] {
            assert_eq!(rename_at("(defn f (x) x)", "defn f", bad), None, "{bad:?}");
            assert!(!is_valid_symbol(bad), "{bad:?}");
        }
        for good in ["g", "a^b", "a#", "string/join", "set!", "->x", "+"] {
            assert!(is_valid_symbol(good), "{good:?}");
        }
        assert_eq!(rename_at("(defn f (x) x)", "defn f", "a^b"), Some(1));
    }

    #[test]
    fn no_rename_off_a_symbol() {
        assert_eq!(rename_at("(+ 1 2)", "1", "x"), None);
    }
}
