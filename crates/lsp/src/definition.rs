//! `textDocument/definition`: jump from a symbol to its binder. Two layers,
//! the hybrid `docs/lsp.md` (ADR-031) describes:
//!
//! 1. **In-buffer** — pure CST + scope analysis ([`scope`](brood::syntax::scope)):
//!    a local resolves to its param/`let` binder, a document `def` to its name
//!    token. No interpreter needed.
//! 2. **Cross-file** — a name that's *free* in this buffer (defined in another
//!    module, or in the prelude) isn't in the CST, so we fall back to the
//!    runtime's def-site table via `(source-location 'name)`
//!    ([`introspect::source_location`]). That table is populated as the file
//!    loader runs (`note_definition`), so it answers only for modules the
//!    server's `Interp` has loaded — which is exactly what `bootstrap_project`
//!    arranges on the first `didOpen` under a project.
//!
//! A name that is neither bound here nor recorded anywhere (a builtin, or
//! genuinely unbound) has nowhere to jump — `None`.

use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::{Resolution, ScopeTree};
use brood::Interp;
use lsp_types::{Location, Position, Range, Uri};

use crate::line_index::LineIndex;
use brood::introspect;

pub fn definition(
    interp: &mut Interp,
    uri: &Uri,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Option<Location> {
    // Module navigation: a symbol that's an argument of `(require '…)` jumps to
    // the module's source file, located on the live `*load-path*` exactly as
    // `require` would. Checked first — a module name resolves `Free` in the CST
    // (it binds nothing), so the generic path below would otherwise miss it.
    if let Some(feature) = require_arg(root, text, offset) {
        if let Some(file) = introspect::module_file(interp, feature) {
            // Jump to the top of the module file.
            let top = Position::new(0, 0);
            return crate::path_to_uri(&file).map(|u| Location::new(u, Range::new(top, top)));
        }
    }
    match tree.resolve_at(root, text, offset) {
        // Bound in this buffer (local or a document-level `def`): jump to the
        // binder token, in this same file.
        Resolution::Defined { def, .. } => {
            let range = Range::new(
                index.position(text, def.start),
                index.position(text, def.end),
            );
            Some(Location::new(uri.clone(), range))
        }
        // Free here — ask the runtime where the name was defined (another
        // module, the prelude). `None` if it has no recorded site.
        Resolution::Free => {
            let node = root.node_at(offset)?;
            if node.kind != NodeKind::Symbol {
                return None;
            }
            let loc = introspect::source_location(interp, node.text(text))?;
            cross_file_location(&loc)
        }
        Resolution::NotASymbol => None,
    }
}

/// If the symbol at `offset` is an argument of a `(require …)` form, return its
/// text (the feature name). Walks the chain of nodes containing `offset` and
/// looks for an enclosing `List` whose head symbol is `require` — so it matches
/// `(require 'a 'b)` whether or not the name is quoted, and ignores a bare
/// `require` reference that isn't a call argument.
fn require_arg<'s>(root: &Node, src: &'s str, offset: u32) -> Option<&'s str> {
    let node = root.node_at(offset)?;
    if node.kind != NodeKind::Symbol {
        return None;
    }
    // The head `require` itself isn't an argument — don't navigate from it.
    let mut chain = Vec::new();
    chain_to(root, offset, &mut chain);
    let in_require = chain.iter().any(|n| head_sym(n, src) == Some("require"));
    (in_require && head_sym_is_not(&chain, src, node)).then(|| node.text(src))
}

/// The chain of nodes from `root` down to the innermost one containing `offset`.
fn chain_to<'a>(node: &'a Node, offset: u32, out: &mut Vec<&'a Node>) {
    out.push(node);
    for child in &node.children {
        if child.span.start <= offset && offset < child.span.end {
            chain_to(child, offset, out);
            break; // children don't overlap — at most one contains the offset
        }
    }
}

/// The head symbol's text of a `List` node (`require` in `(require 'a)`), or `None`.
fn head_sym<'s>(node: &Node, src: &'s str) -> Option<&'s str> {
    if node.kind != NodeKind::List {
        return None;
    }
    let first = node.forms().next()?;
    (first.kind == NodeKind::Symbol).then(|| first.text(src))
}

/// True unless `node` is itself the `require` head symbol of some list in `chain`
/// (so `M-.` on the word `require` doesn't try to open a `require.blsp`).
fn head_sym_is_not(chain: &[&Node], src: &str, node: &Node) -> bool {
    !chain.iter().any(|n| {
        n.kind == NodeKind::List
            && n.forms()
                .next()
                .map(|h| std::ptr::eq(h, node))
                .unwrap_or(false)
            && head_sym(n, src) == Some("require")
    })
}

/// Project a recorded [`introspect::SourceLoc`] (1-based line/col into some
/// other file) into an LSP [`Location`]. The position is a zero-width caret at
/// the definition's start — editors land the cursor there and select the line.
/// `line`/`col` are *character* columns; for an all-ASCII definition line (the
/// common case for a top-level `(defn …`) that equals the UTF-16 column LSP
/// wants. A non-ASCII prefix on the def line could be off by a few columns —
/// acceptable until it bites, since the jump still lands on the right line.
fn cross_file_location(loc: &introspect::SourceLoc) -> Option<Location> {
    let uri = crate::path_to_uri(&loc.file)?;
    let pos = Position::new(loc.line.saturating_sub(1), loc.col.saturating_sub(1));
    Some(Location::new(uri, Range::new(pos, pos)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn def_char_at(src: &str, needle: &str) -> Option<u32> {
        let mut interp = Interp::new();
        let uri: Uri = "file:///t.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        definition(&mut interp, &uri, src, &root, &tree, &index, at)
            .map(|l| l.range.start.character)
    }

    #[test]
    fn jumps_from_a_call_to_the_defn() {
        // The `f` call resolves to the `f` in `(defn f …)` at column 6.
        assert_eq!(def_char_at("(defn f (x) x)\n(f 1)", "f 1"), Some(6));
    }

    #[test]
    fn jumps_from_a_use_to_the_param_binder() {
        // The `x` use resolves to the param binder `x` at column 9.
        assert_eq!(def_char_at("(defn f (x) (g x))", "x))"), Some(9));
    }

    #[test]
    fn a_name_unknown_to_the_runtime_has_no_definition() {
        // `frobnicate` is neither in this buffer nor loaded anywhere.
        assert_eq!(def_char_at("(frobnicate 1)", "frobnicate"), None);
    }

    #[test]
    fn falls_back_to_a_loaded_modules_def_site() {
        // A name free in this buffer but `def`d in a file the Interp has loaded
        // resolves cross-file through `source-location`. We write a real file
        // and `load` it (the only path that records a def site), mirroring what
        // `bootstrap_project` does for a project's sources.
        let dir = std::env::temp_dir();
        let path = dir.join(format!("brood_lsp_def_{}.blsp", std::process::id()));
        std::fs::write(&path, "(defn greet (who) who)\n").unwrap();

        let mut interp = Interp::new();
        let load = format!("(load \"{}\")", path.display());
        interp.eval_str(&load).expect("load the module");

        let src = "(greet \"world\")";
        let uri: Uri = "file:///main.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find("greet").unwrap() as u32;

        let loc = definition(&mut interp, &uri, src, &root, &tree, &index, at)
            .expect("cross-file definition");
        assert!(
            loc.uri
                .as_str()
                .ends_with(&format!("brood_lsp_def_{}.blsp", std::process::id())),
            "should point at the loaded module file, got {:?}",
            loc.uri
        );
        // `greet` is the first form, column 1 → 0-based line 0, character 0.
        assert_eq!(loc.range.start, Position::new(0, 0));

        std::fs::remove_file(&path).ok();
    }
}
