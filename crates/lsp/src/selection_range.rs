//! `textDocument/selectionRange`: smart expand/shrink selection. For each cursor
//! position the editor gets a chain of progressively larger ranges to grow the
//! selection through — and Lisp's structure makes this especially natural: a
//! symbol → its enclosing list → the outer list → … → the whole file, read
//! straight off the CST. No interpreter, no scope analysis — pure tree geometry.

use brood::syntax::cst::Node;
use lsp_types::{Position, Range, SelectionRange};

use crate::line_index::LineIndex;

/// One [`SelectionRange`] chain per input position (the LSP contract: the result
/// list is positional with `positions`).
pub fn selection_ranges(
    root: &Node,
    text: &str,
    index: &LineIndex,
    positions: &[Position],
) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|&pos| {
            let offset = index.offset(text, pos);
            selection_at(root, text, index, offset, pos)
        })
        .collect()
}

/// The expand-selection chain at byte `offset`: the CST nodes containing it, from
/// the whole file inward to the tightest form, linked outermost-as-parent so the
/// editor grows the selection one structural level per keystroke. Trivia
/// (whitespace / comments) and duplicate spans (a wrapper sharing its child's
/// extent) are skipped so each expansion is a visible jump.
fn selection_at(
    root: &Node,
    text: &str,
    index: &LineIndex,
    offset: u32,
    pos: Position,
) -> SelectionRange {
    let mut chain = Vec::new();
    node_chain(root, offset, &mut chain);

    let mut sel: Option<SelectionRange> = None;
    let mut last: Option<Range> = None;
    for node in chain.into_iter().filter(|n| !n.kind.is_trivia()) {
        let range = index.range(text, node.span);
        if Some(range) == last {
            continue; // a wrapper with the same extent — don't add a no-op level
        }
        sel = Some(SelectionRange {
            range,
            parent: sel.map(Box::new),
        });
        last = Some(range);
    }
    // Cursor outside any structural node (empty file / past EOF): a zero-width
    // range at the position, with no parent.
    sel.unwrap_or(SelectionRange {
        range: Range::new(pos, pos),
        parent: None,
    })
}

/// The path of nodes from `root` down to the innermost one containing `offset`
/// (root first). Mirrors `Node::node_at` but keeps every ancestor on the way.
fn node_chain<'a>(node: &'a Node, offset: u32, out: &mut Vec<&'a Node>) {
    out.push(node);
    for child in &node.children {
        if child.span.start <= offset && offset < child.span.end {
            node_chain(child, offset, out);
            break; // children don't overlap — at most one contains the offset
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    /// The chain of range texts from innermost to outermost at `needle`.
    fn ranges_at(src: &str, needle: &str) -> Vec<String> {
        let root = cst::parse(src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        let pos = index.range(src, brood::error::Span { start: at, end: at }).start;
        let mut out = Vec::new();
        let mut cur = Some(selection_at(&root, src, &index, at, pos));
        while let Some(sr) = cur {
            // Recover the byte slice the range covers (ASCII test sources).
            let s = byte_slice(src, &sr.range);
            out.push(s.to_string());
            cur = sr.parent.map(|b| *b);
        }
        out
    }

    fn byte_slice<'s>(src: &'s str, r: &Range) -> &'s str {
        let line_start = |line: u32| {
            src.split_inclusive('\n')
                .take(line as usize)
                .map(str::len)
                .sum::<usize>()
        };
        let start = line_start(r.start.line) + r.start.character as usize;
        let end = line_start(r.end.line) + r.end.character as usize;
        &src[start..end]
    }

    #[test]
    fn expands_symbol_to_list_to_outer_list() {
        // Cursor on `x` grows: x → (+ x 1) → (defn f (x) (+ x 1)) → whole file.
        let chain = ranges_at("(defn f (y) (+ x 1))", "x 1");
        assert_eq!(chain[0], "x");
        assert_eq!(chain[1], "(+ x 1)");
        assert_eq!(chain[2], "(defn f (y) (+ x 1))");
        // Outermost is the whole document.
        assert_eq!(chain.last().unwrap(), "(defn f (y) (+ x 1))");
    }

    #[test]
    fn innermost_first_each_level_grows() {
        let chain = ranges_at("(a (b c))", "c");
        assert_eq!(chain[0], "c");
        assert_eq!(chain[1], "(b c)");
        assert_eq!(chain[2], "(a (b c))");
        // Strictly increasing length — each step is a real expansion.
        for w in chain.windows(2) {
            assert!(w[1].len() > w[0].len(), "not growing: {chain:?}");
        }
    }

    #[test]
    fn one_chain_per_position() {
        let src = "(a b)";
        let root = cst::parse(src);
        let index = LineIndex::new(src);
        let positions = vec![Position::new(0, 1), Position::new(0, 3)];
        let out = selection_ranges(&root, src, &index, &positions);
        assert_eq!(out.len(), 2);
    }
}
