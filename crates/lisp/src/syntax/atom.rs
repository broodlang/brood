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
    /// An integer-shaped token (digits only, optional leading sign) that won't
    /// fit in `i64`. The reader turns this into a `LispError::parse`; the CST
    /// records it as an `Error` node. Distinguishing it from `Float` is the
    /// point: `9223372036854775808` is not a `Float(9.22e18)` — that would
    /// silently lose precision against the user's intent.
    IntOverflow,
    /// A `:keyword` (the leading `:` is part of the token; strip it to intern).
    Keyword,
    Symbol,
    /// A Clojure-style **decimal** literal: a numeric token with a trailing `M`
    /// (or `m`) — `1.50M`, `0M`, `-3.14M`, `100M`. The reader strips the suffix
    /// from the token and parses the prefix as a `bigdecimal::BigDecimal`.
    /// Additive: a token like `1.5M` was never a valid number before.
    Decimal,
    /// A decimal-shaped token (trailing `M`/`m`) whose numeric prefix doesn't parse
    /// as a decimal — the reader turns it into a parse error, the CST an `Error` node
    /// (mirrors [`AtomKind::IntOverflow`]).
    DecimalInvalid,
}

/// Classify an atom token. No heap needed — atoms are numbers/keywords/symbols.
pub fn classify(token: &str) -> AtomKind {
    match token {
        "nil" => return AtomKind::Nil,
        "true" => return AtomKind::Bool(true),
        "false" => return AtomKind::Bool(false),
        _ => {}
    }
    // A Clojure-style decimal literal: a trailing `M`/`m` on a numeric-shaped
    // prefix (`1.50M`, `0M`, `-3.14M`, `100M`). Checked before everything else so
    // the `M` is never mistaken for a symbol char. Additive — these tokens were
    // never valid numbers. A bare `M` (no numeric prefix) stays a symbol.
    if token.len() > 1 && (token.ends_with('M') || token.ends_with('m')) {
        let prefix = &token[..token.len() - 1];
        let shape = numeric_shape(prefix);
        // The prefix must be a *complete* number (int- or float-shaped); a `+`/`-`
        // alone, or a trailing sign, isn't. `BigDecimal::parse` is the final say.
        if shape.numeric {
            if prefix.parse::<bigdecimal::BigDecimal>().is_ok() {
                return AtomKind::Decimal;
            }
            return AtomKind::DecimalInvalid;
        }
    }
    if let Ok(i) = token.parse::<i64>() {
        return AtomKind::Int(i);
    }
    // Classify the token's numeric shape in a single pass — whether it's
    // number-ish at all, and whether it has any fractional/exponent part — so we
    // don't re-walk it once per `looks_*` query.
    let shape = numeric_shape(token);
    if shape.numeric {
        // An integer-shaped token that didn't fit in `i64` is its own outcome —
        // *not* a Float fall-through (which would silently round e.g.
        // `9223372036854775808` to `9.22e18`). A user who wrote digits got a
        // diagnostic; a user who wrote `1e1000` still gets the `Float(inf)` path.
        if !shape.has_fraction_or_exp {
            return AtomKind::IntOverflow;
        }
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

/// Inter-form trivia whitespace: real whitespace, plus `,` (a comma is
/// whitespace in Brood). The single definition both parsers share so the
/// reader and the lossless CST can't disagree on where trivia runs — the
/// whitespace counterpart of [`is_delimiter`]. (Line comments start with `;`,
/// which both parsers handle separately because the CST keeps the comment as
/// its own node.)
pub fn is_trivia_ws(c: char) -> bool {
    c.is_whitespace() || c == ','
}

/// Characters that terminate an atom (and so can't appear unescaped inside one).
pub fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | '\'' | '`' | '~' | ','
        )
}

/// The numeric shape of a token, computed in one pass over its characters.
struct NumericShape {
    /// Passes the cheap pre-filter for `f64::parse` — starts with a digit, or a
    /// sign/dot followed by more, and holds only number-ish characters. Plain
    /// symbols like `-` or `...` are *not* numeric.
    numeric: bool,
    /// Has a `.`, `e`, or `E` — i.e. a fractional or exponent part, so it's
    /// float-shaped rather than integer-shaped. Only meaningful when `numeric`.
    has_fraction_or_exp: bool,
}

/// Classify a token's numeric shape in a single character walk. Replaces the
/// old `looks_numeric` + three `contains` scans (the former `looks_integer`),
/// which re-read the token up to four times.
fn numeric_shape(token: &str) -> NumericShape {
    let mut chars = token.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => {
            return NumericShape {
                numeric: false,
                has_fraction_or_exp: false,
            }
        }
    };
    // First char: a digit, or a sign/dot that leads a longer token.
    let first_ok = first.is_ascii_digit()
        || ((first == '-' || first == '+' || first == '.') && token.len() > 1);
    let mut numeric = first_ok;
    // A leading `.` is itself a fractional marker.
    let mut has_fraction_or_exp = first == '.';
    // The remaining chars must all be number-ish; note any fraction/exponent.
    for c in chars {
        match c {
            '0'..='9' | '-' | '+' => {}
            '.' | 'e' | 'E' => has_fraction_or_exp = true,
            _ => numeric = false,
        }
    }
    NumericShape {
        numeric,
        has_fraction_or_exp,
    }
}
