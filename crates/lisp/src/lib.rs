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

pub mod core; // substrate: value, heap, alloc — what everything is addressed through
pub mod eval; // the tree-walking evaluator + its macro / compile pass
pub mod syntax; // surface: reader (text to Value) + printer (Value to text)
pub mod types; // the advisory type lattice + checker (nothing gates on it)

pub mod audio; // optional audio output backend (feature "audio", pulled in by "gui")
pub mod builtins;
pub mod bundle; // single-binary app release: append-to-binary bundling (ADR-038)
pub mod cli_support; // tiny mechanism the `brood` and `nest` binaries share
pub mod coverage; // line-coverage recording, off unless BROOD_COVERAGE is set (ADR-148)
pub mod dist; // distributed nodes: connect two runtimes over TCP, route messages
pub mod error; // errors + source positions (cross-cutting)
pub mod gui; // optional windowed display backend (feature "gui") — ADR-046 frontend #2
#[cfg(feature = "gui-gpu")]
pub mod gui_gpu; // optional GPU (OpenGL) render backend for `gui` — feature "gui-gpu"
pub mod introspect; // tooling-facing queries on a live Interp (LSP today, MCP next)
#[cfg(feature = "jit")]
pub mod jit; // tier-1 template JIT via Cranelift (feature "jit") — ADR-101, docs/value-repr.md
pub mod net; // thin non-blocking TCP socket mechanism (ADR-062); policy lives in bundled std/net/* (ADR-097)
pub mod perf; // VM work-attribution counters (feature "perf-stats") — docs/benchmarking.md
pub mod process; // the green-process scheduler // the primitive kernel (Rust mechanism; policy lives in std/*.blsp)
pub mod profile; // sampling CPU profiler over the VM's reified frames (observability timing tier)
pub mod subprocess; // persistent child-process mechanism: spawn + stdio pipes over the mailbox seam (ADR-104)
pub mod text_width; // grapheme-cluster display-cell width (the `display-width` builtin + the GUI grid)
pub mod treesit; // optional tree-sitter parsing for foreign languages (feature "treesit") — ROADMAP §C
#[cfg(feature = "wasm")]
pub mod wasm; // WASM component interop host (ADR-071/145); policy lives in std/wasm.blsp

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
    // Fast path: boot from the expanded-prelude cache (ReadyToRun-lite). The
    // full source boot costs ~31 ms, ~27 ms of which is macro-EXPANSION of the
    // prelude (measured 2026-07-19; see the devlog) — parse, eval, and freeze
    // together are ~4 ms. So the cache stores the *post-compile* (expanded +
    // resolved + static-quasiquote) forms as plain text, keyed by `build-id`
    // (the prelude is `include_str!`'d, so any binary change invalidates), and
    // a warm boot skips `eval::macros::compile` entirely. Any mismatch or
    // failure falls back to the source boot, which rewrites the cache.
    if std::env::var_os("BROOD_NO_BOOT_CACHE").is_none() {
        if let Some(bundle) = boot_from_cache() {
            return bundle;
        }
    }
    boot_from_source()
});

/// The expanded-prelude cache file for THIS binary:
/// `~/.cache/brood/prelude-expanded-<hash-of-build-id>.blsp`. Per-binary
/// naming (not one shared file) because the staleness key — `build-id` —
/// embeds each executable's own mtime: `brood`, `nest`, and every test binary
/// carry different stamps, and a single shared file would be endlessly
/// overwritten by whichever booted last, never hitting. Old builds' files are
/// pruned by age at write time (see `boot_cache_prune`).
fn boot_cache_path() -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    use std::path::PathBuf;
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    // DefaultHasher is deterministic across processes (fixed keys — unlike
    // RandomState), so every run of the same binary derives the same name.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    builtins::build_id_string().hash(&mut h);
    Some(
        base.join("brood")
            .join(format!("prelude-expanded-{:016x}.blsp", h.finish())),
    )
}

/// Best-effort prune of OTHER builds' expanded-prelude caches: any
/// `prelude-expanded-*.blsp` (except `keep`) not modified in ~7 days. Keeps a
/// dev machine's rebuild churn from accumulating one ~90 KB file per binary
/// per build forever, without deleting the caches other live binaries
/// (`nest`, an older installed `brood`) are actively hitting.
fn boot_cache_prune(dir: &std::path::Path, keep: &std::path::Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p == keep {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("prelude-expanded-") && name.ends_with(".blsp")) {
            continue;
        }
        let stale = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        if stale {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The boot cache's header line for THIS binary: `;; brood-boot-cache v1
/// <build-id> gensym=` (the caching boot's final gensym counter follows). A
/// cache whose header doesn't match byte-for-byte is stale and ignored.
fn boot_cache_header_prefix() -> String {
    format!(
        ";; brood-boot-cache v1 {} gensym=",
        builtins::build_id_string()
    )
}

/// Boot the shared bundle from the expanded-prelude cache. `None` (fall back
/// to [`boot_from_source`]) if the cache is absent, stale, or fails ANY step —
/// a failing cache file is deleted so the source boot's rewrite starts clean.
/// The raw prelude is still read (positioned) for `note_definition`, so LSP
/// stdlib navigation is identical on both paths; only the ~27 ms compile pass
/// is skipped.
fn boot_from_cache() -> Option<SharedBundle> {
    let t_start = std::time::Instant::now();
    let path = boot_cache_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let (header, body) = text.split_once('\n')?;
    // A non-matching header is a stale build — leave the file; the source boot
    // rewrites it.
    let gensym_max: u64 = header
        .strip_prefix(&boot_cache_header_prefix())?
        .trim()
        .parse()
        .ok()?;
    let run = || -> Option<SharedBundle> {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        heap.set_global(root);
        builtins::register(&mut heap, root);
        heap.set_current_file(prelude_source_path());
        // Raw positioned read for definition sites only (M-. parity with the
        // source boot); the cached forms drive evaluation. 1:1 by construction
        // (compile never splits a top-level form) — any drift is a stale file.
        let raw = syntax::reader::read_all_positioned(&mut heap, PRELUDE).ok()?;
        let cached = syntax::reader::read_all(&mut heap, body).ok()?;
        if raw.len() != cached.len() {
            return None;
        }
        // The cached expansions embed gensyms minted up to `gensym_max` in the
        // caching boot; floor the counter so runtime gensyms can't collide.
        core::value::gensym_floor(gensym_max);
        for ((raw_form, pos), form) in raw.into_iter().zip(cached) {
            heap.note_definition(raw_form, pos);
            heap.note_definition(form, pos);
            eval::eval(&mut heap, form, root).ok()?;
        }
        heap.set_current_file(None);
        let (code, bindings) = heap.freeze_as_shared_code(root);
        Some(SharedBundle {
            code: Arc::new(code),
            bindings,
        })
    };
    let bundle = run();
    if bundle.is_none() {
        // Current-build header but the body failed to read/eval: the file is
        // corrupt — remove it so the next source boot rewrites from scratch.
        let _ = std::fs::remove_file(&path);
    } else if std::env::var_os("BROOD_BOOT_TRACE").is_some() {
        eprintln!("[boot] cache hit — total={:?}", t_start.elapsed());
    }
    bundle
}

/// The full source boot: parse + macro-expand + eval + freeze the prelude,
/// then (best-effort) write the expanded-prelude cache for the next boot.
fn boot_from_source() -> SharedBundle {
    let t_start = std::time::Instant::now();
    // Build the prelude + builtins in a throwaway builder heap, then relocate it
    // all into the shared region. Done once for the whole process.
    let mut heap = Heap::new();
    let root = heap.new_env(None);
    heap.set_global(root);
    builtins::register(&mut heap, root);
    let t_builtins = t_start.elapsed();
    // Record each prelude def's source location against a materialized, on-disk
    // copy of the prelude, so the LSP can jump `M-.` into the standard library
    // (the prelude is `include_str!`'d — there's no source file at runtime
    // otherwise). Best-effort and nav-only: if the cache can't be written we
    // simply set no file, `note_definition` no-ops, and stdlib goto stays
    // unavailable (everything else is unaffected). See `prelude_source_path`.
    let prelude_file = prelude_source_path();
    heap.set_current_file(prelude_file);
    let t_mark = std::time::Instant::now();
    // Positioned read so each def carries the line/col goto-definition lands on.
    let forms = syntax::reader::read_all_positioned(&mut heap, PRELUDE).expect("read prelude");
    let t_read = t_mark.elapsed();
    let t_mark = std::time::Instant::now();
    let mut t_expand = std::time::Duration::ZERO;
    // The boot cache's payload: each compiled form, printed. A form whose
    // print→read→print round-trip isn't a fixpoint poisons the whole cache
    // (never write a file we can't provably re-read into the same forms).
    let write_cache = std::env::var_os("BROOD_NO_BOOT_CACHE").is_none();
    let mut cache_ok = write_cache;
    let mut printed_forms: Vec<String> = Vec::new();
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
        let t_e = std::time::Instant::now();
        let form = eval::macros::compile(&mut heap, form, root)
            .unwrap_or_else(|e| panic!("prelude expand: {}", e));
        let d = t_e.elapsed();
        if d.as_micros() > 300 && std::env::var_os("BROOD_BOOT_TRACE").is_some() {
            eprintln!("[boot-form] {:?} at {:?}", d, pos);
        }
        t_expand += d;
        if cache_ok {
            let printed = syntax::printer::print(&heap, form);
            match syntax::reader::read_all(&mut heap, &printed) {
                Ok(v) if v.len() == 1 && syntax::printer::print(&heap, v[0]) == printed => {
                    printed_forms.push(printed);
                }
                _ => cache_ok = false,
            }
        }
        heap.note_definition(form, pos);
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    heap.set_current_file(None);
    let t_eval = t_mark.elapsed();
    let t_mark = std::time::Instant::now();
    let (code, bindings) = heap.freeze_as_shared_code(root);
    let t_freeze = t_mark.elapsed();
    if cache_ok {
        if let Some(path) = boot_cache_path() {
            // Atomic-enough for the purpose: write to a sibling temp file and
            // rename, so a concurrent booting process never reads a torn file.
            let _ = (|| -> std::io::Result<()> {
                let dir = path.parent().expect("cache path has a dir");
                std::fs::create_dir_all(dir)?;
                let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
                let mut payload = format!(
                    "{}{}\n",
                    boot_cache_header_prefix(),
                    core::value::gensym_counter()
                );
                payload.push_str(&printed_forms.join("\n"));
                payload.push('\n');
                std::fs::write(&tmp, payload)?;
                std::fs::rename(&tmp, &path)?;
                boot_cache_prune(dir, &path);
                Ok(())
            })();
        }
    }
    if std::env::var_os("BROOD_BOOT_TRACE").is_some() {
        eprintln!(
            "[boot] builtins={:?} read={:?} expand={:?} eval={:?} freeze={:?} total={:?} (source boot{})",
            t_builtins,
            t_read,
            t_expand,
            t_eval - t_expand,
            t_freeze,
            t_start.elapsed(),
            if cache_ok { ", cache written" } else { "" }
        );
    }
    SharedBundle {
        code: Arc::new(code),
        bindings,
    }
}

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

impl Drop for Interp {
    /// The embedded-host teardown: reap every permanently-parked green process
    /// of THIS runtime (a `(receive)` nothing will ever send to holds its
    /// whole process + heap in the mailbox waiter slot — the long-flagged
    /// leak; see `shutdown_runtime_parked`). The standalone binaries exit the
    /// OS process right after, so this is effectively free there; a long-lived
    /// host that creates and drops `Interp`s no longer accumulates them.
    fn drop(&mut self) {
        process::shutdown_runtime_parked(&self.heap.runtime_arc());
    }
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
        let exit = process::spawn_root_program(&self.heap, src, file)?;
        exit.wait()
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
        let form_vals: Vec<Value> = forms.iter().map(|&(f, _)| f).collect();
        let prev_ns = self.heap.set_compile_ns(None);
        let known = if eval::macros::file_opens_ns(&self.heap, &form_vals) {
            eval::macros::scan_def_names(&self.heap, &form_vals)
        } else {
            std::collections::HashSet::new()
        };
        let prev_known = self.heap.set_ns_known_names(known);
        let prev_imports = self.heap.set_imports(std::collections::HashMap::new());
        let cp = self.heap.checkpoint();
        let gc = self.heap.gc_enabled();
        let mut result = Value::nil();
        let n = forms.len();
        let roots_base = self.heap.roots_len();
        for &(form, _) in &forms {
            self.heap.push_root(form);
        }
        let mut ret: Result<(), LispError> = Ok(());
        for i in 0..n {
            // The form's current handle (relocated if an earlier form's eval
            // triggered a collection); the `forms` Vec copy may be stale.
            let form = self.heap.root_at(roots_base + i);
            let pos = forms[i].1;
            // Record def sites (file runs only): the raw form first (preserves
            // pre-expansion spans for `defn`/`defmacro`), then the expanded form
            // so def-like macros whose raw head isn't recognised (e.g. `defseq`)
            // still get their call-site position. Both no-op off a definition or
            // with no file set.
            if let Some(pos) = pos {
                self.heap.note_definition(form, pos);
            }
            // Compile pass: expand macros once before evaluating (form-by-form, so
            // a macro a form defines is in scope for the forms after it), then
            // route through the compiling VM (ADR-076) or, under BROOD_VM=0, the
            // tree-walker (Stage 0 defers, so the two are at parity).
            let outcome = eval::macros::compile(&mut self.heap, form, self.root)
                .and_then(|f| {
                    if let Some(pos) = pos {
                        self.heap.note_definition(f, pos);
                    }
                    if eval::compile::vm_enabled() {
                        eval::compile::run(&mut self.heap, f, self.root)
                    } else {
                        eval::eval(&mut self.heap, f, self.root)
                    }
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
        self.heap.truncate_roots(roots_base);
        self.heap.set_compile_ns(prev_ns);
        self.heap.set_ns_known_names(prev_known);
        self.heap.set_imports(prev_imports);
        ret.map(|()| result)
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
