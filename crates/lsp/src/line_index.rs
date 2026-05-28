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

use lsp_types::Position;

/// Precomputed line-start byte offsets for a document, so byte ↔ `Position`
/// projection is a binary search plus a short UTF-16 count.
pub struct LineIndex {
    /// Byte offset of the start of each line. Always begins with `0`; grows by
    /// one entry per `\n`.
    line_starts: Vec<u32>,
    /// Total length of the source in bytes — the clamp for out-of-range offsets.
    len: u32,
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
        LineIndex {
            line_starts,
            len: text.len().min(u32::MAX as usize) as u32,
        }
    }

    /// The `Position` of byte `offset` within `text` (the same text this index
    /// was built from). Out-of-range offsets clamp to end-of-document.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let offset = offset.min(self.len);
        // The line is the last line-start at or before `offset`.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(next) => next - 1, // `next` is never 0: line_starts[0] == 0 <= offset
        };
        let line_start = self.line_starts[line] as usize;
        // `character` counts UTF-16 code units from the line start to `offset`.
        let character: u32 = text[line_start..offset as usize]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        Position::new(line as u32, character)
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
            return self.len; // a line past EOF → end of document
        };
        let mut col_u16 = 0u32;
        let mut byte = line_start as usize;
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
