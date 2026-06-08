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
pub mod bundle; // single-binary app release: append-to-binary bundling (ADR-038)
pub mod cli_support; // tiny mechanism the `brood` and `nest` binaries share
pub mod dist; // distributed nodes: connect two runtimes over TCP, route messages
pub mod error; // errors + source positions (cross-cutting)
pub mod gui; // optional windowed display backend (feature "gui") — ADR-046 frontend #2
pub mod introspect; // tooling-facing queries on a live Interp (LSP today, MCP next)
pub mod net; // thin non-blocking TCP socket mechanism (ADR-062); policy lives in bundled std/net/* (ADR-097)
pub mod perf; // VM work-attribution counters (feature "perf-stats") — docs/benchmarking.md
pub mod process; // the green-process scheduler // the primitive kernel (Rust mechanism; policy lives in std/*.blsp)
pub mod text_width; // grapheme-cluster display-cell width (the `display-width` builtin + the GUI grid)
pub mod treesit; // optional tree-sitter parsing for foreign languages (feature "treesit") — ROADMAP §C

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
    // Record each prelude def's source location against a materialized, on-disk
    // copy of the prelude, so the LSP can jump `M-.` into the standard library
    // (the prelude is `include_str!`'d — there's no source file at runtime
    // otherwise). Best-effort and nav-only: if the cache can't be written we
    // simply set no file, `note_definition` no-ops, and stdlib goto stays
    // unavailable (everything else is unaffected). See `prelude_source_path`.
    let prelude_file = prelude_source_path();
    heap.set_current_file(prelude_file);
    // Positioned read so each def carries the line/col goto-definition lands on.
    let forms = syntax::reader::read_all_positioned(&mut heap, PRELUDE).expect("read prelude");
    for (form, pos) in forms {
        // Try the raw form first — catches `defn`/`defmacro` before lowering
        // discards their source positions. Then also try the expanded form so
        // user-defined def-like macros (e.g. `defseq`) whose raw head isn't
        // `def`/`defn`/`defmacro` but whose expansion IS a `defn` still get
        // their call-site position recorded. Both calls are no-ops when the
        // form isn't recognisably a definition, or no file is set.
        heap.note_definition(form, pos);
        // Compile pass (expand macros, then namespace-resolve — a no-op here since
        // the prelude is the root namespace), then evaluate. Form-by-form so a
        // macro defined by one form is visible to the next.
        let form = eval::macros::compile(&mut heap, form, root)
            .unwrap_or_else(|e| panic!("prelude expand: {}", e));
        heap.note_definition(form, pos);
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    heap.set_current_file(None);
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
        // A top-level run starts at the root namespace; the source's own `(ns …)`
        // sets it, with a forward-reference pre-scan (ADR-065). Restored after.
        let prev_ns = self.heap.set_compile_ns(None);
        let known = if eval::macros::file_opens_ns(&self.heap, &forms) {
            eval::macros::scan_def_names(&self.heap, &forms)
        } else {
            std::collections::HashSet::new()
        };
        let prev_known = self.heap.set_ns_known_names(known);
        let prev_imports = self.heap.set_imports(std::collections::HashMap::new());
        // The parsed forms sit in LOCAL below this checkpoint; each form's eval
        // allocates above it. Between top-level forms the eval stack is empty and
        // nothing in LOCAL is live but the (discarded) intermediate result —
        // globals live in PRELUDE/RUNTIME — so we reclaim that form's garbage
        // before the next. The final form's result is kept for the caller.
        let cp = self.heap.checkpoint();
        let gc = self.heap.gc_enabled();
        let mut result = Value::Nil;
        let n = forms.len();
        // GC-root the unevaluated forms across the per-form eval: at the
        // outermost-eval safepoint (`GC_BLOCK == 1`) the copying collector
        // relocates forms[i+1..] (LOCAL pairs the loop still needs). We re-fetch
        // each form from the (relocated) root stack via `root_at` — the `forms`
        // `Vec`'s own handles go stale across a collection. The
        // `roots_len`/`truncate_roots` pairing stays balanced on the error path.
        let roots_base = self.heap.roots_len();
        for &form in &forms {
            self.heap.push_root(form);
        }
        for i in 0..n {
            // The form's current handle (relocated if an earlier form's eval
            // triggered a collection); the `forms` Vec copy may be stale.
            let form = self.heap.root_at(roots_base + i);
            // Compile pass: expand macros once before evaluating (form-by-form,
            // so a macro a form defines is in scope for the forms after it).
            let outcome = eval::macros::compile(&mut self.heap, form, self.root)
                .and_then(|f| {
                    // BROOD_VM → the compiling engine (ADR-076); off → tree-walker.
                    // Stage 0 defers, so this is at parity. Mirrors `eval_source`.
                    if eval::compile::vm_enabled() {
                        eval::compile::run(&mut self.heap, f, self.root)
                    } else {
                        eval::eval(&mut self.heap, f, self.root)
                    }
                });
            match outcome {
                Ok(v) => result = v,
                Err(e) => {
                    self.heap.truncate_roots(roots_base);
                    self.heap.set_compile_ns(prev_ns);
                    self.heap.set_ns_known_names(prev_known);
                    self.heap.set_imports(prev_imports);
                    return Err(e);
                }
            }
            // Per-form arena reset is the *no-GC* reclamation path (ADR-016). With
            // the safepoint collector on, GC reclaims instead — and a copy moves
            // the slabs, so the pre-loop checkpoint is stale and a reset would
            // corrupt. Skip it when GC is enabled.
            if !gc && i + 1 < n {
                self.heap.reset_local_to(cp);
            }
        }
        self.heap.truncate_roots(roots_base);
        self.heap.set_compile_ns(prev_ns);
        self.heap.set_ns_known_names(prev_known);
        self.heap.set_imports(prev_imports);
        Ok(result)
    }

    /// Like [`eval_str`](Self::eval_str), but for source loaded from a named
    /// file: each top-level form is paired with its start position, so a parse
    /// or runtime error that lacks one is tagged with that form's `line:col`.
    /// The caller (the CLI) renders `PATH:LINE:COL: message` (see
    /// `docs/tooling.md`); parse errors keep the reader's precise position.
    pub fn eval_source(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = syntax::reader::read_all_positioned(&mut self.heap, src)?;
        // Root namespace + forward-ref pre-scan for a file run (ADR-065), restored
        // after. The file's own `(ns …)` form sets the namespace.
        let prev_ns = self.heap.set_compile_ns(None);
        let form_vals: Vec<Value> = forms.iter().map(|&(f, _)| f).collect();
        let known = if eval::macros::file_opens_ns(&self.heap, &form_vals) {
            eval::macros::scan_def_names(&self.heap, &form_vals)
        } else {
            std::collections::HashSet::new()
        };
        let prev_known = self.heap.set_ns_known_names(known);
        let prev_imports = self.heap.set_imports(std::collections::HashMap::new());
        let cp = self.heap.checkpoint();
        let gc = self.heap.gc_enabled();
        let mut result = Value::Nil;
        let n = forms.len();
        // GC-root the unevaluated forms across the loop (see `eval_str`); re-fetch
        // each form's relocated handle via `root_at` (positions are plain data in
        // `forms`, so they don't move).
        let roots_base = self.heap.roots_len();
        for &(form, _) in &forms {
            self.heap.push_root(form);
        }
        for i in 0..n {
            let form = self.heap.root_at(roots_base + i);
            let pos = forms[i].1;
            // Record def sites — try the raw form first (preserves pre-expansion
            // spans for `defn`/`defmacro`), then also the expanded form so
            // user-defined def-like macros whose raw head isn't recognised
            // (e.g. `defseq`) still get their call-site position recorded.
            // Both calls no-op when not a definition or no file is set.
            self.heap.note_definition(form, pos);
            let outcome = eval::macros::compile(&mut self.heap, form, self.root)
                .and_then(|f| {
                    self.heap.note_definition(f, pos);
                    // BROOD_VM routes the resolved form through the compiling
                    // engine (ADR-076); off by default → the tree-walker. Stage 0:
                    // the VM path defers every form back to `eval`, so this is at
                    // exact parity until lexical addressing lands (Stage 1).
                    if eval::compile::vm_enabled() {
                        eval::compile::run(&mut self.heap, f, self.root)
                    } else {
                        eval::eval(&mut self.heap, f, self.root)
                    }
                })
                .map_err(|e| e.or_pos(pos));
            match outcome {
                Ok(v) => result = v,
                Err(e) => {
                    self.heap.truncate_roots(roots_base);
                    self.heap.set_compile_ns(prev_ns);
                    self.heap.set_ns_known_names(prev_known);
                    self.heap.set_imports(prev_imports);
                    return Err(e);
                }
            }
            // See `eval_str`: per-form reset is the no-GC path; with the collector
            // on, GC reclaims and a move would invalidate the checkpoint.
            if !gc && i + 1 < n {
                self.heap.reset_local_to(cp);
            }
        }
        self.heap.truncate_roots(roots_base);
        self.heap.set_compile_ns(prev_ns);
        self.heap.set_ns_known_names(prev_known);
        self.heap.set_imports(prev_imports);
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

/// Materialize the embedded prelude to a stable, read-only-ish cache file and
/// return its path — the file the prelude's def-sites point at, so tools (the
/// LSP's `M-.`) can open the standard library's source. The prelude is
/// `include_str!`'d, so it has no source file at runtime; this writes one copy
/// to `$XDG_CACHE_HOME/brood/prelude.blsp` (falling back to `~/.cache`), only
/// when missing or stale (a new build ships a different prelude). Editing it
/// has no effect — it's a navigation artefact, not a load path.
///
/// Returns `None` if no cache dir can be determined or the write fails; the
/// caller treats that as "stdlib navigation unavailable" and carries on.
fn prelude_source_path() -> Option<String> {
    use std::path::PathBuf;
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let dir = base.join("brood");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("prelude.blsp");
    // Rewrite only when the on-disk copy is absent or differs from this build's
    // embedded prelude — keeps the file stable across runs and across versions.
    let stale = match std::fs::read(&path) {
        Ok(existing) => existing != PRELUDE.as_bytes(),
        Err(_) => true,
    };
    if stale {
        std::fs::write(&path, PRELUDE).ok()?;
    }
    Some(path.to_string_lossy().into_owned())
}
