//! Shared lexical rules for the two parsers in this layer: the evaluation
//! [`reader`](super::reader) (text → `Value`) and the tooling
//! [`cst`](super::cst) (text → lossless span tree). Both must agree on *what
//! counts as a token* — where atoms end, and whether an atom is a number, a
//! keyword, a boolean, `nil`, or a symbol. ADR-025 calls for one definition so
//! the two can't drift; this module is it.

/// The lexical class of an atom token, independent of the heap. The reader turns
/// this into a `Value` (interning symbols/keywords, parsing numbers); the CST
/// turns it into a `NodeKind`. The same token always classifies the same way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AtomKind {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// A `:keyword` (the leading `:` is part of the token; strip it to intern).
    Keyword,
    Symbol,
}

/// Classify an atom token. No heap needed — atoms are numbers/keywords/symbols.
pub fn classify(token: &str) -> AtomKind {
    match token {
        "nil" => return AtomKind::Nil,
        "true" => return AtomKind::Bool(true),
        "false" => return AtomKind::Bool(false),
        _ => {}
    }
    if let Ok(i) = token.parse::<i64>() {
        return AtomKind::Int(i);
    }
    if looks_numeric(token) {
        if let Ok(f) = token.parse::<f64>() {
            return AtomKind::Float(f);
        }
    }
    // A bare `:` is a symbol, not an empty keyword.
    if token.len() > 1 && token.starts_with(':') {
        return AtomKind::Keyword;
    }
    AtomKind::Symbol
}

/// Characters that terminate an atom (and so can't appear unescaped inside one).
pub fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | '\'' | '`' | '~' | ','
        )
}

/// A cheap pre-filter before `f64::parse`, so plain symbols like `-` or `...`
/// aren't misread as numbers: the token must start with a digit, or with a
/// sign/dot followed by more, and contain only number-ish characters.
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
