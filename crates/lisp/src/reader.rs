//! The reader: turns source text into [`Value`]s (the "read" of read-eval-print).
//!
//! A hand-written recursive-descent parser over a `Vec<char>`. It is small on
//! purpose; the grammar it accepts is documented in `docs/language.md`.

use std::rc::Rc;

use crate::error::LispError;
use crate::value::{self, Value};

/// Read every form in `src`.
pub fn read_all(src: &str) -> Result<Vec<Value>, LispError> {
    let mut parser = Parser::new(src);
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

/// Read exactly one form, ignoring any trailing input.
pub fn read_one(src: &str) -> Result<Value, LispError> {
    let mut parser = Parser::new(src);
    parser.skip_trivia();
    if parser.at_end() {
        return Err(LispError::parse("unexpected end of input"));
    }
    parser.read_form()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(src: &str) -> Self {
        Parser { chars: src.chars().collect(), pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
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

    /// Skip whitespace and `;` line comments. Commas count as whitespace
    /// (Clojure-style), which is why `~` is used for unquote rather than `,`.
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
        let c = self.peek().ok_or_else(|| LispError::parse("unexpected end of input"))?;
        match c {
            '(' => self.read_seq(')'),
            '[' => self.read_vector(),
            ')' | ']' | '}' => Err(LispError::parse(format!("unexpected '{}'", c))),
            '{' => Err(LispError::parse("map literals '{ }' are not supported yet")),
            '\'' => {
                self.pos += 1;
                let form = self.read_form()?;
                Ok(value::list(vec![value::sym("quote"), form]))
            }
            '`' => {
                self.pos += 1;
                let form = self.read_form()?;
                Ok(value::list(vec![value::sym("quasiquote"), form]))
            }
            '~' => {
                self.pos += 1;
                if self.peek() == Some('@') {
                    self.pos += 1;
                    let form = self.read_form()?;
                    Ok(value::list(vec![value::sym("unquote-splicing"), form]))
                } else {
                    let form = self.read_form()?;
                    Ok(value::list(vec![value::sym("unquote"), form]))
                }
            }
            '"' => self.read_string(),
            _ => self.read_atom(),
        }
    }

    fn read_seq(&mut self, close: char) -> Result<Value, LispError> {
        self.pos += 1; // consume the opening delimiter
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(LispError::parse("unclosed list")),
                Some(c) if c == close => {
                    self.pos += 1;
                    break;
                }
                Some(_) => items.push(self.read_form()?),
            }
        }
        Ok(value::list(items))
    }

    fn read_vector(&mut self) -> Result<Value, LispError> {
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(LispError::parse("unclosed vector")),
                Some(']') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => items.push(self.read_form()?),
            }
        }
        Ok(Value::Vector(Rc::new(items)))
    }

    fn read_string(&mut self) -> Result<Value, LispError> {
        self.pos += 1; // consume opening quote
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(LispError::parse("unterminated string")),
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('0') => s.push('\0'),
                    Some('\\') => s.push('\\'),
                    Some('"') => s.push('"'),
                    Some(other) => s.push(other),
                    None => return Err(LispError::parse("unterminated string escape")),
                },
                Some(c) => s.push(c),
            }
        }
        Ok(value::str_val(&s))
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
        || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | '\'' | '`' | '~' | ',')
}

/// Decide what an atom token means: keyword, number, special literal, or symbol.
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
    // Only treat as a float if it actually looks numeric, so that symbols like
    // `inf` or `nan` (which f64::from_str would accept) stay symbols.
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
    if !(first.is_ascii_digit() || ((first == '-' || first == '+' || first == '.') && token.len() > 1)) {
        return false;
    }
    token.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
}
