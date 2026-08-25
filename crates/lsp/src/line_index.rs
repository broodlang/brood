//! Maps byte offsets (what the CST records in its [`Span`]s) to LSP
//! [`Position`]s. LSP `Position.character` is a **UTF-16 code-unit** offset by
//! default — not bytes, not Unicode scalar values — so this is the one place
//! that arithmetic lives, built once and correctly rather than rediscovered as
//! off-by-N bugs feature by feature (see `docs/lsp.md` §Positions).
//!
//! We advertise the default UTF-16 encoding in `initialize`; negotiating UTF-8
//! via `positionEncoding` would make this map trivial, but the UTF-16 fallback
//! must exist regardless, so we implement it.
//!
//! [`Span`]: brood::error::Span

use brood::error::Span;
use lsp_types::{Position, Range};

/// Precomputed line-start byte offsets for a document, so byte ↔ `Position`
/// projection is a binary search plus a short UTF-16 count.
pub struct LineIndex {
    /// Byte offset of the start of each line. Always begins with `0`; grows by
    /// one entry per `\n`.
    line_starts: Vec<u32>,
}

/// The largest char boundary of `text` at or before byte `i`, clamping `i` past
/// the end to `text.len()`. `str::floor_char_boundary` is still unstable, and
/// this is the whole safety net under [`LineIndex::position`]: an offset derived
/// by arithmetic on a span can land inside a multibyte character, and slicing
/// there is a panic.
fn floor_char_boundary(text: &str, i: usize) -> usize {
    if i >= text.len() {
        return text.len();
    }
    let mut i = i;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        // The whole span machinery is `u32`-indexed (`error::Span` etc.) — flag
        // a > 4 GiB document in debug. In release, callers downstream that
        // index past the truncated length will return saturated positions
        // rather than panic.
        debug_assert!(
            text.len() <= u32::MAX as usize,
            "LineIndex: document larger than 4 GiB ({} bytes)",
            text.len()
        );
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        LineIndex { line_starts }
    }

    /// The `Position` of byte `offset` within `text` (the same text this index
    /// was built from). **Total**: an out-of-range offset clamps to
    /// end-of-document, and an offset that lands *inside* a multibyte character
    /// snaps back to that character's start.
    ///
    /// The snap is load-bearing, not paranoia. Callers legitimately derive an
    /// offset by arithmetic on a span — `folding.rs` takes `span.end - 1` to
    /// name "the last byte inside this container" — and the CST hands an
    /// **unclosed** delimiter a span that runs to EOF, so `span.end - 1` lands
    /// mid-character for any buffer ending in one (`"(é"` while typing). Slicing
    /// there panicked, which on the stdio transport kills the whole server and
    /// takes the editor's language support with it. Every conversion funnels
    /// through here, so making *this* total fixes the whole class.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let offset = floor_char_boundary(text, offset as usize);
        // The line is the last line-start at or before `offset`.
        let line = match self.line_starts.binary_search(&(offset as u32)) {
            Ok(exact) => exact,
            Err(next) => next - 1, // `next` is never 0: line_starts[0] == 0 <= offset
        };
        // Clamped for the same reason as `offset` — a caller that pairs this
        // index with a *different* text (a bug, but not one worth a panic) must
        // still get a position back.
        let line_start = floor_char_boundary(text, self.line_starts[line] as usize).min(offset);
        // `character` counts UTF-16 code units from the line start to `offset`.
        let character: u32 = text[line_start..offset]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        Position::new(line as u32, character)
    }

    /// The byte offset of a reader [`Pos`] — 1-based line, 1-based **character**
    /// column — within `text`.
    ///
    /// This is the *third* column convention in play and the one that trips
    /// people up: `cst::Span` counts bytes, LSP `Position.character` counts
    /// UTF-16 code units, and [`brood::error::Pos`] (what the reader and the
    /// advisory checker report) counts **characters**. Feeding a `Pos.col`
    /// straight into a `Position.character` is only correct while the line is
    /// all-BMP; one emoji ahead of the column silently shifts the answer by one
    /// per astral character, which is how a checker finding's squiggle — and the
    /// quick-fix edit anchored to it — landed on the wrong text.
    ///
    /// Out-of-range line/column clamp like [`offset`](Self::offset): a line past
    /// EOF yields end-of-document, a column past end-of-line yields the line's
    /// end (never spilling into the next line).
    pub fn offset_of_char_pos(&self, text: &str, pos: brood::error::Pos) -> u32 {
        let line = pos.line.saturating_sub(1) as usize;
        let target_col = pos.col.saturating_sub(1) as usize;
        let Some(&line_start) = self.line_starts.get(line) else {
            return text.len() as u32;
        };
        let mut byte = floor_char_boundary(text, line_start as usize);
        // `col` is the count of characters already stepped over, so the enumerate index
        // is it exactly — the walk advances `byte` by whole characters, never bytes.
        for (col, c) in text[byte..].chars().enumerate() {
            if c == '\n' || col >= target_col {
                break;
            }
            byte += c.len_utf8();
        }
        byte as u32
    }

    /// The byte offset one character past `offset` (clamped to end-of-document).
    /// For a "one character wide" marker range — the diagnostic fallback — which
    /// must step a whole character, not one byte and not one UTF-16 unit.
    pub fn next_char(&self, text: &str, offset: u32) -> u32 {
        let at = floor_char_boundary(text, offset as usize);
        match text[at..].chars().next() {
            Some(c) => (at + c.len_utf8()) as u32,
            None => at as u32,
        }
    }

    /// The LSP `Range` of a byte [`Span`] within `text` — both endpoints projected
    /// via [`position`]. The span→`Range` projection every request needs (goto,
    /// references, rename, symbols, semantic tokens), so the
    /// `Range::new(position(start), position(end))` pair lives in exactly one place.
    ///
    /// [`position`]: Self::position
    pub fn range(&self, text: &str, span: Span) -> Range {
        Range::new(
            self.position(text, span.start),
            self.position(text, span.end),
        )
    }

    /// The byte offset of `pos` within `text` — the inverse of [`position`], for
    /// requests that arrive as a `Position` (hover, completion, goto). `character`
    /// is a UTF-16 code-unit count, so we walk the line's chars summing UTF-16
    /// widths until we reach it. A `line`/`character` past the end clamps to
    /// end-of-line / end-of-document, mirroring `position`'s out-of-range clamp.
    ///
    /// [`position`]: Self::position
    pub fn offset(&self, text: &str, pos: Position) -> u32 {
        let Some(&line_start) = self.line_starts.get(pos.line as usize) else {
            return text.len() as u32; // a line past EOF → end of document
        };
        let mut col_u16 = 0u32;
        // Clamped/floored defensively: see `position`'s note on a mismatched text.
        let mut byte = floor_char_boundary(text, line_start as usize);
        for c in text[byte..].chars() {
            // Stop at the line's end so a `character` past the line doesn't spill
            // into the next one.
            if c == '\n' {
                break;
            }
            // Test the column *after* adding this char's width: if it would step
            // past the target, the target lands within this char, so stop before
            // it. A mid-surrogate `character` (a client can emit one for a non-BMP
            // char like an emoji) thus snaps back to the char's start, not forward
            // to the next char.
            let w = c.len_utf16() as u32;
            if col_u16 + w > pos.character {
                break;
            }
            col_u16 += w;
            byte += c.len_utf8();
        }
        byte as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_offsets_on_a_single_line() {
        let text = "(foo bar)";
        let idx = LineIndex::new(text);
        assert_eq!(idx.position(text, 0), Position::new(0, 0));
        assert_eq!(
            idx.position(text, text.find("bar").unwrap() as u32),
            Position::new(0, 5)
        );
    }

    #[test]
    fn maps_offsets_across_lines() {
        let text = "(a)\n  (b)\n";
        let idx = LineIndex::new(text);
        // start of the second line's `(b)`
        let at = text.find("(b)").unwrap() as u32;
        assert_eq!(idx.position(text, at), Position::new(1, 2));
        // a newline byte projects to the end of its line
        let nl = text.find('\n').unwrap() as u32;
        assert_eq!(idx.position(text, nl), Position::new(0, 3));
    }

    #[test]
    fn counts_columns_in_utf16_code_units() {
        // 'é' is 2 bytes UTF-8 but 1 UTF-16 unit; '😀' is 4 bytes / 2 units.
        let text = "é😀x";
        let idx = LineIndex::new(text);
        let x = text.find('x').unwrap() as u32; // byte 6
                                                // 'é' (1) + '😀' (2) = 3 UTF-16 units before 'x'
        assert_eq!(idx.position(text, x), Position::new(0, 3));
    }

    #[test]
    fn clamps_out_of_range_offsets() {
        let text = "ab";
        let idx = LineIndex::new(text);
        assert_eq!(idx.position(text, 999), Position::new(0, 2));
    }

    #[test]
    fn offset_inverts_position_across_lines_and_multibyte() {
        // `offset` must round-trip with `position` at every char boundary,
        // including past multibyte chars where bytes != UTF-16 columns.
        let text = "(a)\n  (é😀)\n(c)";
        let idx = LineIndex::new(text);
        for (b, _) in text.char_indices() {
            let p = idx.position(text, b as u32);
            assert_eq!(idx.offset(text, p), b as u32, "round-trip at byte {b}");
        }
    }

    #[test]
    fn offset_snaps_a_mid_surrogate_column_back_to_the_char_start() {
        // '😀' is 4 bytes / 2 UTF-16 units. A client may send `character: 1` —
        // inside the surrogate pair. That must snap back to the emoji's start
        // (byte 0), not forward to the next char `b` (byte 4).
        let text = "😀b";
        let idx = LineIndex::new(text);
        assert_eq!(idx.offset(text, Position::new(0, 1)), 0);
        assert_eq!(idx.offset(text, Position::new(0, 2)), 4); // boundary → `b`
    }

    /// Regression: `position` must be total. An offset landing *inside* a
    /// multibyte character used to slice `&text[..offset]` and panic — which on
    /// the stdio transport killed the whole server. Reached for real by
    /// `folding.rs`'s `span.end - 1` over an unclosed `(é`.
    #[test]
    fn position_snaps_an_interior_byte_back_to_the_char_start() {
        let text = "(é"; // bytes: '(' 0, 'é' 1..3
        let idx = LineIndex::new(text);
        assert_eq!(idx.position(text, 2), Position::new(0, 1)); // mid-'é' → 'é' start
        let emoji = "(😀"; // '😀' is 1..5
        let idx = LineIndex::new(emoji);
        for interior in 2..=4 {
            assert_eq!(idx.position(emoji, interior), Position::new(0, 1));
        }
        // Past the end still clamps.
        assert_eq!(idx.position(emoji, 999), Position::new(0, 3));
    }

    /// A reader `Pos` counts CHARACTERS; `Position.character` counts UTF-16
    /// units. `offset_of_char_pos` is the bridge — an astral char ahead of the
    /// column must not shift the result.
    #[test]
    fn char_columns_are_not_utf16_columns() {
        use brood::error::Pos;
        let text = "(def s \"😀😀\") (frobnicate 1)\n";
        let idx = LineIndex::new(text);
        let want = text.find("frobnicate").unwrap() as u32;
        // The reader would report this form at 1-based char column 15
        // (`(def s "😀😀") ` is 14 characters).
        let col = text[..want as usize].chars().count() as u32 + 1;
        assert_eq!(idx.offset_of_char_pos(text, Pos { line: 1, col }), want);
        // Its UTF-16 column is 16 — two more than the character column, which is
        // exactly the drift that misplaced the squiggle.
        assert_eq!(idx.position(text, want).character, col - 1 + 2);
        // Out of range clamps rather than panicking.
        assert_eq!(
            idx.offset_of_char_pos(text, Pos { line: 99, col: 1 }),
            text.len() as u32
        );
        assert_eq!(
            idx.offset_of_char_pos(text, Pos { line: 1, col: 9999 }),
            text.find('\n').unwrap() as u32
        );
    }

    #[test]
    fn next_char_steps_a_whole_character() {
        let text = "😀é";
        let idx = LineIndex::new(text);
        assert_eq!(idx.next_char(text, 0), 4);
        assert_eq!(idx.next_char(text, 4), 6);
        assert_eq!(idx.next_char(text, 6), 6); // EOF is a fixed point
        assert_eq!(idx.next_char(text, 2), 4); // interior byte floors first
    }

    #[test]
    fn offset_clamps_past_end_of_line_and_document() {
        let text = "(a)\n(b)";
        let idx = LineIndex::new(text);
        // A column past the first line's end clamps to the newline, not line 2.
        assert_eq!(idx.offset(text, Position::new(0, 99)), 3);
        // A line past EOF clamps to end-of-document.
        assert_eq!(idx.offset(text, Position::new(99, 0)), text.len() as u32);
    }
}
