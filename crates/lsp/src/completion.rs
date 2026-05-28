//! `textDocument/completion` (+ `completionItem/resolve`): name candidates at
//! the cursor. Three sources, inner-shadows-outer: **locals** visible at the
//! cursor (from the CST scope walker), the **special forms / core macros** (which
//! aren't in the global table — they're evaluator syntax, so completion would
//! otherwise never offer `if`/`let`/`fn`/`def`…), and the interpreter's
//! **globals** (prelude + builtins). The client does prefix filtering, so we
//! offer the whole visible set.
//!
//! Items ship label + kind only; the signature and docstring are filled in by
//! [`resolve`] when the client asks (`completionItem/resolve`), so building the
//! list stays cheap (no introspection eval per candidate).

use std::collections::HashSet;

use brood::syntax::scope::{BindingKind, ScopeTree};
use brood::Interp;
use lsp_types::{CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind};

use brood::introspect;

use crate::semantic_tokens::SPECIAL_FORMS;

/// Candidates visible at byte `offset`. `tree` is the document's scope analysis
/// (already built by the caller, which also parses the CST).
pub fn completions(interp: &mut Interp, tree: &ScopeTree, offset: u32) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    // Locals (and document-level defs) first — they shadow same-named globals.
    for b in tree.names_in_scope(offset) {
        if seen.insert(b.name.clone()) {
            items.push(item(
                b.name.clone(),
                match b.kind {
                    BindingKind::Local => CompletionItemKind::VARIABLE,
                    BindingKind::Global => CompletionItemKind::FUNCTION,
                },
            ));
        }
    }
    // Special forms / core macros (evaluator syntax — not in the global table).
    // One shared list with the semantic-token classifier, so they can't drift.
    for &kw in SPECIAL_FORMS {
        if seen.insert(kw.to_string()) {
            items.push(item(kw.to_string(), CompletionItemKind::KEYWORD));
        }
    }
    // Then the interpreter's globals (prelude + builtins).
    for name in introspect::global_names(interp) {
        if seen.insert(name.clone()) {
            items.push(item(name, CompletionItemKind::FUNCTION));
        }
    }
    items
}

/// Fill in an item's signature (`detail`) and docstring (`documentation`) — what
/// `completionItem/resolve` is for. Looked up by label against the interpreter's
/// introspection; a local (or anything with neither) is returned unchanged.
pub fn resolve(interp: &mut Interp, mut item: CompletionItem) -> CompletionItem {
    let (sig, doc) = introspect::signature(interp, &item.label);
    if let Some(sig) = sig {
        item.detail = Some(sig);
    }
    if let Some(doc) = doc {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc,
        }));
    }
    item
}

fn item(label: String, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn labels_at(src: &str, needle: &str) -> Vec<String> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.find(needle).unwrap() as u32;
        completions(&mut interp, &tree, at)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn offers_locals_keywords_and_globals() {
        let labels = labels_at("(defn f (x) (+ x 1))", "x 1");
        assert!(labels.contains(&"x".to_string()), "local missing");
        assert!(labels.contains(&"f".to_string()), "doc def missing");
        assert!(labels.contains(&"+".to_string()), "global missing");
        assert!(labels.contains(&"let".to_string()), "special form missing");
    }

    #[test]
    fn a_local_appears_once_even_if_it_shadows() {
        let labels = labels_at("(defn map2 (map) map)", "map)");
        assert_eq!(
            labels.iter().filter(|l| *l == "map").count(),
            1,
            "shadowing local should be de-duped: {labels:?}"
        );
    }

    #[test]
    fn resolve_attaches_a_signature_and_doc_for_a_global() {
        let mut interp = Interp::new();
        let resolved = resolve(&mut interp, item("map".into(), CompletionItemKind::FUNCTION));
        assert!(resolved.detail.unwrap().contains("(map "), "signature");
        assert!(resolved.documentation.is_some(), "doc");
    }
}
