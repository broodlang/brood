//! The reader: turns source text into [`Value`]s. It allocates pairs/vectors/
//! strings, so it threads `&mut Heap`.

use crate::error::{LispError, Pos};
use crate::heap::Heap;
use crate::value::{self, Value};

/// Read every form in `src`.
pub fn read_all(heap: &mut Heap, src: &str) -> Result<Vec<Value>, LispError> {
    let mut parser = Parser::new(heap, src);
    let mut forms = Vec::new();
    loop {
        parser.skip_trivia();
        if parser.at_end() {
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
        parser.skip_trivia();
        if parser.at_end() {
            break;
        }
        let start = parser.pos_at(parser.pos);
        let form = parser.read_form()?;
        forms.push((form, start));
    }
    Ok(forms)
}

/// Read exactly one form, ignoring any trailing input.
pub fn read_one(heap: &mut Heap, src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(heap, src);
    parser.skip_trivia();
    if parser.at_end() {
        return Err(parser.err("unexpected end of input"));
    }
    parser.read_form()
}

struct Parser<'a> {
    heap: &'a mut Heap,
    chars: Vec<char>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(heap: &'a mut Heap, src: &str) -> Self {
        Parser {
            heap,
            chars: src.chars().collect(),
            pos: 0,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    /// The 1-based line/column of character index `idx`. Computed by scanning
    /// from the start; only called on top-level form starts and parse errors,
    /// so the cost is irrelevant.
    fn pos_at(&self, idx: usize) -> Pos {
        let mut line = 1u32;
        let mut col = 1u32;
        for &c in &self.chars[..idx.min(self.chars.len())] {
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        Pos { line, col }
    }

    /// A parse error tagged with the current position.
    fn err(&self, msg: impl Into<String>) -> LispError {
        LispError::parse(msg).with_pos(self.pos_at(self.pos))
    }

    /// A parse error tagged with a specific position (e.g. where a delimiter
    /// opened, which is more useful for "unclosed" than the EOF position).
    fn err_at(&self, pos: Pos, msg: impl Into<String>) -> LispError {
        LispError::parse(msg).with_pos(pos)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    /// Skip whitespace (commas count as whitespace) and `;` line comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() || c == ',' => {
                    self.pos += 1;
                }
                Some(';') => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn read_form(&mut self) -> Result<Value, LispError> {
        self.skip_trivia();
        let c = self
            .peek()
            .ok_or_else(|| self.err("unexpected end of input"))?;
        match c {
            '(' => self.read_seq(')'),
            '[' => self.read_vector(),
            ')' | ']' | '}' => Err(self.err(format!("unexpected '{}'", c))),
            '{' => Err(self.err("map literals '{ }' are not supported yet")),
            '\'' => self.read_wrapped("quote"),
            '`' => self.read_wrapped("quasiquote"),
            '~' => {
                self.pos += 1;
                if self.peek() == Some('@') {
                    self.pos += 1;
                    let form = self.read_form()?;
                    Ok(self.wrap("unquote-splicing", form))
                } else {
                    let form = self.read_form()?;
                    Ok(self.wrap("unquote", form))
                }
            }
            '"' => self.read_string(),
            _ => self.read_atom(),
        }
    }

    /// Read `<form>` and wrap it as `(tag form)`.
    fn read_wrapped(&mut self, tag: &str) -> Result<Value, LispError> {
        self.pos += 1;
        let form = self.read_form()?;
        Ok(self.wrap(tag, form))
    }

    fn wrap(&mut self, tag: &str, form: Value) -> Value {
        self.heap.list(vec![value::sym(tag), form])
    }

    fn read_seq(&mut self, close: char) -> Result<Value, LispError> {
        let start = self.pos_at(self.pos); // position of the opening delimiter
        self.pos += 1; // opening delimiter
        let mut items = Vec::new();
        let mut tail = Value::Nil;
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.err_at(start, "unclosed list (opened here)")),
                Some(c) if c == close => {
                    self.pos += 1;
                    break;
                }
                // A lone `.` introduces an improper (dotted) tail: `(a . b)`.
                Some('.') if self.is_dot_separator() => {
                    if items.is_empty() {
                        return Err(self.err("dotted list needs an element before '.'"));
                    }
                    self.pos += 1; // the '.'
                    self.skip_trivia();
                    match self.peek() {
                        None => return Err(self.err("unclosed list")),
                        Some(c) if c == close => {
                            return Err(self.err("expected a form after '.' in dotted list"))
                        }
                        Some(_) => tail = self.read_form()?,
                    }
                    self.skip_trivia();
                    match self.peek() {
                        Some(c) if c == close => {
                            self.pos += 1;
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

    /// Is the `.` at the cursor a lone dotted-pair separator (followed by a
    /// delimiter or end), rather than the start of an atom like `.5` or `.foo`?
    fn is_dot_separator(&self) -> bool {
        self.chars
            .get(self.pos + 1)
            .is_none_or(|&c| is_delimiter(c))
    }

    fn read_vector(&mut self) -> Result<Value, LispError> {
        let start = self.pos_at(self.pos); // position of the opening '['
        self.pos += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.err_at(start, "unclosed vector (opened here)")),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => items.push(self.read_form()?),
            }
        }
        Ok(self.heap.alloc_vector(items))
    }

    fn read_string(&mut self) -> Result<Value, LispError> {
        self.pos += 1; // opening quote
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(self.err("unterminated string")),
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('e') => s.push('\u{1b}'), // ESC — for ANSI terminal control
                    Some('0') => s.push('\0'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some(other) => s.push(other),
                    None => return Err(self.err("unterminated string escape")),
                },
                Some(c) => s.push(c),
            }
        }
        Ok(self.heap.alloc_string(&s))
    }

    fn read_atom(&mut self) -> Result<Value, LispError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_delimiter(c) {
                break;
            }
            self.pos += 1;
        }
        let token: String = self.chars[start..self.pos].iter().collect();
        Ok(classify(&token))
    }
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | '\'' | '`' | '~' | ','
        )
}

/// Classify an atom token (no heap needed — atoms are numbers/symbols/keywords).
fn classify(token: &str) -> Value {
    match token {
        "nil" => return Value::Nil,
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = token.parse::<i64>() {
        return Value::Int(i);
    }
    if looks_numeric(token) {
        if let Ok(f) = token.parse::<f64>() {
            return Value::Float(f);
        }
    }
    if let Some(rest) = token.strip_prefix(':') {
        if !rest.is_empty() {
            return value::kw(rest);
        }
    }
    value::sym(token)
}

fn looks_numeric(token: &str) -> bool {
    let mut chars = token.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !(first.is_ascii_digit()
        || ((first == '-' || first == '+' || first == '.') && token.len() > 1))
    {
        return false;
    }
    token
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
}
