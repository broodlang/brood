//! `textDocument/completion`: name candidates at the cursor. Two sources, the
//! split `docs/lsp.md` describes — **locals** visible at the cursor (from the CST
//! scope walker) and the interpreter's **globals** (prelude + builtins). Locals
//! come first, so a name they shadow ranks above the global it hides; the client
//! does the prefix filtering, so we offer the whole visible set.

use std::collections::HashSet;

use brood::syntax::scope::{BindingKind, ScopeTree};
use brood::Interp;
use lsp_types::{CompletionItem, CompletionItemKind};

use brood::introspect;

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
    // Then the interpreter's globals (prelude + builtins).
    for name in introspect::global_names(interp) {
        if seen.insert(name.clone()) {
            items.push(item(name, CompletionItemKind::FUNCTION));
        }
    }
    items
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
    fn offers_locals_and_globals() {
        // Inside the body, the param `x` and the global `+` are both candidates.
        let labels = labels_at("(defn f (x) (+ x 1))", "x 1");
        assert!(labels.contains(&"x".to_string()), "local missing");
        assert!(labels.contains(&"f".to_string()), "doc def missing");
        assert!(labels.contains(&"+".to_string()), "global missing");
    }

    #[test]
    fn a_local_appears_once_even_if_it_shadows() {
        // A local named like a global must not be offered twice.
        let labels = labels_at("(defn map2 (map) map)", "map)");
        assert_eq!(
            labels.iter().filter(|l| *l == "map").count(),
            1,
            "shadowing local should be de-duped: {labels:?}"
        );
    }
}
