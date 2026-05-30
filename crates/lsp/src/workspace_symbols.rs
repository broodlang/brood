//! `workspace/symbol` — project-wide symbol search.
//!
//! The top-level `def`/`defn`/`defmacro` of every project file (plus every open
//! buffer), filtered by the client's query. Reuses the same CST def walker the
//! document outline uses ([`defs::top_level`]) and the same project file set
//! `references`/`rename` walk ([`workspace::all_sources`]) — so "go to symbol"
//! across the project costs one parse per file and no evaluation.
//!
//! Matching is a **case-insensitive subsequence** test (the fuzzy model editors
//! expect: `fl0` matches `format-source` via f…o…). The client typically filters
//! again, but a server-side filter keeps the payload small on a big project. An
//! empty query returns every symbol (the "show all" affordance some clients use).

use brood::syntax::cst;
use brood::Interp;
use lsp_types::{Location, OneOf, Range, SymbolKind, Uri, WorkspaceSymbol};

use crate::defs::{self, DefKind};
use crate::line_index::LineIndex;
use crate::{workspace, Documents};

/// Every top-level definition across the project whose name matches `query`.
pub fn workspace_symbols(
    interp: &mut Interp,
    docs: &Documents,
    query: &str,
) -> Vec<WorkspaceSymbol> {
    let mut out = Vec::new();
    for (uri, text) in workspace::all_sources(interp, docs) {
        let root = cst::parse(&text);
        let index = LineIndex::new(&text);
        let container = file_label(&uri);
        for d in defs::top_level(&root, &text) {
            if !matches(query, d.name) {
                continue;
            }
            let range = Range::new(
                index.position(&text, d.name_span.start),
                index.position(&text, d.name_span.end),
            );
            out.push(WorkspaceSymbol {
                name: d.name.to_string(),
                kind: symbol_kind(d.kind),
                tags: None,
                container_name: container.clone(),
                location: OneOf::Left(Location::new(uri.clone(), range)),
                data: None,
            });
        }
    }
    out
}

/// Map a Brood def kind to the LSP symbol kind the editor renders an icon for.
fn symbol_kind(kind: DefKind) -> SymbolKind {
    match kind {
        DefKind::Fn => SymbolKind::FUNCTION,
        DefKind::Macro => SymbolKind::FUNCTION, // no dedicated "macro" kind in LSP
        DefKind::Var => SymbolKind::VARIABLE,
    }
}

/// Case-insensitive subsequence match: every char of `query`, in order, appears
/// somewhere in `name`. An empty query matches everything.
fn matches(query: &str, name: &str) -> bool {
    let mut q = query.chars().map(|c| c.to_ascii_lowercase()).peekable();
    if q.peek().is_none() {
        return true;
    }
    for nc in name.chars().map(|c| c.to_ascii_lowercase()) {
        if q.peek() == Some(&nc) {
            q.next();
            if q.peek().is_none() {
                return true;
            }
        }
    }
    false
}

/// The file name (no directory) for the symbol's `containerName`, giving the
/// user a "which file" hint in the picker. `None` for a non-`file:` URI.
fn file_label(uri: &Uri) -> Option<String> {
    let path = crate::uri_to_path(uri)?;
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matching() {
        assert!(matches("", "anything"));
        assert!(matches("fs", "format-source"));
        assert!(matches("FORMAT", "format-source")); // case-insensitive
        assert!(matches("frmtsrc", "format-source"));
        assert!(!matches("xyz", "format-source"));
        assert!(!matches("sf", "format-source")); // order matters
    }

    #[test]
    fn def_kind_maps_to_symbol_kind() {
        assert_eq!(symbol_kind(DefKind::Fn), SymbolKind::FUNCTION);
        assert_eq!(symbol_kind(DefKind::Macro), SymbolKind::FUNCTION);
        assert_eq!(symbol_kind(DefKind::Var), SymbolKind::VARIABLE);
    }
}
