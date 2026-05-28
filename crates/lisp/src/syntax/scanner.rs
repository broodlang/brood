//! Shared low-level scanner for the two structural parsers in this layer:
//! the evaluation [`reader`](super::reader) (text → `Value`) and the tooling
//! [`cst`](super::cst) (text → lossless span tree).
//!
//! Where [`atom`](super::atom) shares the *token rules* (delimiter set,
//! classification of an atom-shaped token), this module shares the *character
//! stream* + the operations both parsers use to walk it:
//!
//! - [`Scanner::peek`] / [`Scanner::bump`] / [`Scanner::at_end`]
//! - [`Scanner::skip_trivia`] — whitespace (commas count) + `;` comments
//! - [`Scanner::read_atom`] — consume to the next delimiter, return the slice
//! - [`Scanner::is_dot_separator`] — `.` in dotted-pair position
//! - [`Scanner::scan_string_body`] — walk past a `"…"` body, with optional
//!   escape decoding; both ends agree on where a string ends
//! - [`Scanner::pos_at`] — 1-based `Pos` from a byte offset (for diagnostics)
//!
//! Byte-offset based. Pre-consolidation, the reader carried a `Vec<char>`
//! (4× source memory); the CST already used byte offsets. Sharing the scanner
//! brings the reader onto the CST's representation. ADR-025's "one source of
//! truth for what a token is" extended one layer down to "one source of truth
//! for where chars are".

use crate::error::Pos;
use crate::syntax::atom;

/// A byte-offset cursor into `src`. Cheap: just two fields, no allocation.
pub struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

/// Result of [`Scanner::scan_string_body`] — either the closing quote was
/// found (and `pos` is positioned just past it) or EOF arrived first.
pub enum StringScan {
    Closed,
    Unterminated,
}

impl<'a> Scanner<'a> {
    pub fn new(src: &'a str) -> Self {
        Scanner { src, pos: 0 }
    }

    /// Current byte offset into `src`. Both parsers carry their own notion of
    /// position outside the scanner (line/col for diagnostics, spans for the
    /// CST) so this is the one canonical place to read it.
    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Move the cursor — used by both parsers when they consume a single
    /// known-width delimiter (`(`, `[`, …). Asserted to land on a UTF-8
    /// boundary because both delimiters and all moves happen at ASCII chars.
    #[inline]
    pub fn set_pos(&mut self, p: usize) {
        debug_assert!(self.src.is_char_boundary(p));
        self.pos = p;
    }

    #[inline]
    pub fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    #[inline]
    pub fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    /// The next-but-one char (i.e. the second char from `pos`). Used by
    /// [`Scanner::is_dot_separator`]; nothing else has a 2-char lookahead.
    pub fn peek_after(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next()?;
        it.next()
    }

    #[inline]
    pub fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Skip whitespace (commas count) and `;` line comments — exactly what
    /// both parsers want between forms.
    pub fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() || c == ',' => {
                    self.pos += c.len_utf8();
                }
                Some(';') => {
                    while let Some(c) = self.bump() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// Consume an atom token (`pos` is past the last byte of the token on
    /// return). Returns the slice. Behaviour matches both parsers' previous
    /// inline copies — stops at any [`atom::is_delimiter`] char.
    pub fn read_atom(&mut self) -> &'a str {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if atom::is_delimiter(c) {
                break;
            }
            self.pos += c.len_utf8();
        }
        &self.src[start..self.pos]
    }

    /// Is the `.` at the cursor a lone dotted-pair separator (followed by a
    /// delimiter or end), rather than the start of an atom like `.5` or `.foo`?
    /// Used by the reader; the CST currently treats every `.` as atom-start
    /// (it has no dotted-pair node), so this is reader-only today but lives
    /// here because the predicate is purely lexical.
    pub fn is_dot_separator(&self) -> bool {
        self.peek_after().is_none_or(atom::is_delimiter)
    }

    /// Walk past the body of a `"…"` string. Assumes `pos` is currently just
    /// past the opening quote. If `out` is `Some`, decoded chars (handling
    /// the `\n`/`\t`/`\r`/`\e`/`\0`/`\\`/`\"` escapes + `\X` as literal X)
    /// are appended. If `out` is `None`, the body is just skipped — the CST
    /// only needs the span, so it can avoid the allocation.
    ///
    /// On `Closed`, `pos` is past the close quote. On `Unterminated`, `pos`
    /// is at EOF (the reader treats this as a parse error; the CST records an
    /// `Error` node).
    pub fn scan_string_body(&mut self, mut out: Option<&mut String>) -> StringScan {
        loop {
            match self.bump() {
                None => return StringScan::Unterminated,
                Some('"') => return StringScan::Closed,
                Some('\\') => match self.bump() {
                    None => return StringScan::Unterminated,
                    Some(escaped) => {
                        if let Some(buf) = out.as_deref_mut() {
                            buf.push(match escaped {
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                'e' => '\u{1b}', // ESC — for ANSI terminal control
                                '0' => '\0',
                                '\\' => '\\',
                                '"' => '"',
                                other => other, // `\X` falls through to literal X
                            });
                        }
                    }
                },
                Some(c) => {
                    if let Some(buf) = out.as_deref_mut() {
                        buf.push(c);
                    }
                }
            }
        }
    }

    /// The 1-based `Pos` of byte offset `idx`. Computed by scanning from the
    /// start of `src`; only called on top-level form starts and on parse
    /// errors, so the linear scan is fine.
    pub fn pos_at(&self, idx: usize) -> Pos {
        let mut line = 1u32;
        let mut col = 1u32;
        let upto = idx.min(self.src.len());
        for c in self.src[..upto].chars() {
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        Pos { line, col }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_trivia_eats_whitespace_commas_and_comments() {
        let mut s = Scanner::new("  ,, ; comment\n  x");
        s.skip_trivia();
        assert_eq!(s.peek(), Some('x'));
    }

    #[test]
    fn read_atom_stops_at_delimiter() {
        let mut s = Scanner::new("foo bar)");
        let a = s.read_atom();
        assert_eq!(a, "foo");
        assert_eq!(s.peek(), Some(' '));
    }

    #[test]
    fn scan_string_body_decodes_escapes_when_asked() {
        let mut s = Scanner::new(r#"hi\nthere"more"#);
        let mut out = String::new();
        assert!(matches!(
            s.scan_string_body(Some(&mut out)),
            StringScan::Closed
        ));
        assert_eq!(out, "hi\nthere");
        // `pos` is just past the close quote.
        assert_eq!(&s.src[s.pos..], "more");
    }

    #[test]
    fn scan_string_body_skips_without_allocating_when_out_is_none() {
        // Same as the CST path: just walk past the body, span comes from src.
        let mut s = Scanner::new(r#"any \" content "tail"#);
        assert!(matches!(s.scan_string_body(None), StringScan::Closed));
        assert_eq!(&s.src[s.pos..], "tail");
    }

    #[test]
    fn scan_string_body_reports_unterminated() {
        let mut s = Scanner::new(r#"oops"#);
        assert!(matches!(
            s.scan_string_body(None),
            StringScan::Unterminated
        ));
        assert!(s.at_end());
    }

    #[test]
    fn pos_at_counts_lines_and_columns_through_multibyte() {
        let src = "λα\nβγ";
        let s = Scanner::new(src);
        // The byte index of 'β' is 5 (`λ` is 2 bytes, `α` 2 bytes, `\n` 1).
        let beta = src.find('β').unwrap();
        assert_eq!(s.pos_at(beta), Pos { line: 2, col: 1 });
    }

    #[test]
    fn is_dot_separator_distinguishes_dotted_pair_from_atom() {
        // `.` followed by delimiter is the dotted-pair separator.
        let mut s = Scanner::new(".)");
        assert!(s.is_dot_separator());
        // `.5` is the start of an atom, not a dotted-pair `.`.
        let mut t = Scanner::new(".5");
        assert!(!t.is_dot_separator());
        // Suppress unused-warning if we don't bump (we don't).
        s.bump();
        t.bump();
    }
}
