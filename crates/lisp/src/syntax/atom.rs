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
    /// A float-shaped token (number-shaped, with a `.`/`e`/`E`) that does not parse
    /// as an `f64` — e.g. `1e`, `1.2.3`, `1e+`, `1.2e3.4`. The reader turns it into
    /// a parse error, the CST an `Error` node (mirrors [`AtomKind::IntOverflow`]).
    /// The point: a token whose *intent* is clearly numeric (it led with a digit /
    /// sign / dot and held only number characters) but is malformed should fail
    /// loudly, not silently read back as a symbol.
    FloatInvalid,
    /// A **digit-led** token that is not any number Brood has — `1/2`, `0x1F`,
    /// `1_000`, `1N`, `3px`. Reserved, not a symbol.
    ///
    /// This is the three variants above generalised. They each say "a token whose
    /// intent is numeric must not silently read back as a symbol", but they only
    /// caught tokens made *entirely* of number characters, so anything with a
    /// stray letter or punctuation leaked through to [`AtomKind::Symbol`] —
    /// `0x1F` became an identifier, and surfaced far away as "unbound symbol"
    /// rather than at the typo. The rule here is the same principle stated once,
    /// on the token's first character instead of all of them:
    ///
    /// > **A token that leads with a digit (or a sign/dot then a digit) must be a
    /// > number. If it is not one, it is a reader error — never a symbol.**
    ///
    /// Two things fall out. Diagnostics land at the mistake. And every numeric
    /// syntax Brood might ever want — ratios, radix literals, digit separators, a
    /// bigint suffix to match `1M` — stays **reservable after 1.0**, because none
    /// of those tokens is a legal name today, so adding one can never break a
    /// valid program. That is the whole reason to do this before 1.0: the reader
    /// is the one surface where staying silent is a permanent commitment.
    ///
    /// Deliberately unaffected, because they are not digit-led: `+`, `-`, `...`,
    /// `.foo`, `foo.`, `1M`/`1.0M` (decimals, classified above), `.5`, `5.`,
    /// `1e10`, and `inf`/`nan`/`-inf`.
    ReservedNumeric,
}

/// Does `token` lead like a number? True for a leading ASCII digit, and for a
/// `+`/`-`/`.` immediately followed by one. This is the whole of the digit-led
/// rule — see [`AtomKind::ReservedNumeric`].
///
/// The second clause is what keeps ordinary names working: `-`, `+`, `...`,
/// `.foo` and `..bar` all have a sign/dot with no digit behind it, so they are
/// not digit-led and stay symbols.
fn digit_led(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => true,
        Some('+') | Some('-') | Some('.') => chars.next().is_some_and(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// The teaching hint for a [`AtomKind::ReservedNumeric`] token, picked from its
/// shape. Lives here rather than in the reader so the CST and any other consumer
/// give the same explanation (the [`classify`] one-definition rule, ADR-025).
///
/// Each arm names the syntax the token *looks* like and says what Brood does
/// instead, in the style of the `#(…)`/`#'`/`#_` reader hints.
pub fn reserved_numeric_hint(token: &str) -> &'static str {
    let lower = token.to_ascii_lowercase();
    if token.contains('/') {
        // Note for a future ratio type: `/` is also the namespace separator, but
        // the two never collide — a digit-led token is a number, `mod/name` is not.
        return "Brood has no ratio type — `1/2` is reserved syntax, not a name. \
                Write the division `(/ 1 2)` for a float, or use an exact decimal \
                literal like `0.5M` (arbitrary-precision, no binary rounding).";
    }
    if lower.starts_with("0x") || lower.starts_with("0b") || lower.starts_with("0o") {
        return "Brood has no radix literals — `0x1F` / `0b1010` / `0o17` are reserved \
                syntax, not names. Parse at runtime with `(string->number \"1F\" 16)`, \
                or write the value in decimal.";
    }
    if token.contains('_') {
        return "Brood has no digit separators — `1_000` is reserved syntax, not a name. \
                Write the digits out: `1000`.";
    }
    if lower.ends_with('n') {
        return "Brood has no bigint suffix — `1N` is reserved syntax, not a name. \
                Integers widen to arbitrary precision on overflow already, so plain \
                `1` is enough; `1M` is the *decimal* literal.";
    }
    "A token that starts with a digit must be a number. Brood's numeric literals are \
     integers (`42`), floats (`1.5`, `1e10`, `.5`), and exact decimals (`1.50M`). If you \
     meant a name, start it with a letter — a digit-led token is reserved for numeric \
     syntax."
}

/// Classify an atom token. No heap needed — atoms are numbers/keywords/symbols.
pub fn classify(token: &str) -> AtomKind {
    match token {
        "nil" => return AtomKind::Nil,
        "true" => return AtomKind::Bool(true),
        "false" => return AtomKind::Bool(false),
        // Non-finite float literals — the exact inverse of `printer::format_float`,
        // which emits bare `inf`/`-inf`/`nan`. Without these the printed form of an
        // infinity/NaN (the language produces them by design: `1e400`, overflow)
        // read back as a *symbol*, so `(read (pr-str x))` silently changed a float
        // into a symbol. Reserved like `nil`/`true`/`false` (so `inf`/`nan` are not
        // identifiers — the round-trip win outweighs losing three rare names).
        "inf" => return AtomKind::Float(f64::INFINITY),
        "-inf" => return AtomKind::Float(f64::NEG_INFINITY),
        "nan" => return AtomKind::Float(f64::NAN),
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
        // Float-shaped (number intent + a `.`/`e`/`E`) but unparseable — `1e`,
        // `1.2.3`, `1e+`. A malformed number literal, NOT a symbol: fail loudly.
        return AtomKind::FloatInvalid;
    }
    // A bare `:` is a symbol, not an empty keyword.
    if token.len() > 1 && token.starts_with(':') {
        return AtomKind::Keyword;
    }
    // Digit-led but none of the number forms above matched: reserved, not a symbol.
    // Last, so every real literal — including `1M`, `.5`, `1e10` and the malformed
    // ones that earn their own diagnostic — is classified first.
    if digit_led(token) {
        return AtomKind::ReservedNumeric;
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
    /// Passes the cheap pre-filter for `f64::parse` — genuine numeric intent: it
    /// leads with a digit / sign / dot, contains **at least one digit**, holds only
    /// number-ish characters, and every `+`/`-` sits in a valid sign position
    /// (leading, or right after an `e`/`E`). Conventional identifiers like `-`,
    /// `++`, `--`, `...`, `1+`, `2+3` are *not* numeric — they read as symbols; only
    /// a real (possibly-malformed) number like `1e`/`1.2.3`/`1e+` is `numeric`.
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
    // Genuine numeric intent needs a digit somewhere — else `++`/`--`/`...`/`.e`/`+.`
    // are symbols, not malformed numbers (the leading sign/dot alone doesn't make a
    // number). And a `+`/`-` past the first char is only a number character right
    // after an exponent marker (`1e+5`); anywhere else (`1+`, `2+3`, `1-`) it means
    // the token is a symbol. `prev` tracks the preceding char to enforce that.
    let mut has_digit = first.is_ascii_digit();
    let mut prev = first;
    for c in chars {
        match c {
            '0'..='9' => has_digit = true,
            '+' | '-' => {
                if prev != 'e' && prev != 'E' {
                    numeric = false;
                }
            }
            '.' | 'e' | 'E' => has_fraction_or_exp = true,
            _ => numeric = false,
        }
        prev = c;
    }
    NumericShape {
        numeric: numeric && has_digit,
        has_fraction_or_exp,
    }
}
