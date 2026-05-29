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
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Range};

use crate::defs;
use crate::line_index::LineIndex;
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
            let (sig, doc) = introspect::signature(interp, name);
            render_global(name, sig, doc)?
        }
        Resolution::NotASymbol => return None,
    };

    let range = Range::new(
        index.position(text, node.span.start),
        index.position(text, node.span.end),
    );
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
}
