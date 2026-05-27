//! Error type used throughout the interpreter.
//!
//! Every fallible operation returns [`LispResult`]. Errors carry a coarse
//! [`ErrorKind`] (useful later for `try`/`catch` and for tooling) plus a
//! human-readable message.

use std::fmt;

use crate::core::value::Value;

/// A 1-based source position (line and column), used for editor-parseable
/// error reporting (see `docs/tooling.md`). Columns count characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The reader could not parse the source text.
    Parse,
    /// A symbol was referenced that has no binding.
    Unbound,
    /// A function or special form was called with the wrong number of args.
    Arity,
    /// A value had the wrong type for the operation.
    Type,
    /// A catch-all for runtime failures (overflow, division by zero, ...).
    Runtime,
    /// Raised by `(throw v)` from user code.
    User,
}

#[derive(Debug, Clone)]
pub struct LispError {
    pub kind: ErrorKind,
    pub message: String,
    /// The value carried by `(throw v)`, so `catch` can rebind it. Built-in
    /// errors leave this `None` (and `catch` then receives the message string).
    pub payload: Option<Value>,
    /// Source position, when known. Set by the reader (precise, for parse
    /// errors) or filled in by the file runner with the enclosing top-level
    /// form's start (for runtime errors). Drives `FILE:LINE:COL:` output.
    pub pos: Option<Pos>,
    /// The file the error occurred in, when known (set by `load` / the file
    /// runner). Combined with `pos` for `FILE:LINE:COL:` diagnostics.
    pub file: Option<String>,
}

impl LispError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        LispError {
            kind,
            message: message.into(),
            payload: None,
            pos: None,
            file: None,
        }
    }

    /// Attach a source position (builder style).
    pub fn with_pos(mut self, pos: Pos) -> Self {
        self.pos = Some(pos);
        self
    }

    /// Attach `pos` only if none is set yet — so a precise inner position
    /// (e.g. a parse error) is never overwritten by a coarser fallback.
    pub fn or_pos(mut self, pos: Pos) -> Self {
        if self.pos.is_none() {
            self.pos = Some(pos);
        }
        self
    }

    /// Attach a file only if none is set yet (the innermost `load` wins).
    pub fn or_file(mut self, file: impl Into<String>) -> Self {
        if self.file.is_none() {
            self.file = Some(file.into());
        }
        self
    }

    /// A one-line GNU diagnostic: `[FILE:][LINE:COL: ]kind error: message`, the
    /// form editors parse (see `docs/tooling.md`). Falls back gracefully when
    /// the file or position is unknown (e.g. at the REPL).
    pub fn located(&self) -> String {
        let prefix = match (&self.file, self.pos) {
            (Some(f), Some(p)) => format!("{}:{}:{}: ", f, p.line, p.col),
            (Some(f), None) => format!("{}: ", f),
            (None, Some(p)) => format!("{}:{}: ", p.line, p.col),
            (None, None) => String::new(),
        };
        format!("{}{}", prefix, self)
    }
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, message)
    }
    pub fn unbound(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unbound, message)
    }
    pub fn arity(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Arity, message)
    }
    pub fn type_err(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Type, message)
    }
    /// A self-identifying type error: which operation (`who`), what it `expected`,
    /// and the actual tag + printed form of what arrived. Threads the heap to
    /// render the offending value, e.g. `first: expected list or vector, got int (5)`.
    pub fn wrong_type(heap: &crate::core::heap::Heap, who: &str, expected: &str, got: Value) -> Self {
        Self::type_err(format!(
            "{}: expected {}, got {} ({})",
            who,
            expected,
            crate::core::value::tag(got).name(),
            crate::syntax::printer::print(heap, got),
        ))
    }
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Runtime, message)
    }
    /// Construct the error raised by `(throw value)`, carrying the value.
    pub fn thrown(value: Value, heap: &crate::core::heap::Heap) -> Self {
        LispError {
            kind: ErrorKind::User,
            message: crate::syntax::printer::display(heap, value),
            payload: Some(value),
            pos: None,
            file: None,
        }
    }
}

impl fmt::Display for LispError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::User => write!(f, "error: {}", self.message),
            ErrorKind::Parse => write!(f, "parse error: {}", self.message),
            ErrorKind::Unbound => write!(f, "unbound error: {}", self.message),
            ErrorKind::Arity => write!(f, "arity error: {}", self.message),
            ErrorKind::Type => write!(f, "type error: {}", self.message),
            ErrorKind::Runtime => write!(f, "runtime error: {}", self.message),
        }
    }
}

impl std::error::Error for LispError {}

/// The result of evaluating something: a [`Value`](crate::core::value::Value) or a [`LispError`].
pub type LispResult = Result<crate::core::value::Value, LispError>;
