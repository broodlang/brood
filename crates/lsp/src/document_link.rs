//! `textDocument/documentLink`: make the module names that name *files* clickable
//! — Ctrl-click to open. We link every module reference the buffer mentions in a
//! load position: a `(require 'foo)` argument and a `(:use foo)` / `(:alias foo)`
//! `defmodule` clause. Each resolves to its source file the same way `require`
//! does (`introspect::module_file` over `*load-path*`); a name with no file (a
//! baked-in std module, or one not on the path) simply gets no link.
//!
//! This complements goto-definition (`definition.rs` / `module_ref.rs`): goto is
//! cursor-driven and also covers `:implements`; document links are the
//! editor's *passive* underlines over every linkable module name at once.

use brood::syntax::cst::{Node, NodeKind};
use brood::Interp;
use lsp_types::DocumentLink;

use crate::line_index::LineIndex;
use brood::introspect;

/// Every module-name reference in the buffer that resolves to a file, as a
/// clickable link over the name's span.
pub fn document_links(
    interp: &mut Interp,
    text: &str,
    root: &Node,
    index: &LineIndex,
) -> Vec<DocumentLink> {
    let mut names = Vec::new();
    collect_module_names(root, text, &mut names);
    names
        .into_iter()
        .filter_map(|node| {
            let name = node.text(text);
            let file = introspect::module_file(interp, name)?;
            let target = crate::path_to_uri(&file)?;
            Some(DocumentLink {
                range: index.range(text, node.span),
                target: Some(target),
                tooltip: Some(format!("Open module {name}")),
                data: None,
            })
        })
        .collect()
}

/// Walk the CST collecting the symbol nodes that name a requireable module: the
/// quoted argument(s) of a `(require …)` call, and the name in a `(:use …)` /
/// `(:alias …)` clause. Recurses into every node so nested clauses are found.
fn collect_module_names<'a>(node: &'a Node, src: &str, out: &mut Vec<&'a Node>) {
    if node.kind == NodeKind::List {
        let mut forms = node.forms();
        if let Some(head) = forms.next() {
            match head.kind {
                // `(require 'a 'b …)` — each (quoted) symbol argument.
                NodeKind::Symbol if head.text(src) == "require" => {
                    for arg in forms {
                        if let Some(sym) = quoted_symbol(arg) {
                            out.push(sym);
                        }
                    }
                }
                // `(:use foo …)` / `(:alias foo …)` — the form right after the keyword.
                NodeKind::Keyword if matches!(head.text(src), ":use" | ":alias") => {
                    if let Some(name) = node.forms().nth(1) {
                        if name.kind == NodeKind::Symbol {
                            out.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for child in &node.children {
        collect_module_names(child, src, out);
    }
}

/// The inner symbol of a `'sym` quote node, or `None` for any other form.
fn quoted_symbol(node: &Node) -> Option<&Node> {
    if node.kind != NodeKind::Quote {
        return None;
    }
    let inner = node.forms().next()?;
    (inner.kind == NodeKind::Symbol).then_some(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    /// Resolve links against a temp dir holding `greeter.blsp`, returning the
    /// linked name texts (sorted) — proves both the require and `:use` positions
    /// link, and that an unresolvable name (`nope`) doesn't.
    fn linked_names(tag: &str, src: &str) -> Vec<String> {
        // Unique per test: cargo runs these on threads of one process, so a
        // pid-only dir would be shared and racily removed by a sibling test.
        let dir = std::env::temp_dir().join(format!("brood_doclink_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("greeter.blsp"), "(defmodule greeter)\n").unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!("(def *load-path* (cons \"{}\" *load-path*))", dir.display()))
            .expect("extend load-path");

        let root = cst::parse(src);
        let index = LineIndex::new(src);
        let links = document_links(&mut interp, src, &root, &index);
        std::fs::remove_dir_all(&dir).ok();
        let mut got: Vec<String> = links
            .iter()
            .map(|l| src[l.range_bytes(src)].to_string())
            .collect();
        got.sort();
        got
    }

    // Tiny helper: recover the byte slice a link covers (UTF-16 → bytes is overkill
    // for the ASCII test sources, so re-find the name by its 0-based line/char).
    trait RangeBytes {
        fn range_bytes(&self, src: &str) -> std::ops::Range<usize>;
    }
    impl RangeBytes for DocumentLink {
        fn range_bytes(&self, src: &str) -> std::ops::Range<usize> {
            let line_start = src
                .split_inclusive('\n')
                .take(self.range.start.line as usize)
                .map(str::len)
                .sum::<usize>();
            let start = line_start + self.range.start.character as usize;
            let end = line_start + self.range.end.character as usize;
            start..end
        }
    }

    #[test]
    fn links_a_require_argument() {
        assert_eq!(linked_names("require", "(require 'greeter)"), vec!["greeter"]);
    }

    #[test]
    fn links_a_use_clause_module() {
        assert_eq!(linked_names("use", "(defmodule app (:use greeter))"), vec!["greeter"]);
    }

    #[test]
    fn skips_a_module_with_no_file() {
        assert!(linked_names("nofile", "(require 'nope)").is_empty());
    }

    #[test]
    fn links_both_positions_in_one_file() {
        let src = "(defmodule app (:use greeter))\n(require 'greeter)";
        assert_eq!(linked_names("both", src), vec!["greeter", "greeter"]);
    }
}
