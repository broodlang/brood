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
/// / commas / comments). Errors if a second form follows — so `read-string` is a
/// *loud* error on trailing content, not a silent drop (use `read-all` to read
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
            "unexpected trailing content after the form — read-string reads a single \
             form; use read-all (or eval-string) for input with more than one",
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
/// or `eval-string` is rejected with `LispError::parse`.
const MAX_DEPTH: u32 = 256;

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
            _ => self.read_atom(),
        }
    }

    /// Dispatch a leading `#`. Only `#b"…"` is special (a bytes literal); `#` is
    /// otherwise an ordinary atom character, so anything else (`#q`, `#foo`) reads
    /// as a symbol.
    fn read_hash(&mut self) -> Result<Value, LispError> {
        if self.s.starts_with("#b\"") {
            self.s.bump(); // '#'
            self.s.bump(); // 'b'
            self.read_bytes()
        } else {
            self.read_atom()
        }
    }

    /// Read a `#b"…"` bytes literal. The body is scanned like a string, then each
    /// codepoint becomes one byte (the Latin-1 carrier convention used throughout
    /// Brood's binary I/O): printable ASCII is itself, other bytes are `\xHH`. A
    /// codepoint > 255 is an error — use `\xHH`, or `string->utf8-bytes` for UTF-8 text.
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
                "malformed escape in bytes literal: \\x needs two hex digits",
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

    fn read_string(&mut self) -> Result<Value, LispError> {
        self.s.bump(); // opening quote
        let mut s = String::new();
        match self.s.scan_string_body(Some(&mut s)) {
            StringScan::Closed => Ok(self.heap.alloc_string(&s)),
            StringScan::Unterminated => Err(self.err_incomplete("unterminated string")),
            StringScan::BadEscape { at } => Err(self.err_at(
                self.s.pos_at(at),
                "malformed string escape: \\x needs two hex digits, \
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
        }
    }
}
