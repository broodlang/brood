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

use brood::syntax::cst::{self, Node, NodeKind};
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
        // A file's `(defmodule ns …)` is its namespace (ADR-065). Display each
        // symbol qualified (`ns/name`) with the namespace as the container, so
        // two same-named defs in different namespaces are distinguishable in the
        // picker. A file with no `defmodule` falls back to the bare name + file.
        let ns = file_namespace(&root, &text);
        for d in defs::top_level(&root, &text) {
            let name = match ns {
                Some(ns) => format!("{ns}/{}", d.name),
                None => d.name.to_string(),
            };
            if !matches(query, &name) {
                continue;
            }
            let range = Range::new(
                index.position(&text, d.name_span.start),
                index.position(&text, d.name_span.end),
            );
            out.push(WorkspaceSymbol {
                name,
                kind: symbol_kind(d.kind),
                tags: None,
                container_name: ns.map(str::to_string).or_else(|| file_label(&uri)),
                location: OneOf::Left(Location::new(uri.clone(), range)),
                data: None,
            });
        }
    }
    out
}

/// The namespace a file declares via a top-level `(defmodule ns …)` form
/// (ADR-065), or `None` if it declares none. A pure CST scan — the same
/// substrate the rest of the LSP uses, no evaluation.
fn file_namespace<'s>(root: &Node, text: &'s str) -> Option<&'s str> {
    for form in root.forms() {
        if form.kind != NodeKind::List {
            continue;
        }
        let mut forms = form.forms();
        let Some(head) = forms.next() else { continue };
        if head.kind != NodeKind::Symbol || head.text(text) != "defmodule" {
            continue;
        }
        if let Some(name) = forms.next() {
            if name.kind == NodeKind::Symbol {
                return Some(name.text(text));
            }
        }
    }
    None
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
    fn extracts_the_file_namespace_from_defmodule() {
        let root = cst::parse("(defmodule parser)\n(defn parse (s) s)");
        assert_eq!(file_namespace(&root, "(defmodule parser)\n(defn parse (s) s)"), Some("parser"));
        // No defmodule → no namespace.
        let bare = cst::parse("(defn parse (s) s)");
        assert_eq!(file_namespace(&bare, "(defn parse (s) s)"), None);
    }

    #[test]
    fn def_kind_maps_to_symbol_kind() {
        assert_eq!(symbol_kind(DefKind::Fn), SymbolKind::FUNCTION);
        assert_eq!(symbol_kind(DefKind::Macro), SymbolKind::FUNCTION);
        assert_eq!(symbol_kind(DefKind::Var), SymbolKind::VARIABLE);
    }
}
