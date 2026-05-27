//! mylisp — a small, dynamic Lisp built to (eventually) write a modern,
//! self-editing text editor.
//!
//! This crate is the language itself: the reader, evaluator, value model, and
//! builtins. The binary in `crates/cli` wraps it in a REPL.
//!
//! ## The 30-second tour
//!
//! ```
//! use mylisp::Interp;
//! let interp = Interp::new();
//! let result = interp.eval_str("(+ 1 2)").unwrap();
//! assert_eq!(result.to_string(), "3");
//! ```
//!
//! See `docs/` for the architecture, language reference, and roadmap.

pub mod builtins;
pub mod env;
pub mod error;
pub mod eval;
pub mod printer;
pub mod reader;
pub mod value;

use std::rc::Rc;

use env::Env;
use error::LispError;
use value::Value;

/// An interpreter instance: a global environment with builtins and the prelude
/// already loaded. Hold one and feed it source with [`Interp::eval_str`].
pub struct Interp {
    pub root: Rc<Env>,
}

impl Interp {
    pub fn new() -> Self {
        let root = Env::new_root();
        builtins::register(&root);
        let interp = Interp { root };
        // The prelude is bundled and exercised by the test suite, so a failure
        // here is a build-time bug, not a user error.
        interp
            .eval_str(PRELUDE)
            .unwrap_or_else(|e| panic!("failed to load prelude: {}", e));
        interp
    }

    /// Read every form in `src`, evaluate each in turn against the global
    /// environment, and return the value of the last one.
    pub fn eval_str(&self, src: &str) -> Result<Value, LispError> {
        let forms = reader::read_all(src)?;
        let mut result = Value::Nil;
        for form in forms {
            result = eval::eval(form, self.root.clone())?;
        }
        Ok(result)
    }
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

/// The standard prelude, written in mylisp and baked into the binary. Defines
/// the handful of helpers that are more natural in the language than in Rust.
const PRELUDE: &str = include_str!("../../../std/prelude.lisp");
