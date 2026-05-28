//! A scope resolver over the tooling [`cst`](super::cst). Given a symbol
//! occurrence, it answers *"what does this name bind to here?"* — the engine
//! shared by go-to-definition, find-references, rename, "unbound" diagnostics,
//! and locals-in-scope completion (ADR-025 / `docs/lsp.md`). It analyses the
//! **un-expanded** CST, so it never sees through macros (the deliberate limit
//! `tooling.md` also accepts for runtime positions).
//!
//! First cut (ADR-011 — ship the simple shape): **plain-symbol binders only**.
//! - Globals: `def` / `defn` / `defmacro` names (global wherever they appear —
//!   `def` always defines in the global env).
//! - Locals: `fn` / `lambda` / `defn` / `defmacro` params (incl. `&optional`
//!   `(name default)` groups and `& rest`), and `let` / `let*` binding names.
//!
//! **Deferred:** destructuring *pattern* binders (ADR-021 — `(let ([a b] v) …)`,
//! `(fn ((h & t)) …)`, `match` clause patterns). Non-symbol binder targets are
//! skipped, not bound, until that walk is added.

use std::collections::HashSet;

use crate::error::Span;
use crate::syntax::cst::{Node, NodeKind};

/// Whether a binding is global (a top-level `def`-family name) or a lexical
/// local (a parameter or `let` binding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    Global,
    Local,
}

/// A name introduced somewhere in the document, with the span of its binder.
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    /// The span of the binding occurrence (where go-to-definition lands).
    pub def: Span,
    pub kind: BindingKind,
}

/// What a symbol occurrence refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Bound in this document — a local or a top-level def. `def` is where
    /// go-to-definition lands.
    Defined { def: Span, kind: BindingKind },
    /// No binder in this document. It may still be a global/builtin (ask the
    /// runtime with `bound?`) or be genuinely unbound.
    Free,
    /// The offset isn't on a symbol (it's a literal, a list, trivia, …).
    NotASymbol,
}

struct ScopeNode {
    /// The source region this scope covers.
    span: Span,
    parent: Option<usize>,
    bindings: Vec<Binding>,
}

/// The scopes of one document, ready to answer resolution queries. Build with
/// [`analyze`].
pub struct ScopeTree {
    scopes: Vec<ScopeNode>,
}

/// Build the scope tree for a parsed document.
pub fn analyze(root: &Node, src: &str) -> ScopeTree {
    let mut tree = ScopeTree {
        scopes: vec![ScopeNode {
            span: root.span,
            parent: None,
            bindings: Vec::new(),
        }],
    };
    // Pass 1: every `def`-family name is global, wherever it appears.
    collect_globals(root, src, &mut tree.scopes[0].bindings);
    // Pass 2: nested lexical scopes for params and `let` bindings.
    build(root, src, 0, &mut tree);
    tree
}

impl ScopeTree {
    /// Resolve the symbol at byte `offset` against the document `root`/`src`.
    pub fn resolve_at(&self, root: &Node, src: &str, offset: u32) -> Resolution {
        match root.node_at(offset) {
            Some(n) if n.kind == NodeKind::Symbol => self.resolve(offset, n.text(src)),
            _ => Resolution::NotASymbol,
        }
    }

    /// Resolve `name` as seen from byte `offset`. Walks inner→outer scopes.
    pub fn resolve(&self, offset: u32, name: &str) -> Resolution {
        let mut cur = Some(self.scope_at(offset));
        while let Some(i) = cur {
            if let Some(b) = self.scopes[i].bindings.iter().find(|b| b.name == name) {
                return Resolution::Defined {
                    def: b.def,
                    kind: b.kind,
                };
            }
            cur = self.scopes[i].parent;
        }
        Resolution::Free
    }

    /// Every name visible at `offset`, inner shadowing outer (for completion).
    pub fn names_in_scope(&self, offset: u32) -> Vec<&Binding> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = Some(self.scope_at(offset));
        while let Some(i) = cur {
            for b in &self.scopes[i].bindings {
                if seen.insert(b.name.as_str()) {
                    out.push(b);
                }
            }
            cur = self.scopes[i].parent;
        }
        out
    }

    /// Every occurrence (span) that resolves to the *same* binding as the symbol
    /// at `offset` — for find-references / document-highlight / rename. A local
    /// is naturally scoped (occurrences outside its scope resolve elsewhere); a
    /// free name matches its free occurrences across the document.
    pub fn references(&self, root: &Node, src: &str, offset: u32) -> Vec<Span> {
        let want = self.resolve_at(root, src, offset);
        let name = match root.node_at(offset) {
            Some(n) if n.kind == NodeKind::Symbol => n.text(src),
            _ => return Vec::new(),
        };
        if want == Resolution::NotASymbol {
            return Vec::new();
        }
        let mut syms = Vec::new();
        collect_symbols(root, &mut syms);
        syms.iter()
            .filter(|n| n.text(src) == name)
            .filter(|n| same_binding(self.resolve(n.span.start, name), want))
            .map(|n| n.span)
            .collect()
    }

    /// Every occurrence of `name` in this document that refers to the file's
    /// **global** binding (a top-level `def`) or is **free** (defined in another
    /// module) — i.e. every use of the cross-module global `name`, *excluding*
    /// any local that happens to share the spelling (those resolve to a `Local`
    /// binding and are skipped). The building block for **cross-file** references
    /// and rename: under the flat module model (ADR-019) a global is one binding
    /// everywhere, so the union of this over every project file is the global's
    /// full reference set. (Contrast [`references`], which keys off a cursor and
    /// stays within one binding in one document.)
    pub fn references_to_global(&self, root: &Node, src: &str, name: &str) -> Vec<Span> {
        let mut syms = Vec::new();
        collect_symbols(root, &mut syms);
        syms.iter()
            .filter(|n| n.text(src) == name)
            .filter(|n| {
                matches!(
                    self.resolve(n.span.start, name),
                    Resolution::Defined { kind: BindingKind::Global, .. } | Resolution::Free
                )
            })
            .map(|n| n.span)
            .collect()
    }

    /// The innermost scope whose span contains `offset` (smallest containing).
    fn scope_at(&self, offset: u32) -> usize {
        let mut best = 0;
        let mut best_len = u32::MAX;
        for (i, s) in self.scopes.iter().enumerate() {
            if s.span.contains(offset) {
                let len = s.span.end - s.span.start;
                if len <= best_len {
                    best = i;
                    best_len = len;
                }
            }
        }
        best
    }
}

fn same_binding(a: Resolution, b: Resolution) -> bool {
    match (a, b) {
        (Resolution::Defined { def: d1, .. }, Resolution::Defined { def: d2, .. }) => d1 == d2,
        (Resolution::Free, Resolution::Free) => true,
        _ => false,
    }
}

/// The head symbol of a list form, e.g. `def` in `(def x 1)`.
fn head_sym<'s>(node: &Node, src: &'s str) -> Option<&'s str> {
    if node.kind != NodeKind::List {
        return None;
    }
    let first = node.forms().next()?;
    (first.kind == NodeKind::Symbol).then(|| first.text(src))
}

/// Pass 1: collect `def`/`defn`/`defmacro` names into the global scope.
fn collect_globals(node: &Node, src: &str, out: &mut Vec<Binding>) {
    if let Some(head) = head_sym(node, src) {
        if matches!(head, "def" | "defn" | "defmacro") {
            if let Some(name) = node.forms().nth(1) {
                if name.kind == NodeKind::Symbol {
                    out.push(Binding {
                        name: name.text(src).to_string(),
                        def: name.span,
                        kind: BindingKind::Global,
                    });
                }
            }
        }
    }
    for c in &node.children {
        collect_globals(c, src, out);
    }
}

/// Pass 2: descend, opening a child scope at each binding form.
fn build(node: &Node, src: &str, current: usize, tree: &mut ScopeTree) {
    let opened = match head_sym(node, src) {
        Some("let") | Some("let*") => Some(let_names(node, src)),
        Some("fn") | Some("lambda") => Some(param_names(node, src, 1)),
        // defn/defmacro: name at index 1, param list at 2.
        Some("defn") | Some("defmacro") => Some(param_names(node, src, 2)),
        _ => None,
    };
    let scope = match opened {
        Some(bindings) => {
            let id = tree.scopes.len();
            tree.scopes.push(ScopeNode {
                span: node.span,
                parent: Some(current),
                bindings,
            });
            id
        }
        None => current,
    };
    for c in &node.children {
        build(c, src, scope, tree);
    }
}

/// The symbol names bound by a `let` binding list — names at even positions.
/// Non-symbol targets (patterns) are skipped (deferred).
fn let_names(node: &Node, src: &str) -> Vec<Binding> {
    let mut out = Vec::new();
    if let Some(binds) = node.forms().nth(1) {
        if matches!(binds.kind, NodeKind::List | NodeKind::Vector) {
            for (i, item) in binds.forms().enumerate() {
                if i % 2 == 0 && item.kind == NodeKind::Symbol {
                    out.push(Binding {
                        name: item.text(src).to_string(),
                        def: item.span,
                        kind: BindingKind::Local,
                    });
                }
            }
        }
    }
    out
}

/// The symbol names bound by a parameter list at form-index `idx`. Skips the
/// `&optional` / `&` markers; for an `(name default)` optional group, binds
/// `name`. Non-symbol pattern targets are skipped (deferred).
fn param_names(node: &Node, src: &str, idx: usize) -> Vec<Binding> {
    let mut out = Vec::new();
    let Some(params) = node.forms().nth(idx) else {
        return out;
    };
    if !matches!(params.kind, NodeKind::List | NodeKind::Vector) {
        return out;
    }
    for item in params.forms() {
        match item.kind {
            NodeKind::Symbol => {
                let t = item.text(src);
                if t != "&optional" && t != "&" {
                    out.push(Binding {
                        name: t.to_string(),
                        def: item.span,
                        kind: BindingKind::Local,
                    });
                }
            }
            // An `&optional` group `(name default)` — the binder is the head.
            NodeKind::List => {
                if let Some(n) = item.forms().next() {
                    if n.kind == NodeKind::Symbol {
                        out.push(Binding {
                            name: n.text(src).to_string(),
                            def: n.span,
                            kind: BindingKind::Local,
                        });
                    }
                }
            }
            _ => {} // a vector/other pattern target — deferred
        }
    }
    out
}

fn collect_symbols<'t>(node: &'t Node, out: &mut Vec<&'t Node>) {
    if node.kind == NodeKind::Symbol {
        out.push(node);
    }
    for c in &node.children {
        collect_symbols(c, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::cst;

    /// Resolve the symbol at the first occurrence of `needle` in `src`.
    fn resolve(src: &str, needle: &str) -> Resolution {
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let at = src.find(needle).unwrap() as u32;
        tree.resolve_at(&root, src, at)
    }

    #[test]
    fn top_level_def_resolves_globally() {
        let src = "(defn f (x) (f x))"; // the call `f` refers to the defn
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let call = src.rfind('f').unwrap() as u32; // the `f` in `(f x)`
        match tree.resolve_at(&root, src, call) {
            Resolution::Defined { kind, .. } => assert_eq!(kind, BindingKind::Global),
            r => panic!("expected Global, got {:?}", r),
        }
    }

    #[test]
    fn let_binding_is_local_and_scoped() {
        // `a` resolves inside the let body, but is Free after the let closes.
        let src = "(do (let (a 1) a) a)";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let inside = src.find("a) a").unwrap() as u32; // the `a` in the body
        let outside = src.rfind('a').unwrap() as u32; // the trailing `a`
        assert!(matches!(
            tree.resolve_at(&root, src, inside),
            Resolution::Defined {
                kind: BindingKind::Local,
                ..
            }
        ));
        assert_eq!(tree.resolve_at(&root, src, outside), Resolution::Free);
    }

    #[test]
    fn fn_params_resolve_including_optional_and_rest() {
        let src = "(fn (a &optional (b 1) & cs) (list a b cs))";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        for name in ["a", "b", "cs"] {
            // the occurrence inside the body `(list a b cs)`
            let body = src.find("list").unwrap();
            let at = (body + src[body..].find(name).unwrap()) as u32;
            assert!(
                matches!(
                    tree.resolve_at(&root, src, at),
                    Resolution::Defined {
                        kind: BindingKind::Local,
                        ..
                    }
                ),
                "{name} should be a local param"
            );
        }
    }

    #[test]
    fn inner_binding_shadows_outer() {
        let src = "(defn f (x) (let (x 9) x))";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let inner_x = src.rfind('x').unwrap() as u32; // the `x` in the let body
        let r = tree.resolve_at(&root, src, inner_x);
        // It must resolve to the let's `x` (the binder right before the body),
        // not the param `x`.
        let let_x = src.find("x 9").unwrap() as u32;
        assert_eq!(
            r,
            Resolution::Defined {
                def: Span::new(let_x as usize, let_x as usize + 1),
                kind: BindingKind::Local
            }
        );
    }

    #[test]
    fn free_symbol_is_free() {
        // `+` isn't defined in this document → Free (a runtime global/builtin).
        assert_eq!(resolve("(+ 1 2)", "+"), Resolution::Free);
    }

    #[test]
    fn non_symbol_offset_is_not_a_symbol() {
        assert_eq!(resolve("(f 123)", "123"), Resolution::NotASymbol);
    }

    #[test]
    fn references_find_all_uses_of_a_global() {
        let src = "(defn f (x) x) (f 1) (f 2)";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let at = src.find("f (").unwrap() as u32; // the defn name (not the `f` in "defn")
        let refs = tree.references(&root, src, at);
        assert_eq!(refs.len(), 3, "the def name + two calls: {:?}", refs);
    }

    #[test]
    fn references_to_a_local_stay_in_scope() {
        // Two separate `x`s: the param's, and an unrelated outer `x` use.
        let src = "(defn f (x) (g x x)) x";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let param = src.find("x)").unwrap() as u32; // the param binder
        let refs = tree.references(&root, src, param);
        // binder + the two uses inside the body = 3; the trailing free `x` is excluded.
        assert_eq!(refs.len(), 3, "{:?}", refs);
    }

    #[test]
    fn references_to_global_collects_globals_and_frees_but_not_locals() {
        // `f` is a doc global; `g` is free (defined elsewhere); a `let`-bound `f`
        // in another form shadows the global and must be excluded.
        let src = "(defn f (x) (g x)) (f 1) (let (f 9) f)";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        // `f` as a global: the def name + the `(f 1)` call = 2. The `let`-bound
        // `f` (binder + body use) are Local → excluded.
        assert_eq!(tree.references_to_global(&root, src, "f").len(), 2);
        // `g` is free here: its single use.
        assert_eq!(tree.references_to_global(&root, src, "g").len(), 1);
    }

    #[test]
    fn names_in_scope_includes_locals_and_globals() {
        let src = "(defn f (x) (let (y 1) y))";
        let root = cst::parse(src);
        let tree = analyze(&root, src);
        let at = src.rfind('y').unwrap() as u32; // in the let body
        let names: Vec<&str> = tree
            .names_in_scope(at)
            .iter()
            .map(|b| b.name.as_str())
            .collect();
        for expected in ["y", "x", "f"] {
            assert!(
                names.contains(&expected),
                "{expected} should be in scope: {names:?}"
            );
        }
    }
}
