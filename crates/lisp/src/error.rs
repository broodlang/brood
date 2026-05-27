//! Error type used throughout the interpreter.
//!
//! Every fallible operation returns [`LispResult`]. Errors carry a coarse
//! [`ErrorKind`] (useful later for `try`/`catch` and for tooling) plus a
//! human-readable message.

use std::fmt;

use crate::value::Value;

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
}

impl LispError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        LispError { kind, message: message.into(), payload: None }
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
    pub fn wrong_type(
        heap: &crate::heap::Heap,
        who: &str,
        expected: &str,
        got: Value,
    ) -> Self {
        Self::type_err(format!(
            "{}: expected {}, got {} ({})",
            who,
            expected,
            crate::value::tag(got).name(),
            crate::printer::print(heap, got),
        ))
    }
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Runtime, message)
    }
    /// Construct the error raised by `(throw value)`, carrying the value.
    pub fn thrown(value: Value, heap: &crate::heap::Heap) -> Self {
        LispError {
            kind: ErrorKind::User,
            message: crate::printer::display(heap, value),
            payload: Some(value),
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

/// The result of evaluating something: a [`Value`](crate::value::Value) or a [`LispError`].
pub type LispResult = Result<crate::value::Value, LispError>;
