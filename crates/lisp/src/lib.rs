//! Brood — a small, dynamic Lisp built to (eventually) write a modern,
//! self-editing text editor.
//!
//! This crate is the language: reader, evaluator, value model, the per-process
//! [`Heap`](heap::Heap), and builtins. The binary in `crates/cli` wraps it in a
//! REPL.
//!
//! ```
//! use brood::Interp;
//! let mut interp = Interp::new();
//! let result = interp.eval_str("(+ 1 2)").unwrap();
//! assert_eq!(interp.print(result), "3");
//! ```
//!
//! See `docs/` for the architecture, language reference, and roadmaps.

pub mod builtins;
pub mod error;
pub mod eval;
pub mod heap;
pub mod macros;
pub mod printer;
pub mod process;
pub mod reader;
pub mod value;

use error::LispError;
use heap::Heap;
use value::{EnvId, Value};

/// An interpreter instance: a heap and a global environment with builtins and
/// the prelude loaded.
pub struct Interp {
    pub heap: Heap,
    pub root: EnvId,
}

impl Interp {
    pub fn new() -> Self {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        builtins::register(&mut heap, root);
        let mut interp = Interp { heap, root };
        interp
            .eval_str(PRELUDE)
            .unwrap_or_else(|e| panic!("failed to load prelude: {}", e));
        interp
    }

    /// Read every form in `src`, evaluate each against the global environment,
    /// and return the value of the last.
    pub fn eval_str(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = reader::read_all(&mut self.heap, src)?;
        let mut result = Value::Nil;
        for form in forms {
            result = eval::eval(&mut self.heap, form, self.root)?;
        }
        Ok(result)
    }

    /// Render a value to its readable text form.
    pub fn print(&self, v: Value) -> String {
        printer::print(&self.heap, v)
    }
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

/// The standard prelude, written in Brood and baked into the binary.
const PRELUDE: &str = include_str!("../../../std/prelude.lisp");
