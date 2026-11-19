//! Recognize the symbol under the cursor when it names a **module** in a `defmodule`
//! clause — the shared primitive behind goto-definition and hover on `(:use foo)` and
//! `(:alias foo)`.
//!
//! These names bind nothing in the buffer (a module isn't a value, so scope analysis
//! resolves them `Free`), which is exactly why the generic scope-driven hover/goto paths
//! can't see them. We instead recognize them *structurally* from the CST: the cursor is
//! on the form right after a `:use` / `:alias` keyword in a clause list. (There is no
//! `require` form in the language — a module loads only via `(:use)`/`(:alias)` or a
//! qualified `mod/name` reference.)

use brood::syntax::cst::Node;

/// The module the symbol at byte `offset` names as a clause target, or `None` when it
/// isn't one. The target is the form *immediately after* the clause keyword (`foo` in
/// `(:use foo …)`); a `:use … :only [a b]` import name, or the keyword itself, is not a
/// clause target and falls through to the normal paths.
pub fn clause_module_at<'s>(root: &Node, src: &'s str, offset: u32) -> Option<&'s str> {
    let node = root.symbol_at(offset)?;
    // The innermost enclosing `(:use …)` / `(:alias …)` clause list containing the cursor.
    let mut chain = Vec::new();
    chain_to(root, offset, &mut chain);
    let clause = chain
        .iter()
        .rev()
        .find(|n| matches!(n.head_keyword(src), Some(":use" | ":alias")))?;
    // The target is the clause's second form; only navigate when it *is* the
    // symbol under the cursor (not a later `:only`/`:as` operand).
    let target = clause.forms().nth(1)?;
    std::ptr::eq(target, node).then(|| node.text(src))
}

/// The chain of nodes from `root` down to the innermost one containing `offset`.
pub(crate) fn chain_to<'a>(node: &'a Node, offset: u32, out: &mut Vec<&'a Node>) {
    out.push(node);
    for child in &node.children {
        if child.span.start <= offset && offset < child.span.end {
            chain_to(child, offset, out);
            break; // children don't overlap — at most one contains the offset
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    fn kind_at(src: &str, needle: &str) -> Option<&'static str> {
        // Leak the parse so the returned &str can name a static kind tag — test-only.
        let root = Box::leak(Box::new(cst::parse(src)));
        let src: &'static str = Box::leak(src.to_string().into_boxed_str());
        let at = src.find(needle).unwrap() as u32;
        clause_module_at(root, src, at).map(|_| "module")
    }

    #[test]
    fn use_target_is_a_module() {
        assert_eq!(
            kind_at("(defmodule app (:use greeter))", "greeter"),
            Some("module")
        );
    }

    #[test]
    fn alias_target_is_a_module() {
        assert_eq!(
            kind_at("(defmodule app (:alias web/views))", "web/views"),
            Some("module")
        );
    }

    #[test]
    fn an_implements_clause_target_is_not_navigated() {
        // `(:implements …)` is gone (ADR-761); a stale one in a buffer is not a module.
        assert_eq!(
            kind_at("(defmodule app (:implements LiveModule))", "LiveModule"),
            None
        );
    }

    #[test]
    fn the_keyword_itself_is_not_a_target() {
        assert_eq!(kind_at("(defmodule app (:use greeter))", ":use"), None);
    }

    #[test]
    fn an_only_import_name_is_not_a_clause_target() {
        // `greet` after `:only` is an imported name, not the module — falls through
        // to the normal Free-resolution path, not a module jump.
        assert_eq!(
            kind_at("(defmodule app (:use greeter :only [greet]))", "greet]"),
            None
        );
    }

    #[test]
    fn a_bare_symbol_outside_a_clause_is_not_a_target() {
        assert_eq!(kind_at("(greeter 1)", "greeter"), None);
    }
}
