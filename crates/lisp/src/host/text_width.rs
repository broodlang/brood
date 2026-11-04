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
//!
//! **Glyphless characters (ADR-556).** The invisible format characters — a zero-width
//! space, a word joiner, the bidi controls a "Trojan Source" attack hides in, a BOM in
//! mid-text, a tag character ([`is_glyphless`]) — are zero cells, as a terminal draws
//! them. Laid out [`Glyphless::HexCode`] (the dynamic `*glyphless-display*` bound to
//! `:hex-code`, Emacs's `glyphless-char-display`) each one is shown instead as its hex
//! escape, `<200b>`: it occupies the escape's cells, `expand_tabs` writes the escape in
//! its place, and a cell anywhere in it maps back to its cluster. The class is decided
//! per CHARACTER, never by its neighbours, so a string measured in pieces (a view's
//! chunks) sums to the whole. None is ASCII, so the ASCII fast path never meets one.

use unicode_segmentation::UnicodeSegmentation;

use unicode_width::UnicodeWidthStr;

/// The default tab width: a tab stop every 8 columns (Emacs's `tab-width`).
pub const TAB_WIDTH: usize = 8;

/// The widest tab stop the layout functions accept. A tab expands to up to this many
/// spaces, so the cap is what bounds `expand_tabs`'s output to a small multiple of its
/// input — uncapped, `(string/expand-tabs "\t" 0 100000000000)` asked the allocator for
/// 400 GB and aborted the runtime.
pub const MAX_TAB_WIDTH: usize = 1024;

/// The largest starting column the layout functions accept. Columns are summed as the
/// clusters advance, so the cap keeps that sum far from overflow: from `i64::MAX` a
/// three-cluster string's width wrapped to `i64::MIN`. 2^30, not more: it must fit a
/// 32-bit `usize` (the wasm32 playground) with room left for the clusters' widths.
pub const MAX_START_COLUMN: usize = 1 << 30;

/// How a glyphless character ([`is_glyphless`]) is laid out (ADR-556): `Hidden`, zero
/// cells, as a terminal draws it (the default); `HexCode`, as its visible escape
/// (`<200b>`, [`glyphless_escape`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphless {
    Hidden,
    HexCode,
}

/// Whether `c` is a GLYPHLESS character: an invisible format character (Unicode general
/// category Cf) with no part in shaping a script — so nothing on screen says it is
/// there. They are: U+061C ARABIC LETTER MARK, U+200B ZERO WIDTH SPACE, U+200E / U+200F
/// the LTR / RTL marks, U+202A–U+202E the bidi embeddings and overrides, U+2060 WORD
/// JOINER and U+2061–U+2064 the invisible math operators, U+2066–U+2069 the bidi
/// isolates, U+206A–U+206F the deprecated format characters, U+FEFF (a BOM, or a
/// zero-width no-break space in mid-text), U+FFF9–U+FFFB the interlinear annotation
/// marks, and U+E0001 / U+E0020–U+E007F the tag characters ("ASCII smuggling").
///
/// The Cf characters a script's shaping consumes are left out, because showing them
/// would break ordinary text: ZWNJ / ZWJ (U+200C / U+200D — Persian, the Indic scripts,
/// every emoji ZWJ sequence), the prepended concatenation marks (U+0600–U+0605, U+06DD,
/// U+070F, U+0890–U+0891, U+08E2, U+110BD, U+110CD), U+180E MONGOLIAN VOWEL SEPARATOR,
/// the hieroglyph, shorthand and musical format controls, and U+00AD SOFT HYPHEN, which
/// fonts draw.
pub fn is_glyphless(c: char) -> bool {
    matches!(
        c as u32,
        0x061C
            | 0x200B
            | 0x200E..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// The visible escape a glyphless `c` is laid out as under [`Glyphless::HexCode`]: its
/// code point in lowercase hex, at least four digits, between angle brackets — `<200b>`,
/// `<e0041>`. ASCII, so its cells are its length.
pub fn glyphless_escape(c: char) -> String {
    format!("<{:04x}>", c as u32)
}

/// The cells [`glyphless_escape`] of `c` occupies, without building it.
fn glyphless_escape_cells(c: char) -> usize {
    let digits = (32 - (c as u32).leading_zeros()).div_ceil(4) as usize;
    2 + digits.max(4)
}

/// Whether `s` holds a glyphless character — a cheap reject for the common text, every
/// ASCII character included.
fn has_glyphless(s: &str) -> bool {
    s.chars().any(|c| c as u32 >= 0x061C && is_glyphless(c))
}

/// [`cluster_cells_at`] laid out `glyphless`: under [`Glyphless::HexCode`] a cluster
/// holding glyphless characters is the cells of the rest of it plus each one's escape — a
/// lone zero-width space six cells, a tag-sequence flag its two plus the tags' escapes.
fn layout_cells(cluster: &str, col: usize, tab_width: usize, glyphless: Glyphless) -> usize {
    if glyphless == Glyphless::HexCode && has_glyphless(cluster) {
        let rest: String = cluster.chars().filter(|&c| !is_glyphless(c)).collect();
        let escapes: usize = cluster
            .chars()
            .filter(|&c| is_glyphless(c))
            .map(glyphless_escape_cells)
            .sum();
        cluster_cells(&rest) + escapes
    } else {
        cluster_cells_at(cluster, col, tab_width)
    }
}

/// The codepoint index of every glyphless character in `s` ([`is_glyphless`]), in
/// order — where a view lays an escape face over the `<200b>` it shows.
pub fn glyphless_indices(s: &str) -> Vec<usize> {
    s.chars()
        .enumerate()
        .filter(|&(_, c)| is_glyphless(c))
        .map(|(index, _)| index)
        .collect()
}

/// The grapheme clusters of `s`, exactly as `s.graphemes(true)` yields them, without
/// running the segmenter over ASCII.
///
/// The layout functions below walk a whole string per call, and an editor asks them of
/// whole lines: the column of point, where a row breaks, how far a line scrolls. On a
/// minified file or a log the line is a megabyte, and the Unicode segmenter cost ~20 ms
/// per megabyte per call — several calls a keystroke. Nearly all of such a line is
/// ASCII, where the cluster rules are decided by the next byte alone:
///
/// - CR LF is one cluster (GB3); every other ASCII control breaks on both sides (GB4/5).
/// - Two ASCII characters always break between them: nothing in ASCII extends,
///   prepends or joins (GB999).
/// - An ASCII character that is not a control MAY be extended by what follows it (a
///   combining mark, a ZWJ — GB9/9a), so when the next byte is not ASCII the segmenter
///   decides that one cluster.
///
/// Anything starting with a non-ASCII byte goes to the segmenter too, from a cluster
/// boundary, which is where every cluster starts.
pub fn clusters(s: &str) -> Clusters<'_> {
    Clusters { s, at: 0 }
}

/// The iterator [`clusters`] answers.
pub struct Clusters<'a> {
    s: &'a str,
    at: usize,
}

impl<'a> Iterator for Clusters<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let start = self.at;
        if start >= self.s.len() {
            return None;
        }
        let end = cluster_end(self.s, start);
        self.at = end;
        Some(&self.s[start..end])
    }
}

/// The byte offset where the cluster starting at byte `start` (a cluster boundary, before
/// the end of `s`) ends — [`clusters`]'s rule.
fn cluster_end(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let byte = bytes[start];
    if byte.is_ascii() {
        match bytes.get(start + 1).copied() {
            None => start + 1,
            Some(b'\n') if byte == b'\r' => start + 2,
            Some(after) if after.is_ascii() || byte.is_ascii_control() => start + 1,
            Some(_) => start + first_cluster_len(&s[start..]),
        }
    } else {
        start + first_cluster_len(&s[start..])
    }
}

/// Whether the byte at `at` is a printable ASCII character that is a cluster of its own
/// (the byte after it ASCII too, or none) — one cell, one char, no segmenter: the step
/// the layout loops below take for nearly every character of a source file or a log.
#[inline]
fn lone_printable(bytes: &[u8], at: usize) -> bool {
    (b' '..=b'~').contains(&bytes[at]) && bytes.get(at + 1).is_none_or(u8::is_ascii)
}

/// The byte length of the first grapheme cluster of a non-empty `s`, by the segmenter.
fn first_cluster_len(s: &str) -> usize {
    s.graphemes(true).next().map_or(s.len(), str::len)
}

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
    // a printable ASCII character is one cell — the common cluster, answered without the
    // width tables
    if let [byte] = cluster.as_bytes()
        && (b' '..=b'~').contains(byte)
    {
        return 1;
    }
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
/// every `tab_width`, glyphless characters laid out `glyphless`: the sum of its grapheme
/// clusters' widths, a tab reaching its next stop. `(string/display-width "a😀b")` is 4 —
/// `a` and `b` one cell each, the emoji two; `"a\tb"` is 9.
pub fn display_width_from(
    s: &str,
    start_col: usize,
    tab_width: usize,
    glyphless: Glyphless,
) -> usize {
    let bytes = s.as_bytes();
    let mut col = start_col;
    let mut at = 0;
    while at < bytes.len() {
        if lone_printable(bytes, at) {
            col += 1;
            at += 1;
        } else if bytes[at] == b'\t' {
            // a tab is a control: a cluster of its own whatever follows (GB4)
            col += tab_span(col, tab_width);
            at += 1;
        } else {
            let end = cluster_end(s, at);
            col += layout_cells(&s[at..end], col, tab_width, glyphless);
            at = end;
        }
    }
    col - start_col
}

/// `display_width_from` at column 0 with the default tab width, glyphless characters
/// hidden.
pub fn display_width(s: &str) -> usize {
    display_width_from(s, 0, TAB_WIDTH, Glyphless::Hidden)
}

/// The inverse of `display_width`: the codepoint index of the grapheme cluster that
/// occupies display cell `cell` (counted from `start_col`, with stops every
/// `tab_width`, glyphless characters laid out `glyphless`), or the codepoint count of `s`
/// when `cell` lies at or past its end. A cell inside a 2-cell glyph maps to that glyph's
/// start, a cell inside a tab's span to the tab, a cell inside a glyphless escape to its
/// cluster, and a zero-width cluster (a combining mark) belongs to the cell of the base
/// before it — so a click anywhere on a wide glyph puts point before it, never inside
/// it. The column half of the editor's mouse mapping: `display_width` places the caret
/// (chars -> cells), this maps a click back (cells -> chars); one module, so they
/// cannot disagree.
pub fn index_at_cell_from(
    s: &str,
    cell: usize,
    start_col: usize,
    tab_width: usize,
    glyphless: Glyphless,
) -> usize {
    let bytes = s.as_bytes();
    let mut col = start_col;
    let cell = start_col + cell;
    let mut chars = 0;
    let mut at = 0;
    while at < bytes.len() {
        let (width, end) = if lone_printable(bytes, at) {
            (1, at + 1)
        } else if bytes[at] == b'\t' {
            (tab_span(col, tab_width), at + 1)
        } else {
            let end = cluster_end(s, at);
            (layout_cells(&s[at..end], col, tab_width, glyphless), end)
        };
        if width > 0 && cell < col + width {
            return chars;
        }
        col += width;
        chars += if end == at + 1 {
            1
        } else {
            s[at..end].chars().count()
        };
        at = end;
    }
    chars
}

/// `index_at_cell_from` at column 0 with the default tab width, glyphless characters
/// hidden.
pub fn index_at_cell(s: &str, cell: usize) -> usize {
    index_at_cell_from(s, cell, 0, TAB_WIDTH, Glyphless::Hidden)
}

/// `s` with every tab replaced by the spaces that carry it to its next stop, laid out
/// from column `start_col` with stops every `tab_width` — and, laid out
/// [`Glyphless::HexCode`], every glyphless character replaced by its escape (after the
/// rest of its cluster) — so the result's `display_width` is `s`'s, cluster for
/// cluster, with no tab or invisible character left for a frontend to misplace.
/// Borrows `s` unchanged when there is nothing to replace (the common line).
pub fn expand_tabs(
    s: &str,
    start_col: usize,
    tab_width: usize,
    glyphless: Glyphless,
) -> std::borrow::Cow<'_, str> {
    let shows_glyphless = glyphless == Glyphless::HexCode && has_glyphless(s);
    if !s.contains('\t') && !shows_glyphless {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + tab_width.max(1) * 4);
    let mut col = start_col;
    for cluster in clusters(s) {
        let width = layout_cells(cluster, col, tab_width, glyphless);
        if cluster == "\t" {
            out.extend(std::iter::repeat_n(' ', width));
        } else if shows_glyphless && has_glyphless(cluster) {
            out.extend(cluster.chars().filter(|&c| !is_glyphless(c)));
            for c in cluster.chars().filter(|&c| is_glyphless(c)) {
                out.push_str(&glyphless_escape(c));
            }
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

    const HIDDEN: Glyphless = Glyphless::Hidden;
    const HEX: Glyphless = Glyphless::HexCode;

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
        assert_eq!(display_width_from("\tx", 3, TAB_WIDTH, HIDDEN), 6);
        assert_eq!(display_width_from("\tx", 0, 4, HIDDEN), 5);
    }

    #[test]
    fn expand_tabs_materialises_the_same_width() {
        assert_eq!(expand_tabs("a\tb", 0, 8, HIDDEN), "a       b");
        assert_eq!(expand_tabs("\tx", 3, 8, HIDDEN), "     x");
        assert_eq!(expand_tabs("\tx", 0, 4, HIDDEN), "    x");
        assert_eq!(expand_tabs("no tabs", 0, 8, HIDDEN), "no tabs");
        assert!(matches!(
            expand_tabs("no tabs", 0, 8, HIDDEN),
            std::borrow::Cow::Borrowed(_)
        ));
        for (s, col) in [("a\tb\t\tc", 0), ("\t😀\tz", 5), ("x\ty", 7)] {
            assert_eq!(
                display_width_from(&expand_tabs(s, col, 8, HIDDEN), col, 8, HIDDEN),
                display_width_from(s, col, 8, HIDDEN),
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
        assert_eq!(index_at_cell_from("\tx", 4, 3, TAB_WIDTH, HIDDEN), 0);
        assert_eq!(index_at_cell_from("\tx", 5, 3, TAB_WIDTH, HIDDEN), 1);
    }

    /// The fast cluster walk splits exactly where `graphemes(true)` does — on ASCII, on
    /// every place where ASCII meets something that may extend it, and on text that is
    /// not ASCII at all.
    #[test]
    fn clusters_split_where_graphemes_do() {
        let cases = [
            "",
            "plain ascii, with punctuation; and digits 0123",
            "a\tb\t\tc",
            "line\r\nnext\rlone\ncr",
            "e\u{0301}x",
            "ae\u{0301}\u{0302}b",
            "x\u{200d}y",
            "a😀b👨‍👩‍👧c🇿🇦🇿🇦🇿d👍🏽",
            "中文 mixed 日本",
            "\u{7}bell\u{7f}del\u{0}nul",
            "\t\u{0301}after a tab",
            "\r\u{0301}",
            "ascii then \u{0915}\u{094d}\u{0937} devanagari",
            "\u{1100}\u{1161}\u{11a8} hangul jamo",
        ];
        for s in cases {
            let fast: Vec<&str> = clusters(s).collect();
            let slow: Vec<&str> = s.graphemes(true).collect();
            assert_eq!(fast, slow, "{s:?}");
        }
    }

    #[test]
    fn mixed_text_measures_as_the_grapheme_walk_does() {
        // the fast path must not change a single answer: widths and inverses over text
        // that switches between ASCII and not, tabs included
        let s = "ab\tc😀d e\u{0301}f\t中g";
        let mut slow_width = 0;
        for cluster in s.graphemes(true) {
            slow_width += cluster_cells_at(cluster, slow_width, TAB_WIDTH);
        }
        assert_eq!(display_width(s), slow_width);
        for cell in 0..=slow_width + 2 {
            let mut col = 0;
            let mut chars = 0;
            let mut found = None;
            for cluster in s.graphemes(true) {
                let width = cluster_cells_at(cluster, col, TAB_WIDTH);
                if found.is_none() && width > 0 && cell < col + width {
                    found = Some(chars);
                }
                col += width;
                chars += cluster.chars().count();
            }
            assert_eq!(
                index_at_cell(s, cell),
                found.unwrap_or(chars),
                "cell {cell}"
            );
        }
    }

    #[test]
    fn the_glyphless_class_is_the_invisible_format_characters() {
        for c in [
            '\u{200B}',
            '\u{200E}',
            '\u{200F}',
            '\u{202A}',
            '\u{202E}',
            '\u{2060}',
            '\u{2066}',
            '\u{2069}',
            '\u{FEFF}',
            '\u{061C}',
            '\u{E0001}',
            '\u{E0041}',
        ] {
            assert!(is_glyphless(c), "{:04x}", c as u32);
        }
        // the joiners and the marks a script's shaping consumes stay hidden; a soft
        // hyphen is drawn; ordinary text is not glyphless
        for c in [
            '\u{200C}', '\u{200D}', '\u{00AD}', '\u{0600}', '\u{180E}', 'a', ' ', '\t',
        ] {
            assert!(!is_glyphless(c), "{:04x}", c as u32);
        }
    }

    #[test]
    fn hidden_glyphless_characters_are_zero_cells() {
        // a terminal's rule, and the default
        assert_eq!(display_width("a\u{200B}b"), 2);
        assert_eq!(display_width("\u{202E}"), 0);
    }

    #[test]
    fn hex_code_shows_each_glyphless_character_as_its_escape() {
        assert_eq!(display_width_from("a\u{200B}b", 0, 8, HEX), 8);
        assert_eq!(expand_tabs("a\u{200B}b", 0, 8, HEX), "a<200b>b");
        assert_eq!(expand_tabs("\u{202E}x\u{2066}", 0, 8, HEX), "<202e>x<2066>");
        // a five-digit code point takes five digits
        assert_eq!(expand_tabs("\u{E0041}", 0, 8, HEX), "<e0041>");
        assert_eq!(display_width_from("\u{E0041}", 0, 8, HEX), 7);
        // tabs and escapes in one string keep the line's stops
        assert_eq!(expand_tabs("\u{200B}\tx", 0, 8, HEX), "<200b>  x");
        // a string with nothing to show is borrowed
        assert!(matches!(
            expand_tabs("plain", 0, 8, HEX),
            std::borrow::Cow::Borrowed(_)
        ));
        // the joiners are not shown: an emoji ZWJ sequence is still one 2-cell glyph
        assert_eq!(display_width_from("👨‍👩‍👧", 0, 8, HEX), 2);
    }

    #[test]
    fn a_cell_in_an_escape_maps_back_to_its_character() {
        // a=[0,1) <200b>=[1,7) b=[7,8)
        assert_eq!(index_at_cell_from("a\u{200B}b", 0, 0, 8, HEX), 0);
        for cell in 1..7 {
            assert_eq!(index_at_cell_from("a\u{200B}b", cell, 0, 8, HEX), 1);
        }
        assert_eq!(index_at_cell_from("a\u{200B}b", 7, 0, 8, HEX), 2);
        assert_eq!(index_at_cell_from("a\u{200B}b", 8, 0, 8, HEX), 3);
    }

    #[test]
    fn pieces_of_a_string_sum_to_the_whole() {
        // per character, never by context: a view measuring chunks agrees with the line
        let whole = "x\u{202E}\u{2066}yz\u{FEFF}";
        let chars: Vec<char> = whole.chars().collect();
        for split in 0..=chars.len() {
            let head: String = chars[..split].iter().collect();
            let tail: String = chars[split..].iter().collect();
            let head_cells = display_width_from(&head, 0, 8, HEX);
            assert_eq!(
                head_cells + display_width_from(&tail, head_cells, 8, HEX),
                display_width_from(whole, 0, 8, HEX),
                "split at {split}"
            );
        }
        let expanded = expand_tabs(whole, 0, 8, HEX);
        assert_eq!(
            display_width_from(&expanded, 0, 8, HEX),
            display_width_from(whole, 0, 8, HEX)
        );
    }

    #[test]
    fn glyphless_indices_are_codepoint_offsets() {
        assert_eq!(glyphless_indices("a\u{200B}😀\u{202E}"), vec![1, 3]);
        assert!(glyphless_indices("plain").is_empty());
    }
}
