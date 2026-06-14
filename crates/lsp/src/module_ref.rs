//! Recognize the symbol under the cursor when it names a **module or behaviour**
//! in a `defmodule` clause — the shared primitive behind goto-definition and
//! hover on `(:use foo)`, `(:alias foo)`, and `(:implements Bar)`.
//!
//! These names bind nothing in the buffer (a module/behaviour isn't a value, so
//! scope analysis resolves them `Free`), which is exactly why the generic
//! scope-driven hover/goto paths can't see them. We instead recognize them
//! *structurally* from the CST: the cursor is on the form right after a `:use` /
//! `:alias` / `:implements` keyword in a clause list. (`(require 'foo)` is handled
//! separately in [`crate::definition`] — its argument is quoted, not a bare clause
//! target, and it has no hover counterpart.)

use brood::syntax::cst::{Node, NodeKind};

/// What the symbol under the cursor names inside a `defmodule` clause.
pub enum ClauseRef<'s> {
    /// A requireable module name — `(:use foo)` / `(:alias foo)`. Navigates to the
    /// module's source file; hovers its docstring.
    Module(&'s str),
    /// A behaviour/protocol name — `(:implements Bar)`. Navigates to its
    /// `(defbehaviour …)`/`(defprotocol …)` form; hovers its declared ops.
    Behaviour(&'s str),
}

/// Classify the symbol at byte `offset`, or `None` when it isn't the target of a
/// clause we navigate. The target is the form *immediately after* the clause
/// keyword (`foo` in `(:use foo …)`); a `:use … :only [a b]` import name, or the
/// keyword itself, is not a clause target and falls through to the normal paths.
pub fn clause_ref_at<'s>(root: &Node, src: &'s str, offset: u32) -> Option<ClauseRef<'s>> {
    let node = root.node_at(offset)?;
    if node.kind != NodeKind::Symbol {
        return None;
    }
    // The innermost enclosing clause list (`(:use …)` etc.) containing the cursor.
    let mut chain = Vec::new();
    chain_to(root, offset, &mut chain);
    let clause = chain.iter().rev().find_map(|n| {
        clause_keyword(n, src).map(|kw| (*n, kw))
    });
    let (clause, kw) = clause?;
    // The target is the clause's second form; only navigate when it *is* the
    // symbol under the cursor (not a later `:only`/`:as` operand).
    let target = clause.forms().nth(1)?;
    if !std::ptr::eq(target, node) {
        return None;
    }
    match kw {
        ":use" | ":alias" => Some(ClauseRef::Module(node.text(src))),
        ":implements" => Some(ClauseRef::Behaviour(node.text(src))),
        _ => None,
    }
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

/// If `node` is a clause list whose head is one of the navigable clause keywords
/// (`:use` / `:alias` / `:implements`), its keyword text; else `None`.
fn clause_keyword<'s>(node: &Node, src: &'s str) -> Option<&'s str> {
    if node.kind != NodeKind::List {
        return None;
    }
    let head = node.forms().next()?;
    if head.kind != NodeKind::Keyword {
        return None;
    }
    matches!(head.text(src), ":use" | ":alias" | ":implements").then(|| head.text(src))
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
        clause_ref_at(root, src, at).map(|r| match r {
            ClauseRef::Module(_) => "module",
            ClauseRef::Behaviour(_) => "behaviour",
        })
    }

    #[test]
    fn use_target_is_a_module() {
        assert_eq!(kind_at("(defmodule app (:use greeter))", "greeter"), Some("module"));
    }

    #[test]
    fn alias_target_is_a_module() {
        assert_eq!(kind_at("(defmodule app (:alias web/views))", "web/views"), Some("module"));
    }

    #[test]
    fn implements_target_is_a_behaviour() {
        assert_eq!(kind_at("(defmodule app (:implements LiveModule))", "LiveModule"), Some("behaviour"));
    }

    #[test]
    fn the_keyword_itself_is_not_a_target() {
        assert_eq!(kind_at("(defmodule app (:use greeter))", ":use"), None);
    }

    #[test]
    fn an_only_import_name_is_not_a_clause_target() {
        // `greet` after `:only` is an imported name, not the module — falls through
        // to the normal Free-resolution path, not a module jump.
        assert_eq!(kind_at("(defmodule app (:use greeter :only [greet]))", "greet]"), None);
    }

    #[test]
    fn a_bare_symbol_outside_a_clause_is_not_a_target() {
        assert_eq!(kind_at("(greeter 1)", "greeter"), None);
    }
}
