//! `textDocument/definition`: jump from a symbol to its binder. Pure CST + scope
//! analysis ([`scope`](brood::syntax::scope)): a local resolves to its
//! param/`let` binder, a document `def` to its name token. A free name (a
//! prelude/builtin, or genuinely unbound) has no binder *in this document*, so
//! there is nowhere to jump — `None`.

use brood::syntax::cst::Node;
use brood::syntax::scope::{Resolution, ScopeTree};
use lsp_types::{Location, Range, Uri};

use crate::line_index::LineIndex;

pub fn definition(
    uri: &Uri,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Option<Location> {
    let Resolution::Defined { def, .. } = tree.resolve_at(root, text, offset) else {
        return None;
    };
    let range = Range::new(index.position(text, def.start), index.position(text, def.end));
    Some(Location::new(uri.clone(), range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn def_char_at(src: &str, needle: &str) -> Option<u32> {
        let uri: Uri = "file:///t.blsp".parse().unwrap();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        definition(&uri, src, &root, &tree, &index, at).map(|l| l.range.start.character)
    }

    #[test]
    fn jumps_from_a_call_to_the_defn() {
        // The `f` call resolves to the `f` in `(defn f …)` at column 6.
        assert_eq!(def_char_at("(defn f (x) x)\n(f 1)", "f 1"), Some(6));
    }

    #[test]
    fn jumps_from_a_use_to_the_param_binder() {
        // The `x` use resolves to the param binder `x` at column 9.
        assert_eq!(def_char_at("(defn f (x) (g x))", "x))"), Some(9));
    }

    #[test]
    fn a_free_name_has_no_definition() {
        // `+` isn't defined in this document.
        assert_eq!(def_char_at("(+ 1 2)", "+"), None);
    }
}
