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

use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::{BindingKind, ScopeTree};
use brood::Interp;
use lsp_types::{CompletionItem, CompletionItemKind, Documentation, MarkupContent, MarkupKind};

use brood::introspect;

use crate::semantic_tokens::SPECIAL_FORMS;

/// Candidates visible at byte `offset`. `tree` is the document's scope analysis
/// (already built by the caller); `text` is the document source, used to read its
/// namespace + `(:use …)` imports so imported names are offered **bare** (ADR-065
/// §6). The client does prefix filtering, so we offer the whole visible set.
pub fn completions(
    interp: &mut Interp,
    tree: &ScopeTree,
    cst: &Node,
    text: &str,
    offset: u32,
) -> Vec<CompletionItem> {
    // Module-name context: inside `(require '…)` or a `(:use …)`/`(:alias …)`
    // clause the only sensible candidates are requireable modules — offer those
    // alone (a generic `+`/`if`/local would be noise there).
    if in_module_name_position(cst, offset, text) {
        return introspect::loadable_modules(interp)
            .into_iter()
            .map(|m| item(m, CompletionItemKind::MODULE))
            .collect();
    }

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    // Inside `(defimpl Proto …)`, offer the protocol's ops first (so the snippet-y
    // METHOD item shadows the generic global of the same name) — you get exactly the
    // ops you must implement, with their arities.
    if let Some(proto) = enclosing_defimpl(cst, offset, text) {
        for (name, arity) in introspect::protocol_ops(interp, &proto) {
            if seen.insert(name.clone()) {
                let mut it = item(name, CompletionItemKind::METHOD);
                it.detail = Some(format!(
                    "{} op ({} arg{})",
                    proto,
                    arity,
                    if arity == 1 { "" } else { "s" }
                ));
                items.push(it);
            }
        }
    }

    // Locals (and document-level defs) first — they shadow same-named globals.
    // (A namespaced file's own defs are document-level globals here, so they're
    // already offered bare by this path.)
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
    // `(:use …)`-imported names, offered **bare** with the qualified global stashed
    // in `data` so `resolve` can fetch its signature/doc (a bare import isn't a
    // global under its short name, so it'd otherwise be missing from the list).
    for (bare, qualified) in introspect::file_imports(interp, text) {
        if seen.insert(bare.clone()) {
            let mut it = item(bare, CompletionItemKind::FUNCTION);
            it.data = Some(serde_json::Value::String(qualified));
            items.push(it);
        }
    }
    // Then the interpreter's globals (prelude + builtins + every `mod/name` for
    // explicit qualified completion).
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
    // A bare imported item carries its qualified global in `data` (its label isn't
    // a global under its short name); everything else looks up by label.
    let lookup = item
        .data
        .as_ref()
        .and_then(|d| d.as_str())
        .unwrap_or(&item.label)
        .to_string();
    let (sig, doc) = introspect::signature(interp, &lookup);
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

/// If byte `offset` falls inside a `(defimpl Proto …)` form, the protocol name
/// `Proto`. Walks the CST for the innermost enclosing `defimpl` list (they don't
/// nest, so the first found while descending is it).
fn enclosing_defimpl(node: &Node, offset: u32, src: &str) -> Option<String> {
    // Inclusive at the end: while typing, the cursor sits *after* the last char —
    // `offset == span.end` of the still-unclosed `(defimpl …` — and we want to count
    // as inside it (`Span::contains` is end-exclusive).
    if offset < node.span.start || offset > node.span.end {
        return None;
    }
    for child in &node.children {
        if let Some(p) = enclosing_defimpl(child, offset, src) {
            return Some(p);
        }
    }
    if node.kind == NodeKind::List {
        let mut forms = node.forms();
        if forms.next().map(|n| n.text(src)) == Some("defimpl") {
            return forms.next().map(|n| n.text(src).to_string());
        }
    }
    None
}

/// True when byte `offset` sits where a **module name** belongs: an argument of
/// `(require …)`, or the module slot of a `(:use …)`/`(:alias …)` clause (after the
/// keyword, before any `:only`/`:as` marker). End-inclusive, so a cursor typing at
/// the end of a still-open form counts as inside it.
fn in_module_name_position(node: &Node, offset: u32, src: &str) -> bool {
    let Some(list) = innermost_list(node, offset) else {
        return false;
    };
    let mut forms = list.forms();
    let Some(head) = forms.next() else {
        return false;
    };
    match head.kind {
        // `(require '…)` — any slot after the head symbol.
        NodeKind::Symbol if head.text(src) == "require" => offset > head.span.end,
        // `(:use mod …)` / `(:alias mod …)` — after the keyword, before a later
        // `:only`/`:as`/`:refer` marker (so completing inside `:only [..]` doesn't
        // offer modules).
        NodeKind::Keyword if matches!(head.text(src), ":use" | ":alias") => {
            offset > head.span.end
                && !list
                    .forms()
                    .skip(1)
                    .any(|f| f.kind == NodeKind::Keyword && f.span.end <= offset)
        }
        _ => false,
    }
}

/// The innermost `List` whose span contains `offset` (end-inclusive), or `None`.
fn innermost_list(node: &Node, offset: u32) -> Option<&Node> {
    if offset < node.span.start || offset > node.span.end {
        return None;
    }
    for child in &node.children {
        if let Some(inner) = innermost_list(child, offset) {
            return Some(inner);
        }
    }
    (node.kind == NodeKind::List).then_some(node)
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
        completions(&mut interp, &tree, &root, src, at)
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
    fn offers_use_imported_names_bare() {
        // In a `(:use set)` file, `union` (a `set` export) is offered **bare**,
        // carrying its qualified target in `data` for resolve.
        let mut interp = Interp::new();
        let src = "(defmodule app (:use set))\n(uni";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.rfind("uni").unwrap() as u32;
        let items = completions(&mut interp, &tree, &root, src, at);
        let union = items.iter().find(|i| i.label == "union").expect("bare `union` offered");
        assert_eq!(
            union.data.as_ref().and_then(|d| d.as_str()),
            Some("set/union"),
            "data should carry the qualified target"
        );
        // and resolve uses that data to fetch the real signature.
        let r = resolve(&mut interp, union.clone());
        assert!(r.detail.unwrap_or_default().contains("union"), "resolved signature");
    }

    #[test]
    fn resolve_attaches_a_signature_and_doc_for_a_global() {
        let mut interp = Interp::new();
        let resolved = resolve(
            &mut interp,
            item("map".into(), CompletionItemKind::FUNCTION),
        );
        assert!(resolved.detail.unwrap().contains("(map "), "signature");
        assert!(resolved.documentation.is_some(), "doc");
    }

    #[test]
    fn module_position_detection() {
        // require argument, :use slot → module position; the head/keyword itself
        // and an :only operand → not.
        let chk = |src: &str, needle: &str| {
            let root = cst::parse(src);
            let at = src.find(needle).unwrap() as u32;
            in_module_name_position(&root, at, src)
        };
        assert!(chk("(require 'foo)", "foo"));
        assert!(chk("(defmodule a (:use foo))", "foo"));
        assert!(chk("(defmodule a (:alias foo))", "foo"));
        assert!(!chk("(require 'foo)", "require")); // on the head
        assert!(!chk("(defmodule a (:use foo))", ":use")); // on the keyword
        assert!(!chk("(defmodule a (:use foo :only [bar]))", "bar")); // past :only
        assert!(!chk("(+ 1 2)", "+")); // ordinary call
    }

    #[test]
    fn completes_module_names_in_require_and_use() {
        // A module on the load-path is offered inside `(require '…)` and `(:use …)`,
        // and the generic globals are suppressed there.
        let dir = std::env::temp_dir().join(format!("brood_modcomp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("greeter.blsp"), "(defmodule greeter)\n").unwrap();
        let mut interp = Interp::new();
        interp
            .eval_str(&format!("(def *load-path* (cons \"{}\" *load-path*))", dir.display()))
            .unwrap();

        for src in ["(require '", "(defmodule app (:use "] {
            let root = cst::parse(src);
            let tree = scope::analyze(&root, src);
            let at = src.len() as u32;
            let labels: Vec<String> = completions(&mut interp, &tree, &root, src, at)
                .into_iter()
                .map(|i| i.label)
                .collect();
            assert!(labels.contains(&"greeter".to_string()), "module missing in {src:?}: {labels:?}");
            assert!(!labels.contains(&"+".to_string()), "generic global leaked into {src:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn offers_protocol_ops_inside_defimpl() {
        // Seed the registry directly (defprotocol isn't loaded in a bare interp).
        let mut interp = Interp::new();
        interp
            .eval_str("(def *protocols* (assoc {} 'Encode (list (list 'encode '[v]))))")
            .unwrap();
        let src = "(defimpl Encode :int (enc";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.len() as u32; // cursor at end, inside the method form
        let items = completions(&mut interp, &tree, &root, src, at);
        let enc = items
            .iter()
            .find(|i| i.label == "encode")
            .expect("op `encode` offered inside (defimpl Encode …)");
        assert_eq!(enc.kind, Some(CompletionItemKind::METHOD), "tagged as a protocol op");
        assert!(enc.detail.as_deref().unwrap_or("").contains("Encode op"), "{:?}", enc.detail);
    }
}
