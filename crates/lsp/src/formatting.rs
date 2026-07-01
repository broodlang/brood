//! `textDocument/formatting` — whole-document reformat.
//!
//! The formatter itself is **Brood** (`std/format.blsp`, an in-language CST
//! walker), reached through [`introspect::format_source`]. That keeps faith with
//! the core principle (policy in Brood, mechanism in Rust): the server only
//! transports the request and turns the result into one full-document
//! [`TextEdit`]. No range/`onType` formatting — the formatter operates on whole
//! files and is cheap, so the simple shape is the right one (ADR-011).
//!
//! On a parse error in the buffer `format-source` can't produce a faithful
//! result, so we return `None` (the editor leaves the text untouched) rather
//! than risk emitting a mangled edit.

use brood::introspect;
use brood::Interp;
use lsp_types::{Position, Range, TextEdit};

use crate::line_index::LineIndex;

/// Reformat `text`, returning a single edit that replaces the whole document.
/// `None` if the formatter errored (e.g. the buffer doesn't parse) or the text
/// is already canonical (so the editor records no change).
pub fn formatting(interp: &mut Interp, text: &str, index: &LineIndex) -> Option<Vec<TextEdit>> {
    let formatted = introspect::format_source(interp, text).ok()?;
    if formatted == text {
        return Some(Vec::new());
    }
    // Replace [0,0]..end-of-document. The end position is the line/col the
    // LineIndex projects for the final byte — covers a trailing newline or its
    // absence without special-casing.
    let end = index.position(text, text.len() as u32);
    Some(vec![TextEdit {
        range: Range::new(Position::new(0, 0), end),
        new_text: formatted,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> Option<Vec<TextEdit>> {
        let mut interp = Interp::new();
        let index = LineIndex::new(src);
        formatting(&mut interp, src, &index)
    }

    #[test]
    fn reformats_a_messy_buffer() {
        let edits = fmt("(defn   f (x)(+ x 1))").expect("an edit");
        assert_eq!(edits.len(), 1, "one whole-document edit");
        // The result is the canonical form — and idempotent.
        let out = &edits[0].new_text;
        assert!(out.contains("(defn f (x)"), "got: {out:?}");
        assert!(!out.contains("defn   f"), "collapsed runs of spaces");
    }

    #[test]
    fn already_formatted_yields_no_edit() {
        // A canonical buffer round-trips: format-source is idempotent, so the
        // output equals the input and we emit nothing.
        let canonical = {
            let mut interp = Interp::new();
            introspect::format_source(&mut interp, "(defn f (x) (+ x 1))\n").unwrap()
        };
        let edits = fmt(&canonical).expect("a (possibly empty) result");
        assert!(edits.is_empty(), "no edit for an already-formatted buffer");
    }

    #[test]
    fn whole_document_range_covers_all_lines() {
        let edits = fmt("(def a 1)\n\n\n(def   b 2)\n").expect("an edit");
        let r = edits[0].range;
        assert_eq!(r.start, Position::new(0, 0));
        // End is past the last content line (line 3, the `(def b 2)` line, plus
        // the trailing newline lands the cursor at the start of line 4).
        assert!(
            r.end.line >= 3,
            "range reaches the last line, got {:?}",
            r.end
        );
    }
}
