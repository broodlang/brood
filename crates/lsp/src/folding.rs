//! `textDocument/foldingRange` — collapsible regions, straight off the CST.
//!
//! Two kinds, both pure structural analysis (no evaluation):
//! - every multi-line **container** (`( … )`, `[ … ]`, `{ … }`) folds, and
//! - a run of consecutive **comment** lines folds as a block.
//!
//! Folding is line-granular: a region spanning a single line is never emitted
//! (nothing to collapse).

use brood::syntax::cst::{Node, NodeKind};
use lsp_types::{FoldingRange, FoldingRangeKind};

use crate::line_index::LineIndex;

/// All folding regions in the document, in no particular order (the client
/// sorts/dedups). Containers from a recursive walk; comment blocks from a
/// line-run merge.
pub fn folding_ranges(root: &Node, text: &str, index: &LineIndex) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    collect_containers(root, text, index, &mut out);
    collect_comment_blocks(root, text, index, &mut out);
    out
}

/// Emit a region for every multi-line list/vector/map, recursing into children.
fn collect_containers(node: &Node, text: &str, index: &LineIndex, out: &mut Vec<FoldingRange>) {
    if matches!(node.kind, NodeKind::List | NodeKind::Vector | NodeKind::Map) {
        let start = index.position(text, node.span.start).line;
        // The line of the last byte *inside* the node — the closing delimiter's
        // line. `span.end` is exclusive, so step back one to stay on it.
        let end = index
            .position(text, node.span.end.saturating_sub(1))
            .line;
        if end > start {
            out.push(FoldingRange {
                start_line: start,
                end_line: end,
                kind: Some(FoldingRangeKind::Region),
                ..Default::default()
            });
        }
    }
    for child in &node.children {
        collect_containers(child, text, index, out);
    }
}

/// Merge runs of comment nodes that occupy consecutive lines into one fold.
/// Comments live as trivia at every depth, so gather them across the whole tree
/// first, then coalesce by line adjacency.
fn collect_comment_blocks(root: &Node, text: &str, index: &LineIndex, out: &mut Vec<FoldingRange>) {
    let mut lines: Vec<u32> = Vec::new();
    gather_comment_lines(root, text, index, &mut lines);
    lines.sort_unstable();
    lines.dedup();

    let mut i = 0;
    while i < lines.len() {
        let start = lines[i];
        let mut j = i;
        // Extend while the next comment is on the immediately following line.
        while j + 1 < lines.len() && lines[j + 1] == lines[j] + 1 {
            j += 1;
        }
        if lines[j] > start {
            out.push(FoldingRange {
                start_line: start,
                end_line: lines[j],
                kind: Some(FoldingRangeKind::Comment),
                ..Default::default()
            });
        }
        i = j + 1;
    }
}

fn gather_comment_lines(node: &Node, text: &str, index: &LineIndex, lines: &mut Vec<u32>) {
    if node.kind == NodeKind::Comment {
        lines.push(index.position(text, node.span.start).line);
    }
    for child in &node.children {
        gather_comment_lines(child, text, index, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    fn fold(src: &str) -> Vec<FoldingRange> {
        let root = cst::parse(src);
        let index = LineIndex::new(src);
        folding_ranges(&root, src, &index)
    }

    #[test]
    fn multi_line_list_folds() {
        let rs = fold("(defn f (x)\n  (+ x\n     1))");
        let region = rs
            .iter()
            .find(|r| r.start_line == 0 && r.kind == Some(FoldingRangeKind::Region))
            .expect("outer list fold");
        assert_eq!(region.end_line, 2);
    }

    #[test]
    fn single_line_form_does_not_fold() {
        let rs = fold("(+ 1 2)");
        assert!(rs.is_empty(), "got: {rs:?}");
    }

    #[test]
    fn consecutive_comments_fold_as_a_block() {
        let rs = fold(";; one\n;; two\n;; three\n(def a 1)");
        let block = rs
            .iter()
            .find(|r| r.kind == Some(FoldingRangeKind::Comment))
            .expect("a comment block");
        assert_eq!((block.start_line, block.end_line), (0, 2));
    }

    #[test]
    fn lone_comment_does_not_fold() {
        let rs = fold(";; just one\n(def a 1)");
        assert!(
            !rs.iter().any(|r| r.kind == Some(FoldingRangeKind::Comment)),
            "got: {rs:?}"
        );
    }
}
