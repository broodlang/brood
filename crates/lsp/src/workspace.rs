//! Whole-project (cross-file) references and rename, **namespace-aware** (ADR-065
//! §6). The symbol at the cursor is first resolved — against its file's namespace
//! and `(:use …)` imports — to a single qualified global (`observer/observe`);
//! then every project file is scanned for occurrences that resolve to *that same*
//! global. So a bare `observe` counts only in a file whose namespace/imports make
//! it `observer/observe` (a different ns's `observe` is left alone — rename is
//! sound), and a qualified `observer/observe` token matches exactly. This is the
//! static, CST-level reference model ADR-031 keeps (definitions go image-based;
//! references stay source occurrences — macro-generated refs have no faithful
//! spans), now filtered by qualified identity instead of bare text.
//!
//! A **local** never reaches here — the caller routes locals to the single-file
//! [`references`](crate::references) / [`rename`](crate::rename) path. With no
//! project bootstrapped (a bare buffer), the file set degrades to just the open
//! document, so the feature still works, single-file.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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

/// The short (unqualified) tail of a possibly-qualified name: `observer/observe`
/// → `observe`; `map` → `map`.
fn short_name(qualified: &str) -> &str {
    qualified.rsplit('/').next().unwrap_or(qualified)
}

/// One cross-file occurrence: where it is, and whether the *token* was written
/// qualified (`ns/name`) — rename keeps the `ns/` prefix on those.
struct Ref {
    uri: Uri,
    range: Range,
    qualified: bool,
}

/// Resolve `name` (at the cursor in `current`) to its qualified global, then
/// collect every project occurrence that resolves to the **same** global:
/// qualified `target` tokens (exact), plus bare `short` tokens in each file whose
/// namespace/imports resolve `short` → `target`. Deduped by (file, range).
fn collect(interp: &mut Interp, docs: &Documents, current: &Uri, name: &str) -> (String, Vec<Ref>) {
    let cur_text = docs.get(current).map(|d| d.text.clone()).unwrap_or_default();
    let target = introspect::resolve_in_source(interp, &cur_text, name);
    let short = short_name(&target).to_string();
    let target_qualified = target.contains('/');

    let mut out: Vec<Ref> = Vec::new();
    let mut seen = HashSet::new();
    for (uri, text) in project_sources(interp, docs, current) {
        let root = cst::parse(&text);
        let tree = scope::analyze(&root, &text);
        let index = LineIndex::new(&text);
        let mut push = |span: brood::error::Span, qualified: bool, out: &mut Vec<Ref>| {
            let range = Range::new(
                index.position(&text, span.start),
                index.position(&text, span.end),
            );
            if seen.insert((uri.clone(), range.start, range.end)) {
                out.push(Ref { uri: uri.clone(), range, qualified });
            }
        };
        // Qualified `ns/name` tokens that *are* the target (exact identity).
        if target_qualified {
            for span in tree.references_to_global(&root, &text, &target) {
                push(span, true, &mut out);
            }
        }
        // Bare `short` tokens — but only in a file that resolves `short` → target
        // (its own ns defines it, or it `(:use …)`s the target's ns). A same-named
        // def in an unrelated namespace resolves elsewhere and is skipped.
        if introspect::resolve_in_source(interp, &text, &short) == target {
            for span in tree.references_to_global(&root, &text, &short) {
                push(span, false, &mut out);
            }
        }
    }
    (target, out)
}

/// Every cross-file reference to the symbol `name` at the cursor in `current`,
/// namespace-resolved and deduped by location.
pub fn references(
    interp: &mut Interp,
    docs: &Documents,
    current: &Uri,
    name: &str,
) -> Vec<Location> {
    collect(interp, docs, current, name)
        .1
        .into_iter()
        .map(|r| Location::new(r.uri, r.range))
        .collect()
}

/// A project-wide, namespace-aware rename `name` → `new_name`. A qualified
/// occurrence keeps its namespace prefix (`observer/observe` → `observer/<new>`);
/// a bare occurrence becomes the bare `<new>`. `None` if `new_name` isn't a valid
/// bare Brood symbol or there's nothing to rename.
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
    let (target, refs) = collect(interp, docs, current, name);
    if refs.is_empty() {
        return None;
    }
    // The namespace prefix to keep on qualified occurrences (e.g. `observer`).
    let prefix = target.strip_suffix(short_name(&target)).unwrap_or("");
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for r in refs {
        let new_text = if r.qualified {
            format!("{prefix}{new_name}") // `prefix` already ends in `/`
        } else {
            new_name.to_string()
        };
        changes
            .entry(r.uri)
            .or_default()
            .push(TextEdit { range: r.range, new_text });
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
///
/// The open-document overlay and the project/current dedup are keyed by the
/// **decoded filesystem path**, not the URI string: an editor's URI and our
/// `path_to_uri` can differ in percent-encoding for the same file (hex case,
/// which bytes get escaped), and matching on the raw URI would then miss the
/// unsaved buffer *and* list the file twice — which for rename would emit
/// double edits.
fn project_sources(interp: &mut Interp, docs: &Documents, current: &Uri) -> Vec<(Uri, String)> {
    // Open buffers indexed by their decoded path → in-memory text.
    let open: HashMap<PathBuf, &str> = docs
        .iter()
        .filter_map(|(u, d)| Some((crate::uri_to_path(u)?, d.text.as_str())))
        .collect();

    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in introspect::project_files(interp) {
        let pb = PathBuf::from(&path);
        if !seen.insert(pb.clone()) {
            continue;
        }
        let Some(uri) = crate::path_to_uri(&path) else {
            continue;
        };
        let text = open
            .get(&pb)
            .map(|s| s.to_string())
            .or_else(|| std::fs::read_to_string(&path).ok());
        if let Some(text) = text {
            out.push((uri, text));
        }
    }
    // Ensure the open `current` document is covered (a scratch buffer outside
    // the project, or a path the project list spelled differently).
    if let Some(cur_path) = crate::uri_to_path(current) {
        if seen.insert(cur_path) {
            if let Some(doc) = docs.get(current) {
                out.push((current.clone(), doc.text.clone()));
            }
        }
    }
    out
}

/// `(uri, text)` for *every* searchable source: the project files (preferring
/// an open buffer's in-memory text over its on-disk copy) unioned with every
/// open document — so a scratch buffer outside any project is searched too.
/// Unlike [`project_sources`], there is no "current" document: workspace-wide
/// features (symbol search) aren't anchored to one open file. Deduped by the
/// decoded filesystem path, same as `project_sources` (see its note).
pub fn all_sources(interp: &mut Interp, docs: &Documents) -> Vec<(Uri, String)> {
    let open: HashMap<PathBuf, &str> = docs
        .iter()
        .filter_map(|(u, d)| Some((crate::uri_to_path(u)?, d.text.as_str())))
        .collect();

    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for path in introspect::project_files(interp) {
        let pb = PathBuf::from(&path);
        if !seen.insert(pb.clone()) {
            continue;
        }
        let Some(uri) = crate::path_to_uri(&path) else {
            continue;
        };
        let text = open
            .get(&pb)
            .map(|s| s.to_string())
            .or_else(|| std::fs::read_to_string(&path).ok());
        if let Some(text) = text {
            out.push((uri, text));
        }
    }
    // Any open document not already covered by the project set (a file:// path
    // outside the project, or a non-`file:` scratch URI).
    for (uri, doc) in docs {
        // A `file:` path already in the project set is skipped; a non-`file:`
        // scratch URI (no path) is always included.
        if let Some(p) = crate::uri_to_path(uri) {
            if !seen.insert(p) {
                continue;
            }
        }
        out.push((uri.clone(), doc.text.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze, Document, Documents};

    #[test]
    fn rename_is_namespace_sound_across_files() {
        // Two namespaces each define `observe`. Renaming `a`'s must touch only
        // a.blsp — never `b`'s unrelated `observe` (the §6 "rename is sound" promise).
        let dir = std::env::temp_dir().join(format!("brood_ns_rename_{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.join("project.blsp"), "(project :name foo)\n").unwrap();
        std::fs::write(src.join("a.blsp"), "(defmodule a)\n(defn observe (x) x)\n(observe 1)\n").unwrap();
        std::fs::write(src.join("b.blsp"), "(defmodule b)\n(defn observe (y) y)\n(observe 2)\n").unwrap();

        let mut interp = Interp::new();
        introspect::load_tooling_image(&mut interp, &dir.display().to_string()).ok();

        let a_path = src.join("a.blsp");
        let a_src = std::fs::read_to_string(&a_path).unwrap();
        let uri_a = crate::path_to_uri(&a_path.display().to_string()).unwrap();
        let uri_b = crate::path_to_uri(&src.join("b.blsp").display().to_string()).unwrap();
        let mut docs = Documents::new();
        docs.insert(
            uri_a.clone(),
            Document { text: a_src.clone(), analysis: analyze(&a_src), version: 1 },
        );

        let edit = rename(&mut interp, &docs, &uri_a, "observe", "watch")
            .expect("rename a's observe → watch");
        let changes = edit.changes.unwrap();
        assert!(
            changes.get(&uri_a).map(|v| v.len()).unwrap_or(0) >= 2,
            "a.blsp should rename observe's def + its call"
        );
        assert!(
            !changes.contains_key(&uri_b),
            "b.blsp's unrelated observe must NOT be renamed; touched: {:?}",
            changes.keys().map(|u| u.as_str()).collect::<Vec<_>>()
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
