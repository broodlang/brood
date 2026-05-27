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
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        LineIndex {
            line_starts,
            len: text.len() as u32,
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
}
