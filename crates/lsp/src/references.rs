//! `textDocument/references` and `textDocument/documentHighlight`: every
//! occurrence in this document that binds to the same thing as the symbol under
//! the cursor. Both read the one engine — [`ScopeTree::references`]
//! ([`scope`](brood::syntax::scope)) — which scopes a local to its block and
//! matches a free name's free occurrences across the file. Single-file by
//! design: references through `require`d modules and macro-generated code have
//! no faithful spans (ADR-031, docs/lsp.md §Cross-file).

use brood::syntax::cst::Node;
use brood::syntax::scope::ScopeTree;
use lsp_types::{DocumentHighlight, DocumentHighlightKind, Location, Range, Uri};

use crate::line_index::LineIndex;

/// All references to the symbol at `offset`, as cross-referenceable `Location`s
/// in this file. Empty when the cursor isn't on a symbol.
pub fn references(
    uri: &Uri,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Vec<Location> {
    spans(text, root, tree, index, offset)
        .map(|range| Location::new(uri.clone(), range))
        .collect()
}

/// Same occurrences as [`references`], as in-file highlights (what an editor
/// paints when the cursor rests on a name).
pub fn document_highlights(
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Vec<DocumentHighlight> {
    spans(text, root, tree, index, offset)
        .map(|range| DocumentHighlight {
            range,
            kind: Some(DocumentHighlightKind::TEXT),
        })
        .collect()
}

/// Shared core: the occurrence spans projected to LSP ranges.
fn spans<'a>(
    text: &'a str,
    root: &'a Node,
    tree: &'a ScopeTree,
    index: &'a LineIndex,
    offset: u32,
) -> impl Iterator<Item = Range> + 'a {
    tree.references(root, text, offset)
        .into_iter()
        .map(move |s| Range::new(index.position(text, s.start), index.position(text, s.end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn ref_count(src: &str, needle: &str) -> usize {
        let uri: Uri = "file:///t.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        references(&uri, src, &root, &tree, &index, at).len()
    }

    #[test]
    fn finds_every_use_of_a_document_global() {
        // `f` is defined once and called twice → three occurrences.
        let n = ref_count("(defn f (x) (* x x))\n(f 1)\n(f 2)", "f (x)");
        assert_eq!(n, 3);
    }

    #[test]
    fn a_local_is_scoped_to_its_binder() {
        // The two `x`s in this `defn` (param + use) are one binding; a same-named
        // `x` in another scope would not be included.
        let n = ref_count("(defn f (x) (+ x 1))", "x 1");
        assert_eq!(n, 2);
    }

    #[test]
    fn no_references_off_a_symbol() {
        assert_eq!(ref_count("(+ 1 2)", "1"), 0);
    }
}
