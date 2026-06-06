//! `textDocument/documentSymbol`: the outline of a file — its top-level
//! `def`/`defn`/`defmacro` forms (see [`defs`](crate::defs)). A pure CST walk, no
//! evaluation. The full form is the symbol's `range`; the name token is its
//! `selection_range` (what the editor highlights when you pick it).

use brood::syntax::cst::Node;
use lsp_types::{DocumentSymbol, SymbolKind};

use crate::defs::{self, DefKind};
use crate::line_index::LineIndex;

pub fn document_symbols(root: &Node, text: &str, index: &LineIndex) -> Vec<DocumentSymbol> {
    defs::top_level(root, text)
        .into_iter()
        .map(|d| {
            #[allow(deprecated)] // the `deprecated` field is required by the struct
            DocumentSymbol {
                name: d.name.to_string(),
                detail: Some(format!("{} {}", d.kind.keyword(), d.signature())),
                kind: match d.kind {
                    DefKind::Var => SymbolKind::VARIABLE,
                    DefKind::Fn | DefKind::Macro => SymbolKind::FUNCTION,
                },
                tags: None,
                deprecated: None,
                range: index.range(text, d.full_span),
                selection_range: index.range(text, d.name_span),
                children: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    #[test]
    fn outlines_top_level_defs() {
        let src = "(def pi 3.14)\n(defn sq (x) (* x x))\n(defmacro m (x) x)";
        let root = cst::parse(src);
        let index = LineIndex::new(src);
        let syms = document_symbols(&root, src, &index);
        let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["pi", "sq", "m"]);
        assert_eq!(syms[0].kind, SymbolKind::VARIABLE);
        assert_eq!(syms[1].kind, SymbolKind::FUNCTION);
        // The selection range covers the name, the full range the whole form.
        assert_eq!(syms[1].selection_range.start.character, 6); // `sq` after `(defn `
    }
}
