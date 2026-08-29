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
use crate::module_ref;
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
    // A `defmodule` clause target: `(:use foo)` / `(:alias foo)` jumps to the
    // module's file; `(:implements Bar)` jumps to the behaviour's declaration.
    // These resolve `Free` in the CST (they bind nothing), so they're handled
    // before the generic scope path below.
    match module_ref::clause_ref_at(root, text, offset) {
        Some(module_ref::ClauseRef::Module(name)) => {
            if let Some(file) = introspect::module_file(interp, name) {
                let top = Position::new(0, 0);
                return crate::path_to_uri(&file).map(|u| Location::new(u, Range::new(top, top)));
            }
            return None;
        }
        Some(module_ref::ClauseRef::Behaviour(name)) => return behaviour_location(interp, name),
        None => {}
    }
    match tree.resolve_at(root, text, offset) {
        // Bound in this buffer (local or a document-level `def`): jump to the
        // binder token, in this same file.
        Resolution::Defined { def, .. } => Some(Location::new(uri.clone(), index.range(text, def))),
        // Free here — ask the runtime where the name was defined (another
        // module, the prelude). The name is first resolved against this file's
        // namespace + `(:use …)` imports (ADR-065 §4), so a bare imported name
        // (`observe` in a `(:use observer)` file) or a qualified `observer/observe`
        // both reach the right def site. `None` if it has no recorded site.
        Resolution::Free => {
            let node = root.node_at(offset)?;
            if node.kind != NodeKind::Symbol {
                return None;
            }
            let resolved = introspect::resolve_in_source(interp, text, node.text(text));
            let loc = introspect::source_location(interp, &resolved)?;
            cross_file_location(&loc)
        }
        Resolution::NotASymbol => None,
    }
}

/// Locate the `(defbehaviour Name …)` / `(defability Name …)` that declares the
/// behaviour `name`, by scanning the project's own `.blsp` files. The interface
/// registry (`*protocols*`) records ops but not a def site, so — unlike a global —
/// there's no `source-location` to ask; we parse each project file's CST and look
/// for a top-level interface form whose name matches. `None` when no project file
/// declares it (e.g. it lives in an external package).
fn behaviour_location(interp: &mut Interp, name: &str) -> Option<Location> {
    behaviour_in_files(&introspect::project_files(interp), name)
}

/// Scan `files` for the `(defbehaviour name …)` / `(defability name …)` form and
/// return a [`Location`] on its name token. Split from [`behaviour_location`] so it
/// can be tested against an explicit file list (the live `project_files` needs a
/// bootstrapped project). Unreadable files are skipped.
fn behaviour_in_files(files: &[String], name: &str) -> Option<Location> {
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let root = brood::syntax::cst::parse(&text);
        for form in root.forms() {
            if form.kind != NodeKind::List {
                continue;
            }
            let mut head = form.forms();
            let is_iface = head.next().is_some_and(|h| {
                h.kind == NodeKind::Symbol && matches!(h.text(&text), "defbehaviour" | "defability")
            });
            let name_node = head.next();
            let matches_name =
                name_node.is_some_and(|n| n.kind == NodeKind::Symbol && n.text(&text) == name);
            if is_iface && matches_name {
                let index = LineIndex::new(&text);
                let range = index.range(&text, name_node?.span);
                return crate::path_to_uri(path).map(|u| Location::new(u, range));
            }
        }
    }
    None
}

/// Project a recorded [`introspect::SourceLoc`] (1-based line + 1-based
/// **character** column into some other file) into an LSP [`Location`]. The
/// position is a zero-width caret at the definition's start — editors land the
/// cursor there and select the line.
///
/// The column needs the target file's text to be right: `SourceLoc.col` counts
/// characters and `Position.character` counts UTF-16 code units, so treating one
/// as the other lands short on any def line with non-ASCII ahead of the name
/// (short by one per astral char). Goto-definition is user-initiated and this is
/// one small read, so we read the file and convert properly; if it can't be read
/// (deleted, unreadable, or a `%builtin-module` with no file) we fall back to the
/// raw column, which is still the right *line*.
fn cross_file_location(loc: &introspect::SourceLoc) -> Option<Location> {
    let uri = crate::path_to_uri(&loc.file)?;
    let line = loc.line.saturating_sub(1);
    let pos = match std::fs::read_to_string(&loc.file) {
        Ok(text) => {
            let index = LineIndex::new(&text);
            let off = index.offset_of_char_pos(
                &text,
                brood::error::Pos {
                    line: loc.line,
                    col: loc.col,
                },
            );
            index.position(&text, off)
        }
        Err(_) => Position::new(line, loc.col.saturating_sub(1)),
    };
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
        let load = format!("(reflect/load \"{}\")", path.display());
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

    #[test]
    fn jumps_to_an_imported_def_across_namespaces() {
        // A bare reference to an imported name (`(:use greeter)`) resolves through
        // the namespace + import table to `greeter/greet`, then to greeter's file
        // (ADR-065 §4/§6). The module must be require-able by name, so write it as
        // `greeter.blsp` on the load-path.
        let dir = std::env::temp_dir().join(format!("brood_ns_def_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("greeter.blsp"),
            "(defmodule greeter)\n(defn greet (who) who)\n",
        )
        .unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .expect("extend load-path");

        let src = "(defmodule app (:use greeter))\n(greet \"world\")";
        let uri: Uri = "file:///app.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.rfind("greet").unwrap() as u32; // the call site

        let loc = definition(&mut interp, &uri, src, &root, &tree, &index, at)
            .expect("cross-namespace goto");
        assert!(
            loc.uri.as_str().ends_with("greeter.blsp"),
            "should jump to greeter.blsp, got {:?}",
            loc.uri
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jumps_from_a_constructor_call_to_the_defrecord_in_buffer() {
        // `(defrecord point …)` registers `point` as a document global (scope.rs),
        // so goto on a constructor call resolves in-buffer to the record's name
        // token at column 11 — no loaded image needed.
        assert_eq!(
            def_char_at("(defrecord point (x y))\n(point 1 2)", "point 1"),
            Some(11)
        );
    }

    #[test]
    fn jumps_from_a_use_to_the_defability_in_buffer() {
        // Same for an ability name: a `sig` reference to `Shape` resolves to the
        // `(defability Shape …)` name token at column 12.
        assert_eq!(
            def_char_at(
                "(defability Shape (area [s]))\n(sig f (Shape -> int))",
                "Shape -"
            ),
            Some(12)
        );
    }

    #[test]
    fn jumps_to_a_defrecord_constructor_across_files() {
        // A record constructor is *synthesized* by the `defrecord` macro (which
        // expands to a `do` of `defn`s), so it isn't in the buffer's CST. The
        // loader records the expanded form's def sites, so cross-file goto on the
        // constructor still lands on the `(defrecord …)` line — the fix for
        // "can't navigate to a record constructor from another module".
        let dir = std::env::temp_dir().join(format!("brood_rec_def_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("geo.blsp"),
            "(defmodule geo)\n(defrecord circle (r))\n",
        )
        .unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .expect("extend load-path");

        let src = "(defmodule app (:use geo))\n(circle 2)";
        let uri: Uri = "file:///app.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.rfind("circle").unwrap() as u32; // the call site

        let loc = definition(&mut interp, &uri, src, &root, &tree, &index, at)
            .expect("goto to a record constructor");
        assert!(
            loc.uri.as_str().ends_with("geo.blsp"),
            "should jump to geo.blsp, got {:?}",
            loc.uri
        );
        // The `(defrecord circle …)` form is on the second line (0-based line 1).
        assert_eq!(loc.range.start.line, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jumps_from_a_use_clause_to_the_module_file() {
        // Goto-def on the module name *in the `(:use …)` clause itself* opens that
        // module's file (like `require`), located on the live `*load-path*`.
        let dir = std::env::temp_dir().join(format!("brood_use_def_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("greeter.blsp"), "(defmodule greeter)\n").unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .expect("extend load-path");

        let src = "(defmodule app (:use greeter))";
        let uri: Uri = "file:///app.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find("greeter").unwrap() as u32; // the clause target

        let loc = definition(&mut interp, &uri, src, &root, &tree, &index, at)
            .expect("goto on the :use module name");
        assert!(
            loc.uri.as_str().ends_with("greeter.blsp"),
            "should jump to greeter.blsp, got {:?}",
            loc.uri
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scans_project_files_for_the_defbehaviour() {
        // The `:implements` jump scans the project's files for the
        // `(defbehaviour Drawable …)` form and lands on its name token (line 2).
        let dir = std::env::temp_dir().join(format!("brood_impl_def_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let iface = dir.join("shapes.blsp");
        std::fs::write(
            &iface,
            "(defmodule shapes)\n(defbehaviour Drawable (draw [s]))\n",
        )
        .unwrap();

        let files = vec![iface.display().to_string()];
        let loc = behaviour_in_files(&files, "Drawable").expect("found the behaviour");
        assert!(
            loc.uri.as_str().ends_with("shapes.blsp"),
            "should jump to shapes.blsp, got {:?}",
            loc.uri
        );
        // `Drawable` is on the second line — 0-based line 1, after `(defbehaviour `.
        assert_eq!(loc.range.start.line, 1);
        assert_eq!(loc.range.start.character, 14);

        // A name no file declares has no location.
        assert!(behaviour_in_files(&files, "Nonexistent").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
