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

/// A byte-offset cursor into `src` + a one-shot line-start table for fast
/// `pos_at`. Pre-table, every `pos_at` walked the whole prefix of `src` from
/// byte 0 — the reader called it once per top-level form, so a file with
/// `N` top-level forms paid `O(N × file_size)` just locating line numbers.
/// Building a sorted `Vec<u32>` of newline-following byte offsets once at
/// construction lets `pos_at` do an `O(log N)` bsearch for the line, then a
/// short within-line char walk for the column.
pub struct Scanner<'a> {
    src: &'a str,
    pos: usize,
    /// Byte offsets of every line *start* in `src`. `line_starts[0] == 0`;
    /// each subsequent entry is the byte just past a line break (`\n` — which
    /// also covers CRLF — a lone `\r`, or U+2028/U+2029). So the line
    /// containing byte `b` is the largest `i` with `line_starts[i] <= b`.
    /// ~4 bytes per source line — 5–6 KB for the prelude, negligible.
    line_starts: Vec<u32>,
}

/// Result of [`Scanner::scan_string_body`] — the closing quote was found (and
/// `pos` is positioned just past it), EOF arrived first, or the body held a
/// malformed `\xHH` / `\u{H..H}` escape.
pub enum StringScan {
    Closed,
    Unterminated,
    /// A malformed hex escape; `at` is the byte offset of its backslash. The
    /// body was still scanned through its closing quote (so the tolerant CST
    /// keeps a correct span); a string that is *also* unterminated reports
    /// `Unterminated` instead — the REPL's continuation prompt keys off it.
    BadEscape {
        at: usize,
    },
}

impl<'a> Scanner<'a> {
    pub fn new(src: &'a str) -> Self {
        // Build the line-start table in one byte-walk. `\n` covers Unix and
        // (via its second byte) CRLF; a *lone* `\r` (classic-Mac, or a stray
        // CR mid-file) and the Unicode line/paragraph separators U+2028/U+2029
        // also break a line — otherwise every diagnostic after one reports a
        // wrong line:col (kernel audit). Sized for the common `\n`-only case;
        // the rare extra breaks just grow the Vec.
        let bytes = src.as_bytes();
        let nl_count = bytes.iter().filter(|&&b| b == b'\n').count();
        let mut line_starts = Vec::with_capacity(nl_count + 1);
        line_starts.push(0);
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => line_starts.push((i + 1) as u32),
                // CRLF's break is recorded by its `\n`; only a lone CR breaks here.
                b'\r' if bytes.get(i + 1) != Some(&b'\n') => line_starts.push((i + 1) as u32),
                // U+2028 LINE SEPARATOR (E2 80 A8) / U+2029 PARAGRAPH
                // SEPARATOR (E2 80 A9) in UTF-8.
                0xE2 if bytes.get(i + 1) == Some(&0x80)
                    && matches!(bytes.get(i + 2), Some(&0xA8) | Some(&0xA9)) =>
                {
                    line_starts.push((i + 3) as u32);
                    i += 3;
                    continue;
                }
                _ => {}
            }
            i += 1;
        }
        Scanner {
            src,
            pos: 0,
            line_starts,
        }
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
        // ASCII fast-path: most source bytes are ASCII (every delimiter,
        // every whitespace, every keyword, every prelude line), so save the
        // UTF-8 decode in the common case. A naive `src[pos..].chars().next()`
        // walks 1–4 bytes plus a branch even for `< 0x80` — measurable in a
        // parser-heavy bench (`parse_prelude` lost ~1.7× per byte when we
        // moved from `Vec<char>` to byte offsets without this branch).
        let b = *self.src.as_bytes().get(self.pos)?;
        if b < 0x80 {
            Some(b as char)
        } else {
            self.src[self.pos..].chars().next()
        }
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

    /// True if the unconsumed input begins with `prefix`. A cheap multi-char
    /// lookahead — used by the reader/CST to tell a `#b"…"` bytes literal from a
    /// `#`-prefixed symbol (`#`, like any non-delimiter, is an ordinary atom char).
    #[inline]
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.src[self.pos..].starts_with(prefix)
    }

    /// Skip whitespace (commas count) and `;` line comments — exactly what
    /// both parsers want between forms.
    pub fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if atom::is_trivia_ws(c) => {
                    self.pos += c.len_utf8();
                }
                Some(';') => self.skip_line_comment(),
                _ => break,
            }
        }
    }

    /// Consume a `;` line comment through its terminating newline (or EOF).
    /// Assumes `pos` is on the `;`. Shared by [`Scanner::skip_trivia`] and the
    /// CST's depth-cap balanced-skip; the CST's *trivia node* builder keeps its
    /// own copy because it must record the comment's span as a node.
    pub fn skip_line_comment(&mut self) {
        while let Some(c) = self.bump() {
            if c == '\n' {
                break;
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
    /// the `\n`/`\t`/`\r`/`\e`/`\0`/`\\`/`\"` escapes, the `\xHH` two-hex
    /// byte escape, the `\u{H..H}` Unicode-codepoint escape, and `\X` as
    /// literal X for anything else) are appended. If `out` is `None`, the
    /// body is just skipped — the CST only needs the span, so it can avoid
    /// the allocation.
    ///
    /// Malformed `\x` / `\u{}` (wrong number of hex digits, missing brace,
    /// non-hex char, codepoint > 0x10FFFF) is a **hard error** —
    /// [`StringScan::BadEscape`], carrying the backslash's offset. Silently
    /// passing the chars through as literals (the old rule) was a
    /// wrong-output footgun (kernel audit): `"\xZZ"` quietly became `"xZZ"`.
    /// The body is still scanned through its closing quote so the tolerant
    /// CST keeps the right span; an unterminated string wins over a bad
    /// escape (the REPL continuation prompt keys off `Unterminated`). The
    /// catch-all `\X` → literal X rule for *other* chars is unchanged.
    ///
    /// On `Closed`, `pos` is past the close quote. On `Unterminated`, `pos`
    /// is at EOF (the reader treats this as a parse error; the CST records an
    /// `Error` node).
    pub fn scan_string_body(&mut self, mut out: Option<&mut String>) -> StringScan {
        let mut bad: Option<usize> = None;
        loop {
            let ch_start = self.pos();
            match self.bump() {
                None => return StringScan::Unterminated,
                Some('"') => {
                    return match bad {
                        Some(at) => StringScan::BadEscape { at },
                        None => StringScan::Closed,
                    }
                }
                Some('\\') => match self.bump() {
                    None => return StringScan::Unterminated,
                    Some(escaped) => match escaped {
                        // `\xHH` — a two-hex-digit byte (must be ASCII so the
                        // result is a single valid char). Anything else → the
                        // first malformed escape is reported via `BadEscape`.
                        'x' => {
                            if let Some(ch) = self.try_hex_escape_x() {
                                if let Some(buf) = out.as_deref_mut() {
                                    buf.push(ch);
                                }
                            } else {
                                bad.get_or_insert(ch_start);
                            }
                        }
                        // `\u{H..H}` — a 1-to-6-hex-digit Unicode codepoint in
                        // braces. Up to U+10FFFF; surrogates aren't valid scalar
                        // values. Anything malformed → `BadEscape`.
                        'u' => {
                            if let Some(ch) = self.try_hex_escape_u_brace() {
                                if let Some(buf) = out.as_deref_mut() {
                                    buf.push(ch);
                                }
                            } else {
                                bad.get_or_insert(ch_start);
                            }
                        }
                        other => {
                            if let Some(buf) = out.as_deref_mut() {
                                buf.push(match other {
                                    'n' => '\n',
                                    't' => '\t',
                                    'r' => '\r',
                                    'e' => '\u{1b}', // ESC — for ANSI terminal control
                                    '0' => '\0',
                                    '\\' => '\\',
                                    '"' => '"',
                                    c => c, // `\X` falls through to literal X
                                });
                            }
                        }
                    },
                },
                Some(c) => {
                    if let Some(buf) = out.as_deref_mut() {
                        buf.push(c);
                    }
                }
            }
        }
    }

    /// Try to consume exactly two hex digits and return the resulting char,
    /// or `None` (rewinding so we haven't consumed *any* of them) if the next
    /// two chars aren't both hex. The rewind matters so the outer loop can
    /// fall back to "literal x" and still see the original chars.
    fn try_hex_escape_x(&mut self) -> Option<char> {
        let saved = self.pos();
        let h1 = self.bump().and_then(|c| c.to_digit(16));
        let h2 = self.bump().and_then(|c| c.to_digit(16));
        match (h1, h2) {
            (Some(h1), Some(h2)) => char::from_u32(h1 * 16 + h2),
            _ => {
                self.pos = saved;
                None
            }
        }
    }

    /// Try to consume `{H..H}` after `\u` and return the resulting char, or
    /// `None` (rewinding) if anything goes wrong. 1–6 hex digits, surrogate
    /// halves rejected (not valid Unicode scalar values).
    fn try_hex_escape_u_brace(&mut self) -> Option<char> {
        let saved = self.pos();
        if self.bump() != Some('{') {
            self.pos = saved;
            return None;
        }
        let mut code: u32 = 0;
        let mut digits = 0;
        loop {
            match self.bump() {
                Some('}') if digits >= 1 && digits <= 6 => return char::from_u32(code),
                Some(c) if digits < 6 => {
                    if let Some(h) = c.to_digit(16) {
                        code = code * 16 + h;
                        digits += 1;
                    } else {
                        self.pos = saved;
                        return None;
                    }
                }
                _ => {
                    self.pos = saved;
                    return None;
                }
            }
        }
    }

    /// The 1-based `Pos` of byte offset `idx`. `O(log N + col_len)` via the
    /// precomputed `line_starts` bsearch + a short within-line char walk
    /// (column is by character, so multibyte source files still get a
    /// correct column count from `line_start` to `idx`).
    pub fn pos_at(&self, idx: usize) -> Pos {
        let upto = idx.min(self.src.len()) as u32;
        // The line containing `idx` is the largest entry `<= idx`. Using
        // `partition_point` for the 1-based line number directly.
        let line = self.line_starts.partition_point(|&s| s <= upto) as u32;
        // Within-line column: walk chars from this line's start byte to `idx`.
        // For the prelude's mostly-ASCII source this is one byte per char;
        // multibyte chars are counted once. 1-based.
        let line_start = self.line_starts[(line - 1) as usize] as usize;
        let col = self.src[line_start..upto as usize].chars().count() as u32 + 1;
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
        assert!(matches!(s.scan_string_body(None), StringScan::Unterminated));
        assert!(s.at_end());
    }

    #[test]
    fn scan_string_body_decodes_hex_and_unicode_escapes() {
        // `\x1b` → ESC (same char as `\e`); `\u{1b}` → ESC; `\u{1F600}` → 😀.
        let mut s = Scanner::new(r#"a\x1b\u{1b}\u{1F600}b"end"#);
        let mut out = String::new();
        assert!(matches!(
            s.scan_string_body(Some(&mut out)),
            StringScan::Closed
        ));
        assert_eq!(out, "a\u{1b}\u{1b}\u{1F600}b");
    }

    #[test]
    fn malformed_hex_escapes_report_bad_escape() {
        // `\xZ` — Z isn't hex: a hard `BadEscape` at the backslash's offset
        // (kernel audit; the old literal-passthrough silently produced "xZZ").
        // The body is still scanned through the close quote.
        let mut s = Scanner::new(r#"ab\xZZ"after"#);
        let mut out = String::new();
        assert!(matches!(
            s.scan_string_body(Some(&mut out)),
            StringScan::BadEscape { at: 2 }
        ));
        assert_eq!(
            &s.src[s.pos..],
            "after",
            "scan continues past the close quote"
        );

        // Malformed `\u{}` shapes likewise; the catch-all `\X` → literal X
        // rule for other chars is unchanged.
        for bad in [
            r#"\u{}"x"#,
            r#"\u{nothex}"x"#,
            r#"\u{110000}"x"#,
            r#"\u41"x"#,
        ] {
            let mut s = Scanner::new(bad);
            assert!(
                matches!(s.scan_string_body(None), StringScan::BadEscape { at: 0 }),
                "expected BadEscape for {bad:?}"
            );
        }
        let mut s = Scanner::new(r#"\q"x"#);
        let mut out = String::new();
        assert!(matches!(
            s.scan_string_body(Some(&mut out)),
            StringScan::Closed
        ));
        assert_eq!(out, "q", "unknown non-hex escapes still pass through");
    }

    #[test]
    fn unterminated_wins_over_bad_escape() {
        // The REPL continuation prompt keys off `Unterminated`; a bad escape
        // in a string the user is still typing must not pre-empt it.
        let mut s = Scanner::new(r#"\xZZ never closed"#);
        assert!(matches!(s.scan_string_body(None), StringScan::Unterminated));
    }

    #[test]
    fn line_starts_count_lone_cr_and_unicode_separators() {
        // CRLF is one break (via its `\n`); a lone CR, U+2028, and U+2029
        // each break a line of their own (kernel audit: diagnostics after a
        // lone CR reported a wrong line:col).
        let src = "a\r\nb\rc\u{2028}d\u{2029}e";
        let s = Scanner::new(src);
        let pos_of = |ch: char| s.pos_at(src.find(ch).unwrap());
        assert_eq!((pos_of('a').line, pos_of('a').col), (1, 1));
        assert_eq!((pos_of('b').line, pos_of('b').col), (2, 1), "after CRLF");
        assert_eq!((pos_of('c').line, pos_of('c').col), (3, 1), "after lone CR");
        assert_eq!((pos_of('d').line, pos_of('d').col), (4, 1), "after U+2028");
        assert_eq!((pos_of('e').line, pos_of('e').col), (5, 1), "after U+2029");
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
        let s = Scanner::new(".)");
        assert!(s.is_dot_separator());
        // `.5` is the start of an atom, not a dotted-pair `.`.
        let t = Scanner::new(".5");
        assert!(!t.is_dot_separator());
    }
}
