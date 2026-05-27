//! Brood — a small, dynamic Lisp built to (eventually) write a modern,
//! self-editing text editor.
//!
//! This crate is the language: reader, evaluator, value model, the per-process
//! [`Heap`](core::heap::Heap), and builtins. The binary in `crates/cli` wraps it in a
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

// The crate's module map, grouped by layer (see docs/components.md). The
// directory tree mirrors this — core/, syntax/, eval/, types/ — so the layout
// reads as the architecture.

pub mod core; // substrate: value, heap, alloc — what everything is addressed through
pub mod eval; // the tree-walking evaluator + its macro / compile pass
pub mod syntax; // surface: reader (text to Value) + printer (Value to text)
pub mod types; // the advisory type lattice + checker (nothing gates on it)

pub mod builtins;
pub mod dist; // distributed nodes: connect two runtimes over TCP, route messages
pub mod error; // errors + source positions (cross-cutting)
pub mod process; // the green-process scheduler // the primitive kernel (Rust mechanism; policy lives in std/*.blsp)

use std::sync::{Arc, LazyLock};

use core::heap::{Heap, RuntimeCode, SharedCode};
use core::value::{EnvId, Symbol, Value};
use error::LispError;

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
    let forms = syntax::reader::read_all(&mut heap, PRELUDE).expect("read prelude");
    for form in forms {
        // Expand macros once (the compile pass), then evaluate. Form-by-form so
        // a macro defined by one form is visible to the next.
        let form = eval::macros::macroexpand_all(&mut heap, form, root)
            .unwrap_or_else(|e| panic!("prelude expand: {}", e));
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    let (code, bindings) = heap.freeze_as_shared_code(root);
    SharedBundle {
        code: Arc::new(code),
        bindings,
    }
});

/// The byte-counting allocator (see [`core::alloc`]) backs the whole process, so
/// `(mem-bytes)` / `(mem-peak)` see every Rust allocation. Declared here in the
/// library so the CLI and the integration-test binaries all share one.
#[global_allocator]
static GLOBAL: core::alloc::Counting = core::alloc::Counting;

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
        Interp {
            heap,
            root: EnvId::GLOBAL,
        }
    }

    /// Read every form in `src`, evaluate each against the global environment,
    /// and return the value of the last.
    pub fn eval_str(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = syntax::reader::read_all(&mut self.heap, src)?;
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
            let form = eval::macros::macroexpand_all(&mut self.heap, form, self.root)?;
            result = eval::eval(&mut self.heap, form, self.root)?;
            if i + 1 < n {
                self.heap.reset_local_to(cp);
            }
        }
        Ok(result)
    }

    /// Like [`eval_str`](Self::eval_str), but for source loaded from a named
    /// file: each top-level form is paired with its start position, so a parse
    /// or runtime error that lacks one is tagged with that form's `line:col`.
    /// The caller (the CLI) renders `PATH:LINE:COL: message` (see
    /// `docs/tooling.md`); parse errors keep the reader's precise position.
    pub fn eval_source(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = syntax::reader::read_all_positioned(&mut self.heap, src)?;
        let cp = self.heap.checkpoint();
        let mut result = Value::Nil;
        let n = forms.len();
        for (i, (form, pos)) in forms.into_iter().enumerate() {
            // Record def sites pre-expansion (ADR-031); a no-op unless a file is
            // set via `current-file`. Survives the per-form arena reset below,
            // since def sites live in the (shared) RUNTIME region, not LOCAL.
            self.heap.note_definition(form, pos);
            let form = eval::macros::macroexpand_all(&mut self.heap, form, self.root)
                .map_err(|e| e.or_pos(pos))?;
            result = eval::eval(&mut self.heap, form, self.root).map_err(|e| e.or_pos(pos))?;
            if i + 1 < n {
                self.heap.reset_local_to(cp);
            }
        }
        Ok(result)
    }

    /// Render a value to its readable text form.
    pub fn print(&self, v: Value) -> String {
        syntax::printer::print(&self.heap, v)
    }
}

impl Default for Interp {
    fn default() -> Self {
        Self::new()
    }
}

/// The standard prelude, written in Brood and baked into the binary.
const PRELUDE: &str = include_str!("../../../std/prelude.blsp");
