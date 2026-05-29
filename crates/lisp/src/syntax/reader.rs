//! The reader: turns source text into [`Value`]s. It allocates pairs/vectors/
//! strings, so it threads `&mut Heap`.
//!
//! The character stream + trivia/atom/string-body primitives live in
//! [`scanner`](super::scanner) and are shared with the CST. This module
//! handles the *structural* parsing — open/close delimiters, dotted pairs,
//! map literals, quote sigils — and the building of `Value`s through the
//! heap. Lexical rules (where an atom ends, how to classify a token)
//! continue to share [`atom`](super::atom).

use crate::core::heap::Heap;
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

/// Read exactly one form, ignoring any trailing input.
pub fn read_one(heap: &mut Heap, src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.s.skip_trivia();
    if parser.s.at_end() {
        return Err(parser.err("unexpected end of input"));
    }
    parser.read_form()
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

    fn read_form(&mut self) -> Result<Value, LispError> {
        self.s.skip_trivia();
        let c = self
            .s
            .peek()
            .ok_or_else(|| self.err("unexpected end of input"))?;
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
                let v = self.read_wrapped("quote");
                self.exit();
                v
            }
            '`' => {
                self.enter()?;
                let v = self.read_wrapped("quasiquote");
                self.exit();
                v
            }
            '~' => {
                self.enter()?;
                self.s.bump(); // '~'
                let v = if self.s.peek() == Some('@') {
                    self.s.bump();
                    let form = self.read_form()?;
                    Ok(self.wrap("unquote-splicing", form))
                } else {
                    let form = self.read_form()?;
                    Ok(self.wrap("unquote", form))
                };
                self.exit();
                v
            }
            '"' => self.read_string(),
            _ => self.read_atom(),
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
        let mut tail = Value::Nil;
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at(start, "unclosed list (opened here)")),
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
                        None => return Err(self.err("unclosed list")),
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening '['
        self.s.bump(); // '['
        let mut items = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at(start, "unclosed vector (opened here)")),
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
        let start = self.s.pos_at(self.s.pos()); // position of the opening '{'
        self.s.bump(); // '{'
        let mut pairs = Vec::new();
        loop {
            self.s.skip_trivia();
            match self.s.peek() {
                None => return Err(self.err_at(start, "unclosed map (opened here)")),
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
            StringScan::Unterminated => Err(self.err("unterminated string")),
        }
    }

    fn read_atom(&mut self) -> Result<Value, LispError> {
        let token_start = self.s.pos();
        let token = self.s.read_atom();
        match atom::classify(token) {
            AtomKind::Nil => Ok(Value::Nil),
            AtomKind::Bool(b) => Ok(Value::Bool(b)),
            AtomKind::Int(i) => Ok(Value::Int(i)),
            AtomKind::Float(f) => Ok(Value::Float(f)),
            // `atom::classify` only returns `Keyword` for a non-empty `:`-prefixed
            // token, so dropping the `:` always leaves a non-empty name.
            AtomKind::Keyword => Ok(value::kw(&token[1..])),
            AtomKind::Symbol => Ok(value::sym(token)),
            AtomKind::IntOverflow => Err(self.err_at(
                self.s.pos_at(token_start),
                format!("integer literal out of range for i64: {}", token),
            )),
        }
    }
}
