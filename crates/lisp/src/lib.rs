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

pub mod alloc;
pub mod builtins;
pub mod error;
pub mod eval;
pub mod heap;
pub mod macros;
pub mod printer;
pub mod process;
pub mod reader;
pub mod types;
pub mod value;

use std::sync::{Arc, LazyLock};

use error::LispError;
use heap::{Heap, RuntimeCode, SharedCode};
use value::{EnvId, Symbol, Value};

/// The shared code region (prelude closures, code data, builtins) plus the
/// global bindings to seed each process's global env. Built once, lazily.
struct SharedBundle {
    code: Arc<SharedCode>,
    bindings: Vec<(Symbol, Value)>,
}

static SHARED: LazyLock<SharedBundle> = LazyLock::new(|| {
    // Build the prelude + builtins in a throwaway builder heap, then relocate it
    // all into the shared region. Done once for the whole process.
    let mut heap = Heap::new();
    let root = heap.new_env(None);
    heap.set_global(root);
    builtins::register(&mut heap, root);
    let forms = reader::read_all(&mut heap, PRELUDE).expect("read prelude");
    for form in forms {
        // Expand macros once (the compile pass), then evaluate. Form-by-form so
        // a macro defined by one form is visible to the next.
        let form = macros::macroexpand_all(&mut heap, form, root)
            .unwrap_or_else(|e| panic!("prelude expand: {}", e));
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    let (code, bindings) = heap.freeze_as_shared_code(root);
    SharedBundle { code: Arc::new(code), bindings }
});

/// The byte-counting allocator (see [`alloc`]) backs the whole process, so
/// `(mem-bytes)` / `(mem-peak)` see every Rust allocation. Declared here in the
/// library so the CLI and the integration-test binaries all share one.
#[global_allocator]
static GLOBAL: alloc::Counting = alloc::Counting;

/// An interpreter instance: a heap and a global environment with builtins and
/// the prelude loaded.
pub struct Interp {
    pub heap: Heap,
    pub root: EnvId,
}

impl Interp {
    pub fn new() -> Self {
        // Share the immutable prelude; build this runtime a fresh, mutable code
        // region whose global table is seeded from the prelude bindings (no
        // prelude reload). Inner processes spawned from this runtime share that
        // region (see `process::spawn`), so a `def` reaches them — while
        // separate runtimes (nodes) stay independent, each with its own.
        let runtime = Arc::new(RuntimeCode::seeded(&SHARED.bindings));
        let mut heap = Heap::with_regions(Arc::clone(&SHARED.code), runtime);
        heap.set_global(EnvId::GLOBAL);
        Interp { heap, root: EnvId::GLOBAL }
    }

    /// Read every form in `src`, evaluate each against the global environment,
    /// and return the value of the last.
    pub fn eval_str(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = reader::read_all(&mut self.heap, src)?;
        // The parsed forms sit in LOCAL below this checkpoint; each form's eval
        // allocates above it. Between top-level forms the eval stack is empty and
        // nothing in LOCAL is live but the (discarded) intermediate result —
        // globals live in PRELUDE/RUNTIME — so we reclaim that form's garbage
        // before the next. The final form's result is kept for the caller.
        let cp = self.heap.checkpoint();
        let mut result = Value::Nil;
        let n = forms.len();
        for (i, form) in forms.into_iter().enumerate() {
            // Compile pass: expand macros once before evaluating (form-by-form,
            // so a macro a form defines is in scope for the forms after it).
            let form = macros::macroexpand_all(&mut self.heap, form, self.root)?;
            result = eval::eval(&mut self.heap, form, self.root)?;
            if i + 1 < n {
                self.heap.reset_local_to(cp);
            }
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
const PRELUDE: &str = include_str!("../../../std/prelude.blsp");
