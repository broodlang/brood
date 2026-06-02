//! Display-cell width of text — how many terminal/grid columns a string occupies.
//!
//! Shared by the `display-width` builtin (so Brood code — the editor's column /
//! cursor math — can ask) and the GUI renderer (`gui.rs`, which advances the cell
//! grid one *cluster* at a time, not one codepoint). One definition, so the two can
//! never disagree about where a wide glyph ends.
//!
//! The rule: segment into **grapheme clusters** (a ZWJ emoji, a flag, a base +
//! combining marks, a skin-tone sequence are each *one* cluster), then each cluster
//! is 0 cells (pure combining / zero-width), 1 cell (normal), or 2 cells (wide — CJK
//! and emoji). We clamp `unicode-width`'s per-cluster sum to {0,1,2}: a multi-codepoint
//! emoji sums to more than 2 by codepoint, but occupies one double-width cell.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The display width, in cells, of a single grapheme cluster: 0 (zero-width /
/// combining), 1 (normal), or 2 (wide — CJK / emoji). Clamps the codepoint-sum width
/// so a multi-codepoint emoji cluster is one 2-cell glyph, not its component count.
pub fn cluster_cells(cluster: &str) -> usize {
    match cluster.width() {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

/// The display width, in cells, of `s`: the sum of its grapheme clusters' widths.
/// `(display-width "a😀b")` is 4 — `a` and `b` one cell each, the emoji two.
pub fn display_width(s: &str) -> usize {
    s.graphemes(true).map(cluster_cells).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_one_per_char() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn emoji_and_cjk_are_two() {
        assert_eq!(display_width("😀"), 2);
        assert_eq!(display_width("a😀b"), 4);
        assert_eq!(display_width("中文"), 4);
    }

    #[test]
    fn multi_codepoint_emoji_is_one_double_cell() {
        // ZWJ family, regional-indicator flag, skin-tone — each one 2-cell cluster.
        assert_eq!(display_width("👨‍👩‍👧"), 2);
        assert_eq!(display_width("🇿🇦"), 2);
        assert_eq!(display_width("👍🏽"), 2);
    }

    #[test]
    fn combining_marks_add_nothing() {
        // base 'e' + combining acute → one cell, not two.
        assert_eq!(display_width("e\u{0301}"), 1);
    }
}
