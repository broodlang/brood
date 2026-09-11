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

// Two clippy style lints we deliberately accept crate-wide: `too_many_arguments`
// (the evaluator/codegen/render hot paths legitimately thread 8 params — bundling
// them into a struct adds indirection on the very paths we keep flat for speed),
// and `type_complexity` (the kernel's interner/cache/fn-signature types are
// irreducibly nested; a `type` alias just moves the complexity, it doesn't remove
// it).
#![allow(clippy::too_many_arguments, clippy::type_complexity)]
// Two purely-cosmetic doc-style lints we accept rather than churn every affected
// doc comment: `empty_line_after_doc_comments` and `doc_lazy_continuation` (a
// paragraph-after-a-list rendering nit). They don't change the generated docs'
// meaning — fix opportunistically, don't gate on them. Everything else is fixed,
// so `make clippy` runs `-D warnings`.
#![allow(clippy::empty_line_after_doc_comments, clippy::doc_lazy_continuation)]
// A handful of pure-style lints we don't gate on: in the kernel's hot/index-based
// loops and builder code the lint-preferred form is often *less* clear, and several
// sites sit in code under active change. The fatal `-D warnings` gate still catches
// every correctness / perf / suspicious / complexity regression — the lints that
// matter. Tidy these opportunistically.
#![allow(
    clippy::needless_range_loop,
    clippy::manual_range_contains,
    clippy::field_reassign_with_default,
    clippy::while_let_loop,
    clippy::collapsible_match,
    clippy::suspicious_else_formatting
)]

// The crate's module map, grouped by layer (see docs/components.md). The
// directory tree mirrors this — core/, syntax/, eval/, types/ — so the layout
// reads as the architecture.

/// This runtime's semantic version — the same string `(system/brood-version)` returns.
///
/// Exported because `env!("CARGO_PKG_VERSION")` read from a DEPENDENT crate yields that
/// crate's version, not brood's: the playground's `version()` advertised itself as the
/// Brood build it runs and reported `0.1.0`, the wasm shim's own number.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod core; // substrate: value, heap, alloc — what everything is addressed through
pub mod eval; // the tree-walking evaluator + its macro / compile pass
pub mod syntax; // surface: reader (text to Value) + printer (Value to text)
pub mod types; // the advisory type lattice + checker (nothing gates on it)

pub mod boot; // how a runtime comes to hold the prelude: image / text cache / source (ADR-138/314)
pub mod builtins; // the primitive kernel (Rust mechanism; policy lives in std/*.blsp)
pub mod bundle; // single-binary app release: append-to-binary bundling (ADR-038)
pub mod cli_support; // tiny mechanism the `brood` and `nest` binaries share
pub mod diagnostics; // observability: perf counters, profiler, coverage, the BROOD_* flag catalogue
pub mod dist; // distributed nodes: connect two runtimes over TCP, route messages
pub mod error; // errors + source positions (cross-cutting)
pub mod host; // feature-gated machine bindings: gui, audio, net, subprocess, wasm, treesit, text_width
pub mod introspect; // tooling-facing queries on a live Interp (LSP today, MCP next)
#[cfg(feature = "jit")]
pub mod jit; // tier-1 template JIT via Cranelift (feature "jit") — ADR-101, docs/value-repr.md
pub mod process; // the green-process scheduler
pub mod renames; // the rename ledger: where a deliberately renamed public name went (ADR-304)

use std::sync::Arc;

use boot::SHARED;
use core::heap::{Heap, RuntimeCode};
use core::value::{EnvId, Value};
use error::LispError;

/// The byte-counting allocator (see [`core::alloc`]) backs the whole process, so
/// `(%)` / `(%)` see every Rust allocation. Declared here in the
/// library so the CLI and the integration-test binaries all share one.
#[global_allocator]
static GLOBAL: core::alloc::Counting = core::alloc::Counting;

/// An interpreter instance: a heap and a global environment with builtins and
/// the prelude loaded.
pub struct Interp {
    pub heap: Heap,
    pub root: EnvId,
}

impl Drop for Interp {
    /// The embedded-host teardown: reap every permanently-parked green process
    /// of THIS runtime (a `(receive)` nothing will ever send to holds its
    /// whole process + heap in the mailbox waiter slot — the long-flagged
    /// leak; see `shutdown_runtime_parked`). The standalone binaries exit the
    /// OS process right after, so this is effectively free there; a long-lived
    /// host that creates and drops `Interp`s no longer accumulates them.
    fn drop(&mut self) {
        process::shutdown_runtime_parked(&self.heap.runtime_arc());
        // …then retire THIS THREAD's root context, if it minted one.
        //
        // `ensure_ctx` caches the root `Ctx` in a thread-local keyed to the THREAD, not the
        // runtime, and nothing cleared it: a second `Interp` on the same thread inherited the
        // first's pid *and its mailbox*. Six sequential `Interp`s all report
        // `#<pid nonode/1>`. That is not merely untidy — the inherited mailbox keeps whatever
        // the previous runtime left queued, and a `Payload::Local { slot, .. }` is an index
        // into the heap of the runtime that took delivery. Popping one after the swap reads
        // the NEW runtime's `msg_roots` at the OLD runtime's index: a wrong-heap read, and
        // silent, because the slot is in range far more often than not.
        //
        // Retiring here also gives the root ctx the death path a green process gets — its
        // monitors and links fire — rather than leaving it registered until the OS thread
        // ends, which for a host thread that outlives many `Interp`s is never.
        //
        // A no-op (returns false) on a thread that never touched `self`/`send`/`receive`,
        // which is every thread that only ever built and evaluated.
        // …only if it is OURS. `deregister_root_ctx` takes whatever context the thread
        // holds; a host with a long-lived `Interp` that builds a short-lived one on the same
        // thread would otherwise, on dropping the temporary, retire the long-lived
        // interpreter's context — changing its pid, discarding its queued mailbox, and
        // firing its monitors and links as a death.
        process::deregister_root_ctx_of(self.heap.runtime_tag());
    }
}

impl Interp {
    pub fn new() -> Self {
        // Share the immutable prelude; build this runtime a fresh, mutable code
        // region whose global table is seeded from the prelude bindings (no
        // prelude reload). Inner processes spawned from this runtime share that
        // region (see `process::spawn`), so a `def` reaches them — while
        // separate runtimes (nodes) stay independent, each with its own.
        let runtime = Arc::new(RuntimeCode::seeded(
            &SHARED.bindings,
            &SHARED.private,
            &SHARED.meta,
        ));
        let mut heap = Heap::with_regions(Arc::clone(&SHARED.code), runtime);
        heap.set_global(EnvId::GLOBAL);
        // Abilities + the Display protocol are core — defined in the shared prelude
        // (`*show*` is wired on there), so nothing to load per runtime here.
        Interp {
            heap,
            root: EnvId::GLOBAL,
        }
    }

    /// Run a whole top-level program (`brood file.blsp`) as a single green process
    /// (ADR-135), blocking this (root) thread until it finishes. Unlike [`eval_source`],
    /// which runs the forms on the root thread — where a top-level `receive` blocks the
    /// OS thread and every message to a spawned worker crosses a thread boundary — the
    /// program runs on a worker in capture mode, so it uses the userspace direct-handoff
    /// path and its top-level `receive`s park-and-capture. `file` tags errors with a
    /// path. Returns the structured error if a top-level form raised (file/pos attached,
    /// payload stripped at the process boundary) so the caller can render the full
    /// report — caret, hint, call trace.
    pub fn run_program(&mut self, src: &str, file: Option<String>) -> Result<(), LispError> {
        let exit = process::spawn_root_program(&self.heap, src, file, None)?;
        exit.wait()
    }

    /// [`run_program`](Self::run_program) with `preamble` evaluated first, inside the
    /// program's own process. The `brood file` entry point passes
    /// `(%crash-report-arm-default)` here (ADR-305/309): armed in the program's process the
    /// reporter knows the program's pid and leaves its crash to the CLI's report.
    pub fn run_program_with_preamble(
        &mut self,
        preamble: &str,
        src: &str,
        file: Option<String>,
    ) -> Result<(), LispError> {
        let exit = process::spawn_root_program(&self.heap, src, file, Some(preamble))?;
        exit.wait()
    }

    /// Run a top-level program as a green process and return its **printed** result (wasm).
    /// `run_program` discards the value (a handle into the program's heap, which dies at
    /// exit); this captures the last form's rendered form across that boundary so the
    /// in-browser playground can display it. On wasm `exit.wait()` drives the cooperative
    /// single-thread scheduler, so `spawn`/`send`/`receive` run with no OS threads.
    ///
    /// NOT `#[cfg(wasm32)]`, though only wasm calls it: gated, it was invisible to every host
    /// test, so the capture guard beside it could only assert `run_program` — the neighbour —
    /// while the playground's actual entry point went unchecked.
    pub fn run_program_repr(&mut self, src: &str) -> Result<String, LispError> {
        let exit = process::spawn_root_program(&self.heap, src, None, None)?;
        exit.wait()?;
        Ok(exit.take_result().unwrap_or_default())
    }

    /// Test-only: spawn a top-level program as a green process and return immediately,
    /// WITHOUT waiting for it. Lets a test drive the quanta itself on the calling thread —
    /// the shape wasm runs in, where `wait` pumps the run queue on the caller rather than
    /// parking it. `run_program_repr` would deadlock here, since with workers disabled
    /// nothing would ever publish the exit.
    #[doc(hidden)]
    pub fn spawn_program_for_test(&mut self, src: &str) -> Result<(), LispError> {
        process::spawn_root_program(&self.heap, src, None, None)?;
        Ok(())
    }

    /// Read every form in `src`, evaluate each against the global environment,
    /// and return the value of the last.
    pub fn eval_str(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = syntax::reader::read_all(&mut self.heap, src)?;
        self.eval_forms(forms.into_iter().map(|f| (f, None)).collect())
    }

    /// Like [`eval_str`](Self::eval_str), but for source loaded from a named
    /// file: each top-level form is paired with its start position, so a parse
    /// or runtime error that lacks one is tagged with that form's `line:col`.
    /// The caller (the CLI) renders `PATH:LINE:COL: message` (see
    /// `docs/tooling.md`); parse errors keep the reader's precise position.
    pub fn eval_source(&mut self, src: &str) -> Result<Value, LispError> {
        let forms = syntax::reader::read_all_positioned(&mut self.heap, src)?;
        self.eval_forms(forms.into_iter().map(|(f, p)| (f, Some(p))).collect())
    }

    /// Shared top-level driver behind [`eval_str`](Self::eval_str) (no positions)
    /// and [`eval_source`](Self::eval_source) (each form tagged with its
    /// `line:col`, so an otherwise-unpositioned error gets `PATH:LINE:COL` and
    /// def sites are recorded for `M-.`). Evaluates each form against the global
    /// environment and returns the last value.
    ///
    /// Namespace + forward-reference pre-scan (ADR-065): a top-level run starts at
    /// the root namespace; the source's own `(ns …)` sets it, restored after.
    ///
    /// GC-rooting (load-bearing): the parsed forms sit in LOCAL and each form's
    /// eval allocates above a checkpoint. At the outermost-eval safepoint
    /// (`GC_BLOCK == 1`) the copying collector relocates the still-unevaluated
    /// forms, so we root them and re-fetch each via `root_at` — the `forms` Vec's
    /// own handles go stale across a collection; positions are plain data and
    /// don't move. Between forms the eval stack is empty and only the discarded
    /// intermediate result is live (globals live in PRELUDE/RUNTIME), so that
    /// garbage is reclaimed before the next form: by GC when the collector is on,
    /// else by the ADR-016 per-form arena reset (a move would invalidate the
    /// checkpoint, so the reset is skipped whenever GC is enabled). The
    /// `roots_len`/`truncate_roots` pairing and the namespace restore both run
    /// exactly once, on every path.
    fn eval_forms(
        &mut self,
        forms: Vec<(Value, Option<crate::error::Pos>)>,
    ) -> Result<Value, LispError> {
        // Scope this runtime as the owner of any ROOT context minted while these forms run.
        // A root context is minted lazily, the first time the root thread touches
        // `self`/`send`/`receive`, and `ensure_ctx` has no heap to read the runtime from —
        // so this is the one place that can tell it whose it is. `Interp::drop` then retires
        // only a context stamped with its own tag (`deregister_root_ctx_of`).
        let tag = self.heap.runtime_tag();
        process::with_minting_runtime_tag(tag, || self.eval_forms_inner(forms))
    }

    fn eval_forms_inner(
        &mut self,
        forms: Vec<(Value, Option<crate::error::Pos>)>,
    ) -> Result<Value, LispError> {
        let form_vals: Vec<Value> = forms.iter().map(|&(f, _)| f).collect();
        let root = self.root;
        // `NsLoadScope` resets compile-ns + imports + this file's forward-ref pre-scan
        // + assume-own into the file's own namespace scope, and restores the caller's
        // ns-state on EVERY exit path incl. a panic (ADR-065). It owns the heap for the
        // run; reach it via `scope.heap()`.
        let mut scope = eval::macros::NsLoadScope::enter(&mut self.heap, &form_vals);
        let heap = scope.heap();
        let cp = heap.checkpoint();
        let gc = heap.gc_enabled();
        let mut result = Value::nil();
        let n = forms.len();
        let roots_base = heap.roots_len();
        for &(form, _) in &forms {
            heap.push_root(form);
        }
        let mut ret: Result<(), LispError> = Ok(());
        for i in 0..n {
            // The form's current handle (relocated if an earlier form's eval
            // triggered a collection); the `forms` Vec copy may be stale.
            let form = heap.root_at(roots_base + i);
            let pos = forms[i].1;
            // Record def sites (file runs only): the raw form first (preserves
            // pre-expansion spans for `defn`/`defmacro`), then the expanded form
            // so def-like macros whose raw head isn't recognised (e.g. `defseq`)
            // still get their call-site position. Both no-op off a definition or
            // with no file set.
            if let Some(pos) = pos {
                heap.note_definition(form, pos);
            }
            // Compile pass: expand macros once before evaluating (form-by-form, so
            // a macro a form defines is in scope for the forms after it), then
            // route through the compiling VM (ADR-076) or, under BROOD_VM=0, the
            // tree-walker (Stage 0 defers, so the two are at parity).
            let outcome = eval::macros::compile(heap, form, root)
                .and_then(|f| {
                    if let Some(pos) = pos {
                        heap.note_definition(f, pos);
                    }
                    eval::compile::run_top_form(heap, f, root)
                })
                .map_err(|e| match pos {
                    Some(p) => e.or_pos(p),
                    None => e,
                });
            match outcome {
                Ok(v) => result = v,
                Err(e) => {
                    ret = Err(e);
                    break;
                }
            }
            // NO per-form arena reset (KI-12). ADR-016's reset was the no-GC
            // reclamation path, on the premise quoted above — "globals live in
            // PRELUDE/RUNTIME, so the only live thing between forms is the
            // discarded result". That premise is false in the one heap where the
            // reset actually ran: a **builder** heap (`Heap::new` sets
            // `gc_enabled = false`), where a prelude `def` binds a value that is
            // still LOCAL — it only becomes PRELUDE at `freeze_as_shared_code`.
            // So `(def *load-path* (list "."))` stored a LOCAL pair, the next
            // form's reset truncated the slabs back below it, and a later
            // allocation reused those indices: the global's car then aliased
            // whatever came next — a docstring, a symbol, layout-dependent. It
            // silently corrupted the default `*load-path*` in every build.
            //
            // Dropping the reset costs the *builder* heap its boot garbage until
            // freeze (which already tolerates and skips it — see `reachable_clo`),
            // and costs every other path nothing: with the collector on, this
            // branch never ran. `_cp`/`_gc` are kept as the record of what was
            // tried; re-introducing a reset needs reachability from the root env,
            // not a bare high-water mark.
            let _ = (&cp, gc);
        }
        heap.truncate_roots(roots_base);
        ret.map(|()| result)
        // `scope` drops here → the caller's ns-state is restored (also on panic).
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
