//! The reader: turns source text into [`Value`]s. It allocates pairs/vectors/
//! strings, so it threads `&mut Heap`.
//!
//! The character stream + trivia/atom/string-body primitives live in
//! [`scanner`](super::scanner) and are shared with the CST. This module
//! handles the *structural* parsing — open/close delimiters, dotted pairs,
//! map literals, quote sigils — and the building of `Value`s through the
//! heap. Lexical rules (where an atom ends, how to classify a token)
//! continue to share [`atom`](super::atom).

use crate::core::blob::SharedBlob;
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::error::{LispError, Pos};
use crate::syntax::atom::{self, AtomKind};
use crate::syntax::scanner::{BarScan, Scanner, StringScan};

// A first-line `#!…` shebang belongs to a source FILE, so the whole-source readers
// (`read_all`, `read_all_positioned`, and `read_one`, which reads a file's leading
// header) step past it; `read_one_complete` — a datum read, `reflect/read-string` —
// does not, and reports `#!` as the dispatch error it is (`scanner::shebang_len`).

/// Read every form in `src` (a whole source text: a first-line shebang is skipped).
pub fn read_all(heap: &mut Heap, src: &str) -> Result<Vec<Value>, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.s.skip_shebang();
    let mut forms = Vec::new();
    loop {
        parser.s.skip_trivia();
        if parser.s.at_end() {
            break;
        }
        let start = parser.s.pos();
        forms.push(parser.read_form()?);
        parser.note_top_level(start);
    }
    Ok(forms)
}

/// Read every form in `src`, pairing each top-level form with its 1-based
/// start position. The file runner uses these so a runtime error can be
/// reported against the enclosing top-level form (see `docs/tooling.md`).
pub fn read_all_positioned(heap: &mut Heap, src: &str) -> Result<Vec<(Value, Pos)>, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.s.skip_shebang();
    let mut forms = Vec::new();
    loop {
        parser.s.skip_trivia();
        if parser.s.at_end() {
            break;
        }
        let start_offset = parser.s.pos();
        let start = parser.s.pos_at(start_offset);
        let form = parser.read_form()?;
        parser.note_top_level(start_offset);
        forms.push((form, start));
    }
    Ok(forms)
}

/// Read exactly one form, ignoring any trailing input. For internal callers that
/// pass a known single form (macro/type tests, the printer round-trip), and for
/// `reflect/read-first`, which reads a FILE's leading header — so it skips a shebang.
pub fn read_one(heap: &mut Heap, src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.s.skip_shebang();
    parser.s.skip_trivia();
    if parser.s.at_end() {
        return Err(parser.err_incomplete("unexpected end of input"));
    }
    parser.read_form()
}

/// Read exactly one form and require everything after it to be trivia (whitespace
/// / commas / comments). Errors if a second form follows — so `reflect/read-string` is a
/// *loud* error on trailing content, not a silent drop (use `reflect/read-all` to read
/// every form). Trailing whitespace and comments are fine.
pub fn read_one_complete(heap: &mut Heap, src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.s.skip_trivia();
    if parser.s.at_end() {
        return Err(parser.err_incomplete("unexpected end of input"));
    }
    let form = parser.read_form()?;
    parser.s.skip_trivia();
    if !parser.s.at_end() {
        return Err(parser.err(
            "unexpected trailing content after the form — reflect/read-string reads a single \
             form; use reflect/read-all (or reflect/eval-string) for input with more than one",
        ));
    }
    Ok(form)
}

struct Parser<'a> {
    heap: &'a mut Heap,
    s: Scanner<'a>,
    depth: u32,
    /// The byte span of the last top-level form read (`read_all*` only), and of the last
    /// one that began at column 1: what a stray closer at top level is reported against.
    last_top: Option<(usize, usize)>,
    last_margin_top: Option<(usize, usize)>,
}

/// Bound on parser-recursion depth. A new frame is taken for each delimited
/// form (`(`/`[`/`{`/`'`/`` ` ``/`~`); past this we return a parse error
/// instead of growing the native Rust stack (which would abort the process).
/// 256 is comfortably above any
/// hand-written program; pathological deeply-nested input from disk, the LSP,
/// or `reflect/eval-string` is rejected with `LispError::parse`.
///
/// The printer's cap is derived from this one (`printer::MAX_DEPTH`), so that
/// everything the reader accepts still prints readably.
pub(crate) const MAX_DEPTH: u32 = 256;

impl<'a> Parser<'a> {
    fn new(heap: &'a mut Heap, src: &'a str) -> Self {
        Parser {
            heap,
            s: Scanner::new(src),
            depth: 0,
            last_top: None,
            last_margin_top: None,
        }
    }

    /// Enter a nesting level. Errors out at [`MAX_DEPTH`] rather than recursing
    /// into a stack-overflow abort. Pair every successful call with [`exit`].
    fn enter(&mut self) -> Result<(), LispError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(format!("form nested too deeply (max {} levels)", MAX_DEPTH)));
        }
        self.depth += 1;
        Ok(())
    }

    fn exit(&mut self) {
        self.depth -= 1;
    }

    /// A parse error tagged with the current position.
    fn err(&self, msg: impl Into<String>) -> LispError {
        LispError::parse(msg).with_pos(self.s.pos_at(self.s.pos()))
    }

    /// A parse error tagged with a specific position (e.g. where a delimiter
    /// opened, which is more useful for "unclosed" than the EOF position).
    fn err_at(&self, pos: Pos, msg: impl Into<String>) -> LispError {
        LispError::parse(msg).with_pos(pos)
    }

    /// An *incomplete input* parse error — input ended mid-form or mid-string.
    /// Tagged with `INCOMPLETE_INPUT` so a REPL / editor can distinguish "needs
    /// more input" (keep reading) from a genuine syntax error, without having to
    /// re-scan the text for balanced delimiters.
    fn err_incomplete(&self, msg: impl Into<String>) -> LispError {
        self.err(msg)
            .with_code(crate::error::error_codes::INCOMPLETE_INPUT)
    }

    /// `err_at` for an incomplete-input error (see [`err_incomplete`]).
    fn err_at_incomplete(&self, pos: Pos, msg: impl Into<String>) -> LispError {
        self.err_at(pos, msg)
            .with_code(crate::error::error_codes::INCOMPLETE_INPUT)
    }

    /// Record the top-level form that began at `start` and has just been read, for
    /// [`Parser::stray_closer`].
    fn note_top_level(&mut self, start: usize) {
        let span = (start, self.s.pos());
        self.last_top = Some(span);
        let source = self.s.src().as_bytes();
        if start == 0 || matches!(source[start - 1], b'\n' | b'\r') {
            self.last_margin_top = Some(span);
        }
    }

    /// The position of the last character before byte offset `end` (a form's closing
    /// delimiter, when the form has one).
    fn last_char_pos(&self, end: usize) -> Pos {
        let source = self.s.src();
        let offset = source[..end]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.s.pos_at(offset)
    }

    /// A closer with nothing open to close. At top level, name the form it falls
    /// after: the closer usually belongs to a form that already closed too early. When
    /// the last form began indented, the one before it at the margin is the one that
    /// closed early, and the indented lines are its body read as top-level forms.
    fn stray_closer(&self, closer: char) -> LispError {
        let base = format!("unexpected '{closer}'");
        if self.depth > 0 {
            // After a sigil (`'`, `^`, `~`): the sigil wanted a form.
            return self.err(base);
        }
        let source = self.s.src();
        let describe = |(start, end): (usize, usize)| {
            (
                form_preview(source, start),
                self.s.pos_at(start),
                self.last_char_pos(end),
            )
        };
        let detail = match (self.last_top, self.last_margin_top) {
            (Some(last), Some(margin)) if last != margin => {
                let (text, start, end) = describe(margin);
                format!(
                    ": `{text}` that started at {}:{} already closed at {}:{}, so the indented \
                     lines after it are read as top-level forms",
                    start.line, start.col, end.line, end.col
                )
            }
            (Some(last), _) => {
                let (text, start, end) = describe(last);
                format!(
                    ": the form `{text}` that started at {}:{} closed at {}:{}",
                    start.line, start.col, end.line, end.col
                )
            }
            (None, _) => String::new(),
        };
        self.err(format!("{base} — nothing is open here{detail}"))
    }

    /// A closer of the wrong kind inside an open `kind` that `expected` closes.
    fn mismatched(&self, found: char, expected: char, kind: &str, start: Pos) -> LispError {
        self.err(format!(
            "unexpected '{found}' — expected '{expected}' to close the {kind} opened at {}:{}",
            start.line, start.col
        ))
    }

    /// Input ended with the `kind` that opened at `start` still open. Incomplete input
    /// (a REPL keeps reading). When a later line starts at or left of an opener that is
    /// still open, name that opener and that line: the closer belongs before it
    /// ([`unclosed_hint`]). Otherwise point at the innermost opener, as before.
    fn unclosed(&self, start: usize, kind: &str) -> LispError {
        let source = self.s.src();
        match unclosed_hint(source) {
            Some(hint) => {
                let opener = self.s.pos_at(hint.opener.offset);
                let line_start = self.s.pos_at(hint.line_offset);
                let closers: String = hint.closers.iter().collect();
                let missing = if hint.closers.len() == 1 {
                    format!("a `{closers}` is missing")
                } else {
                    format!("`{closers}` are missing")
                };
                self.err_at_incomplete(
                    opener,
                    format!(
                        "unclosed {}: `{}` opened at {}:{} is still open when line {} starts \
                         `{}` at column {} — {missing} before line {}",
                        hint.opener.kind,
                        form_preview(source, hint.opener.offset),
                        opener.line,
                        opener.col,
                        line_start.line,
                        form_preview(source, hint.line_offset),
                        line_start.col,
                        line_start.line,
                    ),
                )
            }
            None => self.err_at_incomplete(
                self.s.pos_at(start),
                format!("unclosed {kind} (opened here)"),
            ),
        }
    }

    fn read_form(&mut self) -> Result<Value, LispError> {
        self.s.skip_trivia();
        let c = self
            .s
            .peek()
            .ok_or_else(|| self.err_incomplete("unexpected end of input"))?;
        // Every branch below that recurses through `read_form` is guarded by
        // `enter`/`exit` so a deeply nested input (e.g. 100 000 open parens
        // from a malicious file or LSP buffer) returns a clean parse error
        // instead of overflowing the native Rust stack.
        match c {
            '(' => {
                self.enter()?;
                let v = self.read_seq(')');
                self.exit();
                v
            }
            '[' => {
                self.enter()?;
                let v = self.read_vector();
                self.exit();
                v
            }
            ')' | ']' | '}' => Err(self.stray_closer(c)),
            '{' => {
                self.enter()?;
                let v = self.read_map();
                self.exit();
                v
            }
            '\'' => {
                self.enter()?;
                let v = self.read_wrapped(kw::QUOTE);
                self.exit();
                v
            }
            '`' => {
                self.enter()?;
                let v = self.read_wrapped(kw::QUASIQUOTE);
                self.exit();
                v
            }
            // `^expr` — a **pin** in pattern position: match against the current
            // value of `expr` instead of binding a name. Its own reader macro (not
            // `~`, which belongs to quasiquote) so a macro template can emit one.
            '^' => {
                self.enter()?;
                let v = self.read_wrapped(kw::PIN);
                self.exit();
                v
            }
            '~' => {
                self.enter()?;
                self.s.bump(); // '~'
                let v = if self.s.peek() == Some('@') {
                    self.s.bump();
                    let form = self.read_form()?;
                    Ok(self.wrap(kw::UNQUOTE_SPLICING, form))
                } else {
                    let form = self.read_form()?;
                    Ok(self.wrap(kw::UNQUOTE, form))
                };
                self.exit();
                v
            }
            '"' => self.read_string(),
            '#' => self.read_hash(),
            // `|…|` bar-quoted symbol, and `:|…|` bar-quoted keyword — the round-trip
            // form the printer emits for a symbol/keyword whose name isn't a clean
            // token (holds whitespace/delimiters, is empty, or would read back as a
            // number/keyword/`nil`). A bare `:` before `|` is the keyword marker.
            '|' => self.read_bar_symbol(false),
            ':' if self.s.peek_after() == Some('|') => self.read_bar_symbol(true),
            // `\c` / `\newline` — Clojure/Scheme character literal. Brood has no
            // char type (a `\` at form start otherwise reads as a stray symbol like
            // `\c`), so catch it with a teaching hint (LLM-native errors,
            // `docs/archive/llm-native.md`) instead of a confusing "unbound symbol: \c".
            '\\' => Err(self
                .err("`\\c` is a Clojure/Scheme character literal, which Brood does not have")
                .with_hint(
                    "Brood has no character type — a character is just a 1-char string. \
                 Write `\"c\"` (or `(string/int->char 99)` from a codepoint).",
                )),
            _ => self.read_atom(),
        }
    }

    /// Read a `|…|` bar-quoted symbol (or, when `keyword`, a `:|…|` keyword). `pos` is
    /// on the `|` (or the `:` for a keyword). The body decodes `\|`/`\\`; an unclosed
    /// bar is a clean parse error.
    fn read_bar_symbol(&mut self, keyword: bool) -> Result<Value, LispError> {
        let start = self.s.pos();
        if keyword {
            self.s.bump(); // ':'
        }
        self.s.bump(); // opening '|'
        let mut name = String::new();
        match self.s.scan_bar_body(Some(&mut name)) {
            crate::syntax::scanner::BarScan::Closed => {
                if keyword {
                    Ok(value::kw(&name))
                } else {
                    Ok(value::sym(&name))
                }
            }
            crate::syntax::scanner::BarScan::Unterminated => Err(self.err_at(
                self.s.pos_at(start),
                "unterminated |…| bar-quoted symbol".to_string(),
            )),
        }
    }

    /// Dispatch a leading `#`. **`#` is a dispatch character, not an atom
    /// character**: `#{…}` (set) and `#b"…"` (bytes) are the only two forms, and
    /// anything else after a leading `#` is a reader error.
    ///
    /// Rejecting the whole `#` space (rather than reading `#foo` as a symbol) keeps
    /// every future `#` literal purely additive — the same reasoning as the digit-led
    /// rule in
    /// [`AtomKind::ReservedNumeric`](crate::syntax::atom::AtomKind::ReservedNumeric).
    ///
    /// The Clojure/Scheme/EDN forms Brood does not have get a teaching hint naming
    /// the Brood idiom (LLM-native errors, `docs/archive/llm-native.md`) instead of a
    /// confusing downstream failure.
    fn read_hash(&mut self) -> Result<Value, LispError> {
        if self.s.starts_with("#b\"") {
            self.s.bump(); // '#'
            self.s.bump(); // 'b'
            return self.read_bytes();
        }
        let pos = self.s.pos_at(self.s.pos());
        match self.s.peek_after() {
            // `#{…}` — a set literal (`Value::Set`). Consume the '#' and read the
            // brace-delimited elements; the evaluator evaluates + dedups them.
            // A set literal nests through `read_form` exactly like `(`/`[`/`{`, so it
            // takes a depth level too (`enter`), or deeply nested `#{#{…}}` would
            // overflow the native stack past `MAX_DEPTH`.
            Some('{') => {
                self.s.bump(); // '#'
                self.enter()?;
                let v = self.read_set();
                self.exit();
                v
            }
            // `#(…)` — Clojure anonymous-function reader macro.
            Some('(') => Err(LispError::parse(
                "`#(…)` is Clojure's anonymous-function reader macro, which Brood does not have",
            )
            .with_pos(pos)
            .with_hint(
                "Write the lambda out: Brood uses `(fn (x) …)` — e.g. `#(+ 1 %)` \
                 becomes `(fn (x) (+ 1 x))`.",
            )),
            // `#'foo` — Clojure var-quote.
            Some('\'') => Err(LispError::parse(
                "`#'` is Clojure's var-quote, which Brood does not have",
            )
            .with_pos(pos)
            .with_hint("Brood symbols are ordinary values — use a plain quote: `'foo`.")),
            // `#_` — Clojure/EDN discard reader macro (skip the next form).
            Some('_') => Err(LispError::parse(
                "`#_` is Clojure/EDN's discard reader macro, which Brood does not have",
            )
            .with_pos(pos)
            .with_hint(
                "Wrap the form in `(comment …)` — its body is read but never \
                 evaluated — or comment it out with `;` (a line comment runs to end \
                 of line).",
            )),
            // `#"…"` — Clojure regex literal. (`#b"…"` was handled above, so a `#"`
            // here is unambiguously the regex form.)
            Some('"') => Err(LispError::parse(
                "`#\"…\"` is Clojure's regex literal, which Brood does not have",
            )
            .with_pos(pos)
            .with_hint(
                "Brood regexes are library values in the `regex` module: \
                 `(regex/match? s \"pat\")` (or `regex/find`, `regex/replace`) — \
                 referencing a `regex/…` name loads the module on demand.",
            )),
            // `#|…|#` — Scheme/CL block comment. Read as a bar-quoted symbol before
            // this arm existed, so it silently became a name instead of a comment.
            Some('|') => Err(LispError::parse(
                "`#|…|#` is a Scheme/Common Lisp block comment, which Brood does not have",
            )
            .with_pos(pos)
            .with_hint(
                "Comment each line with `;` (a line comment runs to end of line), or \
                 wrap the forms in `(comment …)` — its body is read but never evaluated.",
            )),
            // Any other `#…`, and a bare trailing `#`. Reserved, not a symbol.
            _ => Err(LispError::parse(
                "`#` is a dispatch character, and this is not one of Brood's `#` forms",
            )
            .with_pos(pos)
            .with_hint(
                "The only `#` literals are `#{…}` (a set) and `#b\"…\"` (bytes). `#` \
                 cannot start a name — if you meant one, drop the `#`. (A trailing \
                 `#`, as in `x#`, is different: that is auto-gensym inside a \
                 quasiquote.)",
            )),
        }
    }

    /// Read a `#b"…"` bytes literal. The body is scanned like a string, then each
    /// codepoint becomes one byte: printable ASCII is itself, other bytes are
    /// `\xHH`. A codepoint > 255 is an error — use `\xHH`, or
    /// `string->utf8-bytes` for UTF-8 text.
    fn read_bytes(&mut self) -> Result<Value, LispError> {
        self.s.bump(); // opening quote
        let mut body = String::new();
        match self.s.scan_string_body(Some(&mut body)) {
            StringScan::Closed => {
                let mut bytes = Vec::with_capacity(body.len());
                for ch in body.chars() {
                    let cp = ch as u32;
                    if cp > 255 {
                        return Err(self.err(format!(
                            "bytes literal: codepoint U+{:04X} exceeds 255 — use \\xHH, \
                             or string->utf8-bytes for UTF-8 text",
                            cp
                        )));
                    }
                    bytes.push(cp as u8);
                }
                Ok(self.heap.alloc_bytes(SharedBlob::new(&bytes)))
            }
            StringScan::Unterminated => Err(self.err_incomplete("unterminated bytes literal")),
            StringScan::BadEscape { at } => Err(self.err_at(
                self.s.pos_at(at),
                "malformed escape in bytes literal: an unknown letter escape (\\d, \\w, …) \
                 is rejected (write \\\\d); \\x needs two hex digits",
            )),
        }
    }

    /// Read `<form>` and wrap it as `(tag form)`.
    fn read_wrapped(&mut self, tag: &str) -> Result<Value, LispError> {
        self.s.bump(); // sigil
        let form = self.read_form()?;
        Ok(self.wrap(tag, form))
    }

    fn wrap(&mut self, tag: &str, form: Value) -> Value {
        self.heap.list(vec![value::sym(tag), form])
    }

    fn read_seq(&mut self, close: char) -> Result<Value, LispError> {
        let start_offset = self.s.pos();
        let start = self.s.pos_at(start_offset); // position of the opening delimiter
        self.s.bump(); // opening delimiter
        let mut items = Vec::new();
        let mut tail = Value::nil();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.unclosed(start_offset, "list")),
                Some(c) if c == close => {
                    self.s.bump();
                    break;
                }
                Some(c) if is_closer(c) => return Err(self.mismatched(c, close, "list", start)),
                // A lone `.` introduces an improper (dotted) tail: `(a . b)`.
                Some('.') if self.s.is_dot_separator() => {
                    if items.is_empty() {
                        return Err(self.err("dotted list needs an element before '.'"));
                    }
                    self.s.bump(); // the '.'
                    self.s.skip_trivia();
                    match self.s.peek() {
                        None => return Err(self.unclosed(start_offset, "list")),
                        Some(c) if c == close => {
                            return Err(self.err("expected a form after '.' in dotted list"));
                        }
                        Some(_) => tail = self.read_form()?,
                    }
                    self.s.skip_trivia();
                    match self.s.peek() {
                        Some(c) if c == close => {
                            self.s.bump();
                            break;
                        }
                        // Input ran out after the dotted tail (`(1 . 2` at EOF). The
                        // form after `.` WAS given — what's missing is the close — so
                        // this is *incomplete input*, tagged like every other unclosed
                        // delimiter so a REPL/editor offers a continuation prompt
                        // instead of a hard syntax error (see `err_at_incomplete`).
                        None => return Err(self.unclosed(start_offset, "list")),
                        _ => return Err(self.err("expected one form after '.' before close")),
                    }
                }
                Some(_) => items.push(self.read_form()?),
            }
        }
        let form = self.heap.list_with_tail(items, tail);
        self.heap.set_form_pos(form, start); // for (form-pos …); see docs/tooling.md
        Ok(form)
    }

    /// A lone `.` inside a `[…]` / `{…}` / `#{…}` literal. Only a list has a
    /// dotted tail, so this is always a mistake — reading it as the *symbol* `.`
    /// would silently make `[1 . 2]` a three-element vector.
    fn dot_not_allowed(&self, kind: &str) -> LispError {
        self.err(format!(
            "'.' is the dotted-pair separator for lists — a {kind} literal has no dotted tail"
        ))
        .with_hint(
            "Drop the `.`. For a dotted pair use a list: `(a . b)`. For the symbol `.` \
             itself, write it bar-quoted: `|.|`.",
        )
    }

    fn read_vector(&mut self) -> Result<Value, LispError> {
        // No `set_form_pos`: the form-pos table is keyed by LOCAL *pair* index
        // (heap.rs `set_form_pos`/`form_pos` no-op on non-pairs), and only
        // call-shaped lists carry the runtime-error position. A vector/map isn't
        // a pair, so a position would be unrecorded — the exemption is deliberate.
        let start_offset = self.s.pos();
        let start = self.s.pos_at(start_offset); // position of the opening '['
        self.s.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.unclosed(start_offset, "vector")),
                Some(']') => {
                    self.s.bump();
                    break;
                }
                Some(c) if is_closer(c) => return Err(self.mismatched(c, ']', "vector", start)),
                Some('.') if self.s.is_dot_separator() => {
                    return Err(self.dot_not_allowed("vector"));
                }
                Some(_) => items.push(self.read_form()?),
            }
        }
        Ok(self.heap.alloc_vector(items))
    }

    /// Read a map literal `{ k v k v … }`. Keys and values are read as
    /// (unevaluated) forms in source order; the evaluator evaluates them and
    /// canonicalises (last-wins dedup). Commas are whitespace, so
    /// `{:a 1, :b 2}` reads the same as `{:a 1 :b 2}`.
    fn read_map(&mut self) -> Result<Value, LispError> {
        // No `set_form_pos` — see `read_vector`: the form-pos table is pair-keyed
        // and only call-shaped lists carry a runtime-error position.
        let start_offset = self.s.pos();
        let start = self.s.pos_at(start_offset); // position of the opening '{'
        self.s.bump(); // '{'
        let mut pairs = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.unclosed(start_offset, "map")),
                Some('}') => {
                    self.s.bump();
                    break;
                }
                Some(c) if is_closer(c) => return Err(self.mismatched(c, '}', "map", start)),
                Some('.') if self.s.is_dot_separator() => return Err(self.dot_not_allowed("map")),
                Some(_) => {
                    let key = self.read_form()?;
                    self.s.skip_trivia();
                    match self.s.peek() {
                        None => return Err(self.unclosed(start_offset, "map")),
                        Some('}') => {
                            return Err(self.err_at(
                                start,
                                "map literal has an odd number of forms (each key needs a value)",
                            ));
                        }
                        // Same rule in value position — `{:a . 1}` is not a dotted pair.
                        Some('.') if self.s.is_dot_separator() => {
                            return Err(self.dot_not_allowed("map"));
                        }
                        Some(c) if is_closer(c) => {
                            return Err(self.mismatched(c, '}', "map", start));
                        }
                        Some(_) => {
                            let val = self.read_form()?;
                            pairs.push((key, val));
                        }
                    }
                }
            }
        }
        Ok(self.heap.map_from_pairs(pairs))
    }

    /// Read a set literal `#{ a b c … }`. Elements are read as (unevaluated) forms
    /// in source order; the evaluator evaluates them and dedups by structural
    /// equality (`set_from_elems`). Commas are whitespace, so `#{1, 2, 3}` reads
    /// the same as `#{1 2 3}`. The `#` is consumed by `read_hash`; this consumes
    /// the `{`.
    fn read_set(&mut self) -> Result<Value, LispError> {
        // No `set_form_pos` — see `read_vector`: the form-pos table is pair-keyed.
        let start_offset = self.s.pos();
        let start = self.s.pos_at(start_offset); // position of the opening '{'
        self.s.bump(); // '{'
        let mut items = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.unclosed(start_offset, "set")),
                Some('.') if self.s.is_dot_separator() => return Err(self.dot_not_allowed("set")),
                Some('}') => {
                    self.s.bump();
                    break;
                }
                Some(c) if is_closer(c) => return Err(self.mismatched(c, '}', "set", start)),
                Some(_) => items.push(self.read_form()?),
            }
        }
        Ok(self.heap.set_from_elems(items))
    }

    fn read_string(&mut self) -> Result<Value, LispError> {
        self.s.bump(); // opening quote
        let mut s = String::new();
        match self.s.scan_string_body(Some(&mut s)) {
            StringScan::Closed => Ok(self.heap.alloc_string(&s)),
            StringScan::Unterminated => Err(self.err_incomplete("unterminated string")),
            StringScan::BadEscape { at } => Err(self.err_at(
                self.s.pos_at(at),
                // Name the escape that failed, so a malformed `\u{…}` is not reported as
                // an unknown letter escape (the regex footgun).
                match self.s.char_at(at + 1) {
                    Some('u') => {
                        "malformed \\u escape: write \\u{H…H} with 1 to 6 hex digits of a \
                                  Unicode scalar value (up to U+10FFFF, no surrogates); \\u00e9 \
                                  without braces is not a Brood escape"
                            .to_string()
                    }
                    Some('x') => {
                        "malformed \\x escape: \\xHH needs exactly two hex digits (an ASCII \
                                  byte); for a codepoint write \\u{…}"
                            .to_string()
                    }
                    _ => "malformed string escape: an unknown letter escape like \\d \\w \\s \
                          is rejected (write \\\\d for a regex class); \\x needs two hex digits; \
                          \\u needs {1-6 hex digits} (a Unicode scalar value)"
                        .to_string(),
                },
            )),
        }
    }

    fn read_atom(&mut self) -> Result<Value, LispError> {
        let token_start = self.s.pos();
        let token = self.s.read_atom();
        // A control character is not a delimiter (it is not whitespace), so a NUL in the
        // source would become PART of a symbol. Name it instead.
        if let Some(c) = token.chars().find(|c| c.is_control()) {
            return Err(self.err_at(
                self.s.pos_at(token_start),
                format!(
                    "control character U+{:04X} inside a symbol: `{}`",
                    c as u32,
                    token.escape_default()
                ),
            ));
        }
        match atom::classify(token) {
            AtomKind::Nil => Ok(Value::nil()),
            AtomKind::Bool(b) => Ok(Value::boolean(b)),
            AtomKind::Int(i) => Ok(Value::int(i)),
            AtomKind::Float(f) => Ok(Value::float(f)),
            // `atom::classify` only returns `Keyword` for a non-empty `:`-prefixed
            // token, so dropping the `:` always leaves a non-empty name.
            AtomKind::Keyword => Ok(value::kw(&token[1..])),
            AtomKind::Symbol => Ok(value::sym(token)),
            // An integer-shaped literal too big for i64 is a bignum, not an
            // error: parse the decimal text into a `num_bigint::BigInt` and
            // allocate a `Value::BigInt`. `looks_integer` guaranteed the token is
            // all digits + optional sign, so the parse only fails on something
            // `classify` would never have routed here — guard it anyway.
            AtomKind::IntOverflow => match token.parse::<num_bigint::BigInt>() {
                Ok(n) => Ok(self.heap.alloc_bigint(n)),
                Err(_) => Err(self.err_at(
                    self.s.pos_at(token_start),
                    format!("malformed integer literal: {}", token),
                )),
            },
            // A `M`-suffixed decimal literal (`1.50M`). `classify` already validated
            // the prefix, so strip the suffix and parse it as a BigDecimal.
            AtomKind::Decimal => match token[..token.len() - 1].parse::<bigdecimal::BigDecimal>() {
                Ok(n) => Ok(self.heap.alloc_decimal(n)),
                Err(_) => Err(self.err_at(
                    self.s.pos_at(token_start),
                    format!("malformed decimal literal: {}", token),
                )),
            },
            AtomKind::DecimalInvalid => Err(self.err_at(
                self.s.pos_at(token_start),
                format!("malformed decimal literal: {}", token),
            )),
            AtomKind::FloatInvalid => Err(self.err_at(
                self.s.pos_at(token_start),
                format!("malformed float literal: {}", token),
            )),
            // A ratio literal `num/den` (`1/2`, `-3/4`). `classify` validated the
            // shape; `BigRational`'s parse reduces it, and `alloc_ratio` demotes a
            // denominator of 1 to an `Int` (so `4/2` reads as `2`).
            AtomKind::Ratio => match token.parse::<num_rational::BigRational>() {
                Ok(n) => Ok(self.heap.alloc_ratio(n)),
                Err(_) => Err(self.err_at(
                    self.s.pos_at(token_start),
                    format!("malformed ratio literal: {}", token),
                )),
            },
            AtomKind::RatioInvalid => Err(self
                .err_at(
                    self.s.pos_at(token_start),
                    format!("malformed ratio literal: {}", token),
                )
                .with_hint(
                    "a ratio is `num/den` with an integer numerator over a positive, \
                     nonzero integer denominator — write `-1/2` (sign on the numerator), \
                     not `1/-2`, and not `1/0`",
                )),
            // A radix literal past `i64` is a bignum, not an error — the same call
            // `IntOverflow` makes, but parsing the digits at the token's own radix
            // and re-applying the sign. `classify_radix` already validated the
            // digits, so the parse can only fail on something it would never have
            // routed here; handled rather than asserted.
            AtomKind::RadixOverflow => {
                match atom::radix_parts(token).and_then(|(radix, digits, negative)| {
                    num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
                        .map(|magnitude| if negative { -magnitude } else { magnitude })
                }) {
                    Some(n) => Ok(self.heap.alloc_bigint(n)),
                    None => Err(self.err_at(
                        self.s.pos_at(token_start),
                        format!("malformed radix literal: {}", token),
                    )),
                }
            }
            // Digits wrong for the radix (`0b102`), or none at all (`0x`). The hint
            // comes from `atom` so the CST explains it identically.
            AtomKind::RadixInvalid => Err(self
                .err_at(
                    self.s.pos_at(token_start),
                    format!("malformed radix literal: {}", token),
                )
                .with_hint(atom::radix_invalid_hint(token))),
            // Digit-led but not a number Brood has (`1_000`, `1N`, `3px`).
            // Reserved syntax, so it errors here rather than interning as a symbol
            // and resurfacing later as a puzzling "unbound symbol". The hint comes
            // from `atom` so the CST explains it identically.
            AtomKind::ReservedNumeric => Err(self
                .err_at(
                    self.s.pos_at(token_start),
                    format!("`{}` is reserved numeric syntax, not a name", token),
                )
                .with_hint(atom::reserved_numeric_hint(token))),
        }
    }
}

fn is_closer(character: char) -> bool {
    matches!(character, ')' | ']' | '}')
}

/// A delimiter the diagnostic re-scan saw open: where, what kind, what closes it.
#[derive(Clone, Copy, Debug)]
struct OpenDelimiter {
    offset: usize,
    kind: &'static str,
    close: char,
}

/// Where an unclosed delimiter's closer belongs: `opener` is still open when the line
/// at `line_offset` starts at or left of it, and `closers` (innermost first) are the
/// delimiters that line's dedent leaves still open.
struct UnclosedHint {
    opener: OpenDelimiter,
    line_offset: usize,
    closers: Vec<char>,
}

/// Re-scan `source` lexically (strings, comments, bar symbols and bytes skipped) and
/// return the delimiters still open at its end, outermost first, with the offset of the
/// first token on each line that is not a closer. Only run once reading has failed,
/// so the reader's own path pays nothing for it.
fn scan_open_delimiters(source: &str) -> (Vec<OpenDelimiter>, Vec<usize>) {
    let mut scanner = Scanner::new(source);
    scanner.skip_shebang();
    let mut open: Vec<OpenDelimiter> = Vec::new();
    let mut line_firsts = Vec::new();
    let mut last_line = 0;
    loop {
        scanner.skip_trivia();
        let offset = scanner.pos();
        let Some(character) = scanner.peek() else {
            break;
        };
        let line = scanner.pos_at(offset).line;
        if line != last_line && !is_closer(character) {
            line_firsts.push(offset);
        }
        let mut push = |kind, close| {
            open.push(OpenDelimiter {
                offset,
                kind,
                close,
            })
        };
        // A string or bar body can span lines: its LAST line is the one a following
        // token shares. Every other token ends on the line it starts.
        let mut end_line = line;
        match character {
            '(' => {
                push("list", ')');
                scanner.bump();
            }
            '[' => {
                push("vector", ']');
                scanner.bump();
            }
            '{' => {
                push("map", '}');
                scanner.bump();
            }
            '#' if scanner.starts_with("#{") => {
                push("set", '}');
                scanner.bump();
                scanner.bump();
            }
            ')' | ']' | '}' => {
                scanner.bump();
                if open.last().is_some_and(|top| top.close == character) {
                    open.pop();
                }
            }
            '"' => {
                scanner.bump();
                if !skip_string_body(&mut scanner) {
                    break;
                }
                end_line = scanner.pos_at(scanner.pos() - 1).line;
            }
            '#' if scanner.starts_with("#b\"") => {
                scanner.bump();
                scanner.bump();
                if !skip_string_body(&mut scanner) {
                    break;
                }
                end_line = scanner.pos_at(scanner.pos() - 1).line;
            }
            '|' | ':' if character == '|' || scanner.starts_with(":|") => {
                if character == ':' {
                    scanner.bump();
                }
                scanner.bump();
                if !matches!(scanner.scan_bar_body(None), BarScan::Closed) {
                    break;
                }
                end_line = scanner.pos_at(scanner.pos() - 1).line;
            }
            _ => {
                if scanner.read_atom().is_empty() {
                    scanner.bump();
                }
            }
        }
        last_line = end_line;
    }
    (open, line_firsts)
}

/// Step past a string body (the scanner is just past its opening quote). False when
/// the string runs to the end of the source.
fn skip_string_body(scanner: &mut Scanner) -> bool {
    !matches!(scanner.scan_string_body(None), StringScan::Unterminated)
}

/// Where the closer of an unclosed delimiter belongs, parinfer-style: for each
/// delimiter still open at the end of `source`, the first later line whose first token
/// starts at or left of the opener's own column has left its body. The earliest such
/// line wins; on a tie the innermost opener is named. `None` when no line dedents.
fn unclosed_hint(source: &str) -> Option<UnclosedHint> {
    let (open, line_firsts) = scan_open_delimiters(source);
    let positions = Scanner::new(source);
    let mut best: Option<(usize, usize)> = None; // (line offset, index into `open`)
    for (index, opener) in open.iter().enumerate() {
        let opener_pos = positions.pos_at(opener.offset);
        let first_after = line_firsts.partition_point(|&offset| offset <= opener.offset);
        let dedent = line_firsts[first_after..].iter().copied().find(|&offset| {
            let token_pos = positions.pos_at(offset);
            token_pos.line > opener_pos.line && token_pos.col <= opener_pos.col
        });
        if let Some(line_offset) = dedent {
            // `<=`: a later (inner) opener wins a tie.
            if best.is_none_or(|(best_offset, _)| line_offset <= best_offset) {
                best = Some((line_offset, index));
            }
        }
    }
    let (line_offset, index) = best?;
    let column = positions.pos_at(line_offset).col;
    let closers = open[..=index]
        .iter()
        .rev()
        .filter(|outer| positions.pos_at(outer.offset).col >= column)
        .map(|outer| outer.close)
        .collect();
    Some(UnclosedHint {
        opener: open[index],
        line_offset,
        closers,
    })
}

/// A short excerpt of the form at `offset`, for a message: its first word, and the
/// name too when the head is a `def…` (`(defn parse`, `(let`, `[x`).
fn form_preview(source: &str, offset: usize) -> String {
    let line = source[offset..].lines().next().unwrap_or("");
    let mut words = line.split_whitespace();
    let head = words.next().unwrap_or("");
    let text = match words.next() {
        Some(name) if head.starts_with("(def") => format!("{head} {name}"),
        _ => head.to_string(),
    };
    if text.chars().count() > 40 {
        let cut: String = text.chars().take(40).collect();
        format!("{cut}…")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::error_codes::INCOMPLETE_INPUT;

    /// The read error for `source`, with its position.
    fn read_error(source: &str) -> LispError {
        let mut heap = Heap::new();
        read_all(&mut heap, source).expect_err("the source does not read")
    }

    fn at(error: &LispError) -> (u32, u32) {
        let pos = error.pos.expect("a read error has a position");
        (pos.line, pos.col)
    }

    #[test]
    fn a_missing_closer_names_the_opener_and_the_line_it_belongs_before() {
        let error = read_error("(defn a ()\n  (let (x 1)\n    (foo x)\n\n(defn b ()\n  2)\n");
        assert_eq!(
            error.message,
            "unclosed list: `(let` opened at 2:3 is still open when line 5 starts `(defn b` at \
             column 1 — `))` are missing before line 5"
        );
        assert_eq!(at(&error), (2, 3));
        // Still incomplete input, so a REPL keeps reading a multi-line form.
        assert_eq!(error.code, Some(INCOMPLETE_INPUT));
    }

    #[test]
    fn one_missing_closer_names_the_outer_form_the_dedent_leaves() {
        let error = read_error("(defn a ()\n  (let (x 1)\n    (foo x))\n(defn b ()\n  2)\n");
        assert_eq!(
            error.message,
            "unclosed list: `(defn a` opened at 1:1 is still open when line 4 starts `(defn b` \
             at column 1 — a `)` is missing before line 4"
        );
        assert_eq!(at(&error), (1, 1));
    }

    #[test]
    fn a_dedent_inside_a_string_or_a_closed_form_is_not_a_hint() {
        // The docstring's second line and the closed `(foo` are not dedents of an open
        // form; with no dedent the innermost opener is named, as it always was.
        let error = read_error("(defn a ()\n  \"doc\nmore\"\n  (let (x (foo\n  1))\n    x)");
        assert_eq!(error.message, "unclosed list (opened here)");
        assert_eq!(at(&error), (1, 1));
        assert_eq!(error.code, Some(INCOMPLETE_INPUT));
        let error = read_error("[1 2");
        assert_eq!(error.message, "unclosed vector (opened here)");
        assert_eq!(error.code, Some(INCOMPLETE_INPUT));
    }

    #[test]
    fn an_unclosed_map_value_is_incomplete_input() {
        let error = read_error("{:a");
        assert_eq!(error.message, "unclosed map (opened here)");
        assert_eq!(error.code, Some(INCOMPLETE_INPUT));
    }

    #[test]
    fn a_wrong_closer_names_the_one_expected_and_its_opener() {
        let cases = [
            (
                "(def v\n  [1 2))",
                "unexpected ')' — expected ']' to close the vector opened at 2:3",
                (2, 7),
            ),
            (
                "(f 1]",
                "unexpected ']' — expected ')' to close the list opened at 1:1",
                (1, 5),
            ),
            (
                "{:a 1)",
                "unexpected ')' — expected '}' to close the map opened at 1:1",
                (1, 6),
            ),
            (
                "{:a )",
                "unexpected ')' — expected '}' to close the map opened at 1:1",
                (1, 5),
            ),
            (
                "#{1 2]",
                "unexpected ']' — expected '}' to close the set opened at 1:2",
                (1, 6),
            ),
        ];
        for (source, message, position) in cases {
            let error = read_error(source);
            assert_eq!(error.message, message, "{source}");
            assert_eq!(at(&error), position, "{source}");
            assert_ne!(
                error.code,
                Some(INCOMPLETE_INPUT),
                "{source}: a wrong closer is not incomplete input"
            );
        }
    }

    #[test]
    fn an_extra_closer_names_the_form_it_falls_after() {
        let error = read_error("(defn a ()\n  (let (x 1)\n    (foo x))))\n");
        assert_eq!(
            error.message,
            "unexpected ')' — nothing is open here: the form `(defn a` that started at 1:1 \
             closed at 3:13"
        );
        assert_eq!(at(&error), (3, 14));
        assert_eq!(
            read_error(")").message,
            "unexpected ')' — nothing is open here"
        );
    }

    #[test]
    fn an_extra_closer_after_indented_top_level_forms_names_the_form_that_closed_early() {
        let error = read_error("(defn a ()\n  (let (x 1)\n    (foo x)))\n  (bar x))\n");
        assert_eq!(
            error.message,
            "unexpected ')' — nothing is open here: `(defn a` that started at 1:1 already \
             closed at 3:13, so the indented lines after it are read as top-level forms"
        );
        assert_eq!(at(&error), (4, 10));
    }
}
