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
use crate::syntax::scanner::{Scanner, StringScan};

/// Read every form in `src`.
pub fn read_all(heap: &mut Heap, src: &str) -> Result<Vec<Value>, LispError> {
    let mut parser = Parser::new(heap, src);
    let mut forms = Vec::new();
    loop {
        parser.s.skip_trivia();
        if parser.s.at_end() {
            break;
        }
        forms.push(parser.read_form()?);
    }
    Ok(forms)
}

/// Read every form in `src`, pairing each top-level form with its 1-based
/// start position. The file runner uses these so a runtime error can be
/// reported against the enclosing top-level form (see `docs/tooling.md`).
pub fn read_all_positioned(heap: &mut Heap, src: &str) -> Result<Vec<(Value, Pos)>, LispError> {
    let mut parser = Parser::new(heap, src);
    let mut forms = Vec::new();
    loop {
        parser.s.skip_trivia();
        if parser.s.at_end() {
            break;
        }
        let start = parser.s.pos_at(parser.s.pos());
        let form = parser.read_form()?;
        forms.push((form, start));
    }
    Ok(forms)
}

/// Read exactly one form, ignoring any trailing input. For internal callers that
/// pass a known single form (macro/type tests, the printer round-trip).
pub fn read_one(heap: &mut Heap, src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(heap, src);
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
}

/// Bound on parser-recursion depth. A new frame is taken for each delimited
/// form (`(`/`[`/`{`/`'`/`` ` ``/`~`); past this we return a parse error
/// instead of growing the native Rust stack (which would abort the process
/// — see `docs/devlog.md` 2026-05-28 hardening). 256 is comfortably above any
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
            ')' | ']' | '}' => Err(self.err(format!("unexpected '{}'", c))),
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
            // `docs/llm-native.md`) instead of a confusing "unbound symbol: \c".
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
    /// It used to fall through to `read_atom`, so an unrecognised `#…` interned as
    /// a *symbol* — `#foo` was a legal name, and `#|a comment|#` read as the
    /// symbol `|#\|a comment\|#|`. That quietly spent the `#` space: any `#` form
    /// added after 1.0 would be taking a token that had been a valid name, which is
    /// a breaking change. Rejecting the whole space now costs nothing (no real
    /// program names anything `#foo`) and keeps every future `#` literal purely
    /// additive — the same reasoning as the digit-led rule in
    /// [`AtomKind::ReservedNumeric`](crate::syntax::atom::AtomKind::ReservedNumeric).
    ///
    /// The Clojure/Scheme/EDN forms Brood does not have get a teaching hint naming
    /// the Brood idiom (LLM-native errors, `docs/llm-native.md`) instead of a
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
            // takes a depth level too (`enter`). Without one it bypassed the cap
            // altogether: `#{#{…}}` nested deeply enough aborted the process on a
            // native stack overflow (measured at 300k levels) — the very thing
            // `MAX_DEPTH` exists to prevent.
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
                 `(regex/match? \"pat\" s)` (or `regex/find`, `regex/replace`) — \
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening delimiter
        self.s.bump(); // opening delimiter
        let mut items = Vec::new();
        let mut tail = Value::nil();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at_incomplete(start, "unclosed list (opened here)")),
                Some(c) if c == close => {
                    self.s.bump();
                    break;
                }
                // A lone `.` introduces an improper (dotted) tail: `(a . b)`.
                Some('.') if self.s.is_dot_separator() => {
                    if items.is_empty() {
                        return Err(self.err("dotted list needs an element before '.'"));
                    }
                    self.s.bump(); // the '.'
                    self.s.skip_trivia();
                    match self.s.peek() {
                        None => return Err(self.err_incomplete("unclosed list")),
                        Some(c) if c == close => {
                            return Err(self.err("expected a form after '.' in dotted list"))
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
                        None => {
                            return Err(self.err_at_incomplete(start, "unclosed list (opened here)"))
                        }
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
    /// dotted tail, so this is always a mistake — and reading it as the *symbol*
    /// `.` (what used to happen) silently produced a collection of the wrong
    /// length: `[1 . 2]` was a three-element vector, with no diagnostic.
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening '['
        self.s.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at_incomplete(start, "unclosed vector (opened here)")),
                Some(']') => {
                    self.s.bump();
                    break;
                }
                Some('.') if self.s.is_dot_separator() => {
                    return Err(self.dot_not_allowed("vector"))
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening '{'
        self.s.bump(); // '{'
        let mut pairs = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at_incomplete(start, "unclosed map (opened here)")),
                Some('}') => {
                    self.s.bump();
                    break;
                }
                Some('.') if self.s.is_dot_separator() => return Err(self.dot_not_allowed("map")),
                Some(_) => {
                    let key = self.read_form()?;
                    self.s.skip_trivia();
                    match self.s.peek() {
                        Some('}') | None => {
                            return Err(self.err_at(
                                start,
                                "map literal has an odd number of forms (each key needs a value)",
                            ))
                        }
                        // Same rule in value position — `{:a . 1}` is not a dotted pair.
                        Some('.') if self.s.is_dot_separator() => {
                            return Err(self.dot_not_allowed("map"))
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening '{'
        self.s.bump(); // '{'
        let mut items = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at_incomplete(start, "unclosed set (opened here)")),
                Some('.') if self.s.is_dot_separator() => return Err(self.dot_not_allowed("set")),
                Some('}') => {
                    self.s.bump();
                    break;
                }
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
                "malformed string escape: an unknown letter escape like \\d \\w \\s \
                 is rejected (write \\\\d for a regex class); \\x needs two hex digits; \
                 \\u needs {1-6 hex digits} (a Unicode scalar value)"
                    .to_string(),
            )),
        }
    }

    fn read_atom(&mut self) -> Result<Value, LispError> {
        let token_start = self.s.pos();
        let token = self.s.read_atom();
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
            // Digit-led but not a number Brood has (`1/2`, `0x1F`, `1_000`, `1N`).
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
