//! Display-cell width of text — how many terminal/grid columns a string occupies.
//!
//! Shared by the `string/display-width` builtin (so Brood code — the editor's column /
//! cursor math — can ask) and the GUI renderer (`gui.rs`, which advances the cell
//! grid one *cluster* at a time, not one codepoint). One definition, so the two can
//! never disagree about where a wide glyph ends.
//!
//! The rule: segment into **grapheme clusters** (a ZWJ emoji, a flag, a base +
//! combining marks, a skin-tone sequence are each *one* cluster), then each cluster
//! is 0 cells (pure combining / zero-width), 1 cell (normal), or 2 cells (wide — CJK
//! and emoji). We clamp `unicode-width`'s per-cluster sum to {0,1,2}: a multi-codepoint
//! emoji sums to more than 2 by codepoint, but occupies one double-width cell.
//!
//! **Tabs.** A `\t` is the one cluster whose width depends on *where it is*: it
//! advances to the next tab stop — the next multiple of the tab width (8, Emacs's
//! `tab-width` default) counted from the start of the string. So the three functions
//! that lay text out by cell all take the same running column: `display_width`
//! (chars → cells), `index_at_cell` (cells → chars, its inverse) and `expand_tabs`
//! (the string with each tab replaced by the spaces that reach its stop — what a
//! frontend is handed, since a raw tab in a render op has no column to measure from).
//! A string laid out from a column other than 0 (a chunk of a longer line) passes
//! that column as `start_col`, and the stops stay the line's. Without this a tab was
//! a zero-width cluster: `a\tb` painted as `ab` and every tab-indented line flush left.

use unicode_segmentation::UnicodeSegmentation;

use unicode_width::UnicodeWidthStr;

/// The default tab width: a tab stop every 8 columns (Emacs's `tab-width`).
pub const TAB_WIDTH: usize = 8;

/// The cells a tab at column `col` spans to reach the next stop of `tab_width`.
fn tab_span(col: usize, tab_width: usize) -> usize {
    let tab_width = tab_width.max(1);
    tab_width - col % tab_width
}

/// The display width, in cells, of a single grapheme cluster: 0 (zero-width /
/// combining), 1 (normal), or 2 (wide — CJK / emoji). Clamps the codepoint-sum width
/// so a multi-codepoint emoji cluster is one 2-cell glyph, not its component count.
/// A tab is 0 here — its width is a function of its column, which only the
/// column-carrying functions below know (`cluster_cells_at`).
pub fn cluster_cells(cluster: &str) -> usize {
    match cluster.width() {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

/// `cluster_cells` for a cluster that sits at column `col`: the same, except a tab
/// spans to its next stop.
pub fn cluster_cells_at(cluster: &str, col: usize, tab_width: usize) -> usize {
    if cluster == "\t" {
        tab_span(col, tab_width)
    } else {
        cluster_cells(cluster)
    }
}

/// The display width, in cells, of `s` laid out from column `start_col` with stops
/// every `tab_width`: the sum of its grapheme clusters' widths, a tab reaching its
/// next stop. `(string/display-width "a😀b")` is 4 — `a` and `b` one cell each, the
/// emoji two; `"a\tb"` is 9.
pub fn display_width_from(s: &str, start_col: usize, tab_width: usize) -> usize {
    let mut col = start_col;
    for cluster in s.graphemes(true) {
        col += cluster_cells_at(cluster, col, tab_width);
    }
    col - start_col
}

/// `display_width_from` at column 0 with the default tab width.
pub fn display_width(s: &str) -> usize {
    display_width_from(s, 0, TAB_WIDTH)
}

/// The inverse of `display_width`: the codepoint index of the grapheme cluster that
/// occupies display cell `cell` (counted from `start_col`, with stops every
/// `tab_width`), or the codepoint count of `s` when `cell` lies at or past its end. A
/// cell inside a 2-cell glyph maps to that glyph's start, a cell inside a tab's span to
/// the tab, and a zero-width cluster (a combining mark) belongs to the cell of the base
/// before it — so a click anywhere on a wide glyph puts point before it, never inside
/// it. The column half of the editor's mouse mapping: `display_width` places the caret
/// (chars -> cells), this maps a click back (cells -> chars); one module, so they
/// cannot disagree.
pub fn index_at_cell_from(s: &str, cell: usize, start_col: usize, tab_width: usize) -> usize {
    let mut col = start_col;
    let cell = start_col + cell;
    let mut chars = 0;
    for cluster in s.graphemes(true) {
        let width = cluster_cells_at(cluster, col, tab_width);
        if width > 0 && cell < col + width {
            return chars;
        }
        col += width;
        chars += cluster.chars().count();
    }
    chars
}

/// `index_at_cell_from` at column 0 with the default tab width.
pub fn index_at_cell(s: &str, cell: usize) -> usize {
    index_at_cell_from(s, cell, 0, TAB_WIDTH)
}

/// `s` with every tab replaced by the spaces that carry it to its next stop, laid out
/// from column `start_col` with stops every `tab_width` — so the result's
/// `display_width` is `s`'s, cluster for cluster, with no tab left for a frontend to
/// misplace. Borrows `s` unchanged when it holds no tab (the common line).
pub fn expand_tabs(s: &str, start_col: usize, tab_width: usize) -> std::borrow::Cow<'_, str> {
    if !s.contains('\t') {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + tab_width.max(1) * 4);
    let mut col = start_col;
    for cluster in s.graphemes(true) {
        let width = cluster_cells_at(cluster, col, tab_width);
        if cluster == "\t" {
            out.extend(std::iter::repeat_n(' ', width));
        } else {
            out.push_str(cluster);
        }
        col += width;
    }
    std::borrow::Cow::Owned(out)
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

    #[test]
    fn index_at_cell_inverts_display_width() {
        // cells: a=[0,1) 😀=[1,3) b=[3,4); the emoji is ONE codepoint.
        assert_eq!(index_at_cell("a😀b", 0), 0);
        assert_eq!(index_at_cell("a😀b", 1), 1);
        assert_eq!(index_at_cell("a😀b", 2), 1); // inside the wide glyph → its start
        assert_eq!(index_at_cell("a😀b", 3), 2);
        assert_eq!(index_at_cell("a😀b", 4), 3); // at the end → the length
        assert_eq!(index_at_cell("a😀b", 40), 3);
        assert_eq!(index_at_cell("", 0), 0);
        assert_eq!(index_at_cell("hello", 3), 3);
    }

    #[test]
    fn index_at_cell_counts_codepoints_through_clusters() {
        // A ZWJ family is several codepoints in one 2-cell glyph: the index after it
        // is its codepoint count, and a combining mark rides with its base.
        let family = "👨‍👩‍👧";
        assert_eq!(
            index_at_cell(&format!("{family}x"), 2),
            family.chars().count()
        );
        assert_eq!(index_at_cell("e\u{0301}x", 1), 2);
        assert_eq!(index_at_cell("e\u{0301}x", 0), 0);
    }

    #[test]
    fn a_tab_reaches_the_next_stop() {
        // From column 0 a tab spans 8; after one char it spans 7; a tab AT a stop
        // spans a whole width (never 0).
        assert_eq!(display_width("\t"), 8);
        assert_eq!(display_width("a\tb"), 9);
        assert_eq!(display_width("abcdefgh\tx"), 17);
        assert_eq!(display_width("\t\t"), 16);
        // the stops are the LINE's: a chunk starting at column 3 tabs to 8, not 11
        assert_eq!(display_width_from("\tx", 3, TAB_WIDTH), 6);
        assert_eq!(display_width_from("\tx", 0, 4), 5);
    }

    #[test]
    fn expand_tabs_materialises_the_same_width() {
        assert_eq!(expand_tabs("a\tb", 0, 8), "a       b");
        assert_eq!(expand_tabs("\tx", 3, 8), "     x");
        assert_eq!(expand_tabs("\tx", 0, 4), "    x");
        assert_eq!(expand_tabs("no tabs", 0, 8), "no tabs");
        assert!(matches!(
            expand_tabs("no tabs", 0, 8),
            std::borrow::Cow::Borrowed(_)
        ));
        for (s, col) in [("a\tb\t\tc", 0), ("\t😀\tz", 5), ("x\ty", 7)] {
            assert_eq!(
                display_width_from(&expand_tabs(s, col, 8), col, 8),
                display_width_from(s, col, 8),
                "{s:?} from {col}"
            );
        }
    }

    #[test]
    fn index_at_cell_inside_a_tab_is_the_tab() {
        // a=[0,1) \t=[1,8) b=[8,9)
        assert_eq!(index_at_cell("a\tb", 0), 0);
        assert_eq!(index_at_cell("a\tb", 1), 1);
        assert_eq!(index_at_cell("a\tb", 5), 1);
        assert_eq!(index_at_cell("a\tb", 7), 1);
        assert_eq!(index_at_cell("a\tb", 8), 2);
        assert_eq!(index_at_cell("a\tb", 9), 3);
        // from a mid-line column the tab's span follows the line's stops
        assert_eq!(index_at_cell_from("\tx", 4, 3, TAB_WIDTH), 0);
        assert_eq!(index_at_cell_from("\tx", 5, 3, TAB_WIDTH), 1);
    }
}
