//! Whole-project (cross-file) references and rename. A name that resolves to a
//! **global** is, under the flat module model (ADR-019), one binding across the
//! entire project — so its references are the union of `references_to_global`
//! over every project file, and a rename edits them all. This is the static,
//! CST-level reference model ADR-031 keeps (definitions go image-based; refs
//! stay static — macro-generated references have no faithful spans, so we report
//! source occurrences).
//!
//! A **local** never reaches here — the caller routes locals to the single-file
//! [`references`](crate::references) / [`rename`](crate::rename) path. With no
//! project bootstrapped (a bare buffer), the file set degrades to just the open
//! document, so the feature still works, single-file.

use std::collections::{HashMap, HashSet};

use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::{cst, scope};
use brood::Interp;
use lsp_types::{Location, Range, TextEdit, Uri, WorkspaceEdit};

use crate::line_index::LineIndex;
use crate::Documents;
use brood::introspect;

/// The symbol name at `offset`, if the cursor is on one.
pub fn symbol_at<'s>(root: &Node, text: &'s str, offset: u32) -> Option<&'s str> {
    let n = root.node_at(offset)?;
    (n.kind == NodeKind::Symbol).then(|| n.text(text))
}

/// Every cross-file reference to the global `name`, deduped by location.
pub fn references(interp: &mut Interp, docs: &Documents, current: &Uri, name: &str) -> Vec<Location> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (uri, text) in project_sources(interp, docs, current) {
        let root = cst::parse(&text);
        let tree = scope::analyze(&root, &text);
        let index = LineIndex::new(&text);
        for span in tree.references_to_global(&root, &text, name) {
            let range = Range::new(index.position(&text, span.start), index.position(&text, span.end));
            if seen.insert((uri.clone(), range.start, range.end)) {
                out.push(Location::new(uri.clone(), range));
            }
        }
    }
    out
}

/// A project-wide rename of the global `name` → `new_name`: a [`WorkspaceEdit`]
/// with the edits grouped per file. `None` if `new_name` isn't a valid Brood
/// symbol or there's nothing to rename.
pub fn rename(
    interp: &mut Interp,
    docs: &Documents,
    current: &Uri,
    name: &str,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    if !crate::rename::is_valid_symbol(new_name) {
        return None;
    }
    let refs = references(interp, docs, current, name);
    if refs.is_empty() {
        return None;
    }
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for loc in refs {
        changes.entry(loc.uri).or_default().push(TextEdit {
            range: loc.range,
            new_text: new_name.to_string(),
        });
    }
    Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    })
}

/// `(uri, text)` for every project file, preferring an open document's
/// in-memory text over the on-disk copy (so unsaved edits are searched). The
/// open `current` document is always included, even if it's outside the project
/// (a scratch buffer), so its own occurrences aren't missed.
fn project_sources(interp: &mut Interp, docs: &Documents, current: &Uri) -> Vec<(Uri, String)> {
    let mut out = Vec::new();
    let mut uris = HashSet::new();
    for path in introspect::project_files(interp) {
        let Some(uri) = crate::path_to_uri(&path) else {
            continue;
        };
        let text = docs
            .get(&uri)
            .map(|d| d.text.clone())
            .or_else(|| std::fs::read_to_string(&path).ok());
        if let Some(text) = text {
            if uris.insert(uri.clone()) {
                out.push((uri, text));
            }
        }
    }
    if !uris.contains(current) {
        if let Some(doc) = docs.get(current) {
            out.push((current.clone(), doc.text.clone()));
        }
    }
    out
}
