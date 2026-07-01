//! `textDocument/hover`: documentation for the symbol under the cursor. Resolve
//! it against the document's scopes, then render by what it binds to:
//! - a **local** (param / `let`) → a short "local binding" note;
//! - a **document-level `def`** → its signature + docstring, read from the CST;
//! - anything **free** (a prelude or builtin name) → its arglist + docstring from
//!   the interpreter's introspection primitives.
//!
//! No user code runs — see `docs/lsp.md`.

use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
use brood::Interp;
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::defs;
use crate::line_index::LineIndex;
use crate::module_ref;
use brood::introspect;

/// Hover for the symbol at byte `offset`. `root`/`tree`/`index` are the document's
/// already-built CST, scope analysis, and line index (the caller parses once).
pub fn hover(
    interp: &mut Interp,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Option<Hover> {
    let node = root.node_at(offset)?;
    if node.kind != NodeKind::Symbol {
        return None;
    }
    let name = node.text(text);

    // A `defmodule` clause target — the module name in `(:use …)`/`(:alias …)`, or
    // the behaviour name in `(:implements …)` — binds nothing in the buffer, so it
    // resolves `Free` below and renders nothing useful. Handle it first, off the
    // module-docs / interface registries.
    if let Some(cref) = module_ref::clause_ref_at(root, text, offset) {
        let markdown = match cref {
            module_ref::ClauseRef::Module(m) => render_module(interp, m),
            module_ref::ClauseRef::Behaviour(b) => render_behaviour(interp, b),
        };
        return markdown.map(|value| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(index.range(text, node.span)),
        });
    }

    let markdown = match tree.resolve_at(root, text, offset) {
        Resolution::Defined {
            kind: BindingKind::Local,
            ..
        } => format!("```brood\n{name}\n```\n\nlocal binding"),
        Resolution::Defined {
            def,
            kind: BindingKind::Global,
        } => defs::find_def(root, text, def)
            .map(|d| render_def(&d))
            .unwrap_or_else(|| code(name)),
        Resolution::Free => {
            // Resolve against this file's namespace + imports (ADR-065 §4) so a
            // bare imported name or a qualified `observer/observe` finds its docs.
            let resolved = introspect::resolve_in_source(interp, text, name);
            let (sig, doc) = introspect::signature(interp, &resolved);
            render_global(name, sig, doc)?
        }
        Resolution::NotASymbol => return None,
    };

    let range = index.range(text, node.span);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(range),
    })
}

fn code(text: &str) -> String {
    format!("```brood\n{text}\n```")
}

fn render_def(d: &defs::Def) -> String {
    let mut s = code(&d.signature());
    if let Some(doc) = d.doc {
        s.push_str("\n\n");
        s.push_str(doc);
    }
    s
}

/// Render a free (prelude/builtin) name. With neither a signature nor a doc
/// there's nothing useful to show, so the popup is suppressed (`None`).
fn render_global(name: &str, sig: Option<String>, doc: Option<String>) -> Option<String> {
    if sig.is_none() && doc.is_none() {
        return None;
    }
    let mut s = code(sig.as_deref().unwrap_or(name));
    if let Some(doc) = doc {
        s.push_str("\n\n");
        s.push_str(&doc);
    }
    Some(s)
}

/// Hover for a `(:use foo)` / `(:alias foo)` module name: a `(module foo)` header
/// plus the docstring its `defmodule` declared (when the module is loaded). Always
/// renders the header so the popup confirms what the name refers to.
fn render_module(interp: &mut Interp, name: &str) -> Option<String> {
    let mut s = code(&format!("(module {name})"));
    if let Some(doc) = introspect::module_doc(interp, name) {
        s.push_str("\n\n");
        s.push_str(&doc);
    }
    Some(s)
}

/// Hover for a `(:implements Bar)` behaviour name: a `(behaviour Bar)` header plus
/// its declared ops (name + arity), read from the interface registry. The ops are
/// absent when the declaring package isn't loaded — then just the header shows.
fn render_behaviour(interp: &mut Interp, name: &str) -> Option<String> {
    let mut s = code(&format!("(behaviour {name})"));
    let ops = introspect::protocol_ops(interp, name);
    if !ops.is_empty() {
        s.push_str("\n\nOps:");
        for (op, arity) in ops {
            let plural = if arity == 1 { "" } else { "s" };
            s.push_str(&format!("\n- `{op}` ({arity} arg{plural})"));
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn hover_at(src: &str, needle: &str) -> Option<String> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        hover(&mut interp, src, &root, &tree, &index, at).map(|h| match h.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup hover"),
        })
    }

    #[test]
    fn hovers_a_prelude_function_with_signature() {
        let md = hover_at("(map f xs)", "map").expect("hover on map");
        assert!(md.contains("(map "), "signature missing: {md:?}");
    }

    #[test]
    fn hovers_a_document_def_with_its_docstring() {
        let src = "(defn sq (x) \"square it\" (* x x)) (sq 3)";
        // hover on the *call site* — it resolves to the document def.
        let md = hover_at(src, "sq 3").expect("hover on sq call");
        assert!(md.contains("(sq x)"), "signature missing: {md:?}");
        assert!(md.contains("square it"), "docstring missing: {md:?}");
    }

    #[test]
    fn hovers_a_local_as_a_local_binding() {
        let md = hover_at("(defn f (x) (+ x 1))", "x 1").expect("hover on x");
        assert!(md.contains("local binding"), "{md:?}");
    }

    #[test]
    fn hovers_a_nested_def_by_finding_it_at_any_depth() {
        // `helper` is global despite being nested in a `do`; hover must still
        // render its signature, not fall back to the bare name.
        let src = "(do (defn helper (x) (* x 2))) (helper 3)";
        let md = hover_at(src, "helper 3").expect("hover on helper call");
        assert!(md.contains("(helper x)"), "signature missing: {md:?}");
    }

    #[test]
    fn hovers_a_primitive_with_signature_and_doc() {
        // Builtins now carry params + a docstring (PRIMITIVE_DOCS), so hover on a
        // primitive renders both, like a Brood function.
        let md = hover_at("(cons 1 xs)", "cons").expect("hover on cons");
        assert!(md.contains("(cons x xs)"), "signature missing: {md:?}");
        assert!(md.contains("pair"), "doc missing: {md:?}");
    }

    #[test]
    fn no_hover_on_a_literal() {
        assert!(hover_at("(+ 1 2)", "1").is_none());
    }

    #[test]
    fn hovers_a_use_module_name_with_its_docstring() {
        // Load a module with a docstring, then hover its name in a `(:use …)` clause.
        let dir = std::env::temp_dir().join(format!("brood_use_hover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("greeter.blsp"),
            "(defmodule greeter \"says hello\")\n",
        )
        .unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .expect("extend load-path");
        interp.eval_str("(require 'greeter)").expect("load greeter");

        let src = "(defmodule app (:use greeter))";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find("greeter").unwrap() as u32;
        let md = hover(&mut interp, src, &root, &tree, &index, at)
            .map(|h| match h.contents {
                HoverContents::Markup(m) => m.value,
                _ => panic!("expected markup"),
            })
            .expect("hover on the :use module name");
        assert!(md.contains("(module greeter)"), "header missing: {md:?}");
        assert!(md.contains("says hello"), "docstring missing: {md:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hovers_an_implements_behaviour_with_its_ops() {
        let mut interp = Interp::new();
        // Seed the interface registry directly (the protocol package isn't loaded in
        // a bare interp) — `*protocols*` maps a behaviour name to its op specs.
        interp
            .eval_str("(def *protocols* {'Drawable '((draw [s]) (area [s]))})")
            .expect("seed protocols");

        let src = "(defmodule app (:implements Drawable))";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find("Drawable").unwrap() as u32;
        let md = hover(&mut interp, src, &root, &tree, &index, at)
            .map(|h| match h.contents {
                HoverContents::Markup(m) => m.value,
                _ => panic!("expected markup"),
            })
            .expect("hover on the :implements behaviour name");
        assert!(
            md.contains("(behaviour Drawable)"),
            "header missing: {md:?}"
        );
        assert!(md.contains("`draw` (1 arg)"), "op missing: {md:?}");
        assert!(md.contains("`area` (1 arg)"), "op missing: {md:?}");
    }
}
