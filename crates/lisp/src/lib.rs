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

pub mod audio; // optional audio output backend (feature "audio", pulled in by "gui")
pub mod builtins;
pub mod bundle; // single-binary app release: append-to-binary bundling (ADR-038)
pub mod cli_support; // tiny mechanism the `brood` and `nest` binaries share
pub mod coverage; // line-coverage recording, off unless BROOD_COVERAGE is set (ADR-148)
pub mod debug_flags; // the BROOD_* diagnostic-flag catalogue (`brood --debug-flags`)
pub mod dist; // distributed nodes: connect two runtimes over TCP, route messages
pub mod error; // errors + source positions (cross-cutting)
pub mod gui; // optional windowed display backend (feature "gui") — ADR-046 frontend #2
#[cfg(feature = "gui-gpu")]
pub mod gui_gpu; // optional GPU (OpenGL) render backend for `gui` — feature "gui-gpu"
pub mod introspect; // tooling-facing queries on a live Interp (LSP today, MCP next)
#[cfg(feature = "jit")]
pub mod jit; // tier-1 template JIT via Cranelift (feature "jit") — ADR-101, docs/value-repr.md
#[cfg(not(target_arch = "wasm32"))]
pub mod net; // thin non-blocking TCP socket mechanism (ADR-062); policy lives in bundled std/net/* (ADR-097)
#[cfg(target_arch = "wasm32")]
#[path = "net_wasm.rs"]
pub mod net; // wasm has no sockets — stub with the same API (fails at runtime)
pub mod perf; // VM work-attribution counters (feature "perf-stats") — docs/benchmarking.md
pub mod process; // the green-process scheduler // the primitive kernel (Rust mechanism; policy lives in std/*.blsp)
pub mod profile; // sampling CPU profiler over the VM's reified frames (observability timing tier)
pub mod renames; // the rename ledger: where a deliberately renamed public name went (ADR-304)
pub mod subprocess; // persistent child-process mechanism: spawn + stdio pipes over the mailbox seam (ADR-104)
pub mod text_width; // grapheme-cluster display-cell width (the `string/display-width` builtin + the GUI grid)
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
    /// The prelude's module-private names (ADR-146), captured from the builder heap
    /// where `%mark-private` recorded them. Seeds each live runtime's private set,
    /// since the prelude is inserted (not re-evaluated) and clean names can't be
    /// re-derived. Parallel to `bindings`.
    private: Vec<Symbol>,
    /// The prelude's stability metadata (ADR-283) — the `(meta …)` facts recorded in the
    /// builder heap. Carried for the same reason `private` is: the prelude is inserted
    /// into each live runtime rather than re-evaluated, so nothing re-runs
    /// `%register-meta` there. Parallel to `bindings`.
    meta: Vec<(Symbol, core::heap::NameMeta)>,
}

static SHARED: LazyLock<SharedBundle> = LazyLock::new(|| {
    // Fast path: boot from the expanded-prelude cache (ReadyToRun-lite). The
    // full source boot costs ~31 ms, ~27 ms of which is macro-EXPANSION of the
    // prelude (measured 2026-07-19; see the devlog) — parse, eval, and freeze
    // together are ~4 ms. So the cache stores the *post-compile* (expanded +
    // resolved + static-quasiquote) forms as plain text, keyed by `system/build-id`
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
/// naming (not one shared file) because the staleness key — `system/build-id` —
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

/// Best-effort prune of OTHER builds' expanded-prelude caches: keep the
/// `MAX_KEEP` most recently modified `prelude-expanded-*.blsp` (plus `keep`)
/// and delete the rest, and separately drop anything older than `MAX_AGE`.
///
/// **Bounded by COUNT, not only by age, because age does not bound anything.**
/// The cache name hashes `system/build-id`, which embeds the binary's mtime, so
/// every rebuild of every binary — `brood`, `nest`, `brood-lsp`, each test
/// binary, each `target/ab/<sha>` worktree — mints a *new* ~190 KB file. The
/// original 7-day rule then deletes nothing at all on a machine that rebuilds
/// more than a handful of times a week: measured 2026-08-27 on this repo's own
/// dev machine, **4192 files / 732 MB**, none of them week-old. Worse, the prune
/// itself walks that directory and stats every entry on each cache-writing boot
/// — **7.6 ms**, which is an entire warm boot (7.6 ms) spent tidying.
///
/// Deleting a *recent* file that another live binary is still hitting is safe:
/// that binary pays one source boot and rewrites its own cache. The failure mode
/// is a slower boot once, never a wrong one — so a count cap is the right shape,
/// and the age rule stays as a floor for a directory that is under the cap but
/// full of long-dead builds.
fn boot_cache_prune(dir: &std::path::Path, keep: &std::path::Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    /// Enough for the binaries plausibly in play at once (`brood`, `nest`,
    /// `brood-lsp`, a couple of test binaries, an `ab` worktree or two) — at
    /// ~190 KB each this bounds the prelude cache at ~3 MB rather than at
    /// whatever a week of rebuilding produces.
    const MAX_KEEP: usize = 16;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // (mtime, path) for every candidate but `keep`, which is this binary's own
    // freshly-written file and is never a candidate.
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
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
        let Ok(modified) = e.metadata().and_then(|m| m.modified()) else {
            // Unreadable metadata: treat as ancient so it sorts to the drop end
            // rather than occupying a keep slot it cannot justify.
            let _ = std::fs::remove_file(&p);
            continue;
        };
        found.push((modified, p));
    }
    // Newest first, then everything past the cap goes, plus anything stale.
    found.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    for (i, (modified, p)) in found.iter().enumerate() {
        let stale = modified.elapsed().ok().is_some_and(|age| age > MAX_AGE);
        if i >= MAX_KEEP || stale {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The boot cache's header line for THIS binary: `;; brood-boot-cache v1
/// <build-id> gensym=` (the caching boot's final gensym counter follows). A
/// cache whose header doesn't match byte-for-byte is stale and ignored.
fn boot_cache_header_prefix() -> String {
    format!(
        ";; brood-boot-cache v2 {} gensym=",
        builtins::build_id_string()
    )
}

/// Boot the shared bundle from the expanded-prelude cache. `None` (fall back
/// to [`boot_from_source`]) if the cache is absent, stale, or fails ANY step —
/// a failing cache file is deleted so the source boot's rewrite starts clean.
/// Each cached line carries its form's source position and the def-names the
/// *un-expanded* form contributed, so LSP stdlib navigation is identical on both
/// paths without re-reading the prelude: that positioned read was 3.5 ms of a
/// 26 ms warm boot and produced nothing else. Only the ~27 ms compile pass and
/// that read are skipped.
fn boot_from_cache() -> Option<SharedBundle> {
    let t_start = web_time::Instant::now();
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
        // Each line is `<line>:<col>:<def-name,…> <printed form>` — the position and
        // the un-expanded form's def-names, recorded by the source boot that wrote
        // this file, then the expansion that drives evaluation.
        let mut meta = Vec::new();
        // One bulk read of the expansions, not one per line: the reader amortises
        // its scanner across a single buffer, and splitting it per form measured
        // +1 ms on a 23 ms boot.
        let mut source = String::with_capacity(body.len());
        for line in body.lines().filter(|l| !l.is_empty()) {
            let (head, printed) = line.split_once(' ')?;
            let mut parts = head.splitn(3, ':');
            let l: u32 = parts.next()?.parse().ok()?;
            let c: u32 = parts.next()?.parse().ok()?;
            meta.push((crate::error::Pos { line: l, col: c }, parts.next()?));
            source.push_str(printed);
            source.push('\n');
        }
        let cached = syntax::reader::read_all(&mut heap, &source).ok()?;
        // 1:1 by construction (one printed form per line) — any drift is a torn file.
        if cached.len() != meta.len() {
            return None;
        }
        // The cached expansions embed gensyms minted up to `gensym_max` in the
        // caching boot; floor the counter so runtime gensyms can't collide.
        core::value::gensym_floor(gensym_max);
        let t_read = t_start.elapsed();
        for ((pos, names), form) in meta.into_iter().zip(cached) {
            // The un-expanded form's def-names, recorded by the boot that wrote this
            // file — the raw prelude is not read here.
            for name in names.split(',').filter(|n| !n.is_empty()) {
                heap.record_def_site(core::value::intern(name), pos);
            }
            heap.note_definition(form, pos);
            eval::eval(&mut heap, form, root).ok()?;
        }
        let t_eval = t_start.elapsed();
        heap.set_current_file(None);
        let private = heap.private_names_snapshot();
        let name_meta = heap.name_meta_snapshot();
        let t_pre_freeze = t_start.elapsed();
        let (code, bindings) = heap.freeze_as_shared_code(root);
        if std::env::var_os("BROOD_BOOT_TRACE").is_some() {
            // The cache-hit phase breakdown, the counterpart of the source boot's
            // line below. Without it the only number this path reported was its
            // total, which cannot say whether a boot regression is in reading the
            // cache, in evaluating the prelude, or in one `require` inside it.
            eprintln!(
                "[boot] parse={:?} eval={:?} freeze={:?}",
                t_read,
                t_eval - t_read,
                t_start.elapsed() - t_pre_freeze
            );
        }
        Some(SharedBundle {
            code: Arc::new(code),
            bindings,
            private,
            meta: name_meta,
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
    let t_start = web_time::Instant::now();
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
    let t_mark = web_time::Instant::now();
    // Positioned read so each def carries the line/col goto-definition lands on.
    let forms = syntax::reader::read_all_positioned(&mut heap, PRELUDE).expect("read prelude");
    let t_read = t_mark.elapsed();
    let t_mark = web_time::Instant::now();
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
        // Recording variant: the cache-hit boot does not read the raw prelude, so
        // the names this *un-expanded* form contributes are captured here and
        // written into the cache line below.
        let raw_names = heap.note_definition_recording(form, pos);
        // Compile pass (expand macros, then namespace-resolve — a no-op here since
        // the prelude is the root namespace), then evaluate. Form-by-form so a
        // macro defined by one form is visible to the next.
        let t_e = web_time::Instant::now();
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
                    let names: Vec<&str> = raw_names
                        .iter()
                        .map(|&n| core::value::symbol_name_ref(n))
                        .collect();
                    // A printed form never contains a newline (the printer emits one
                    // line) and a symbol never contains a space, so `line:col:names `
                    // is unambiguous against the form that follows it.
                    printed_forms.push(format!(
                        "{}:{}:{} {}",
                        pos.line,
                        pos.col,
                        names.join(","),
                        printed
                    ));
                }
                _ => cache_ok = false,
            }
        }
        heap.note_definition(form, pos);
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    heap.set_current_file(None);
    let t_eval = t_mark.elapsed();
    let t_mark = web_time::Instant::now();
    let private = heap.private_names_snapshot();
    let name_meta = heap.name_meta_snapshot();
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
        private,
        meta: name_meta,
    }
}

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
    /// `(crash-report/arm-default)` here (ADR-305): armed in the program's process the
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
    #[cfg(target_arch = "wasm32")]
    pub fn run_program_repr(&mut self, src: &str) -> Result<String, LispError> {
        let exit = process::spawn_root_program(&self.heap, src, None, None)?;
        exit.wait()?;
        Ok(exit.take_result().unwrap_or_default())
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

/// The standard prelude, written in Brood and baked into the binary. Split across
/// `std/prelude/*.blsp` for navigability and concatenated **in order** here — the pieces are
/// bare-root prelude source, so evaluation order is load-bearing (macros before use, forward
/// references). **This list is the authoritative order**; a new prelude file must be added at
/// the right position here. The concatenation is byte-identical to the former single
/// `std/prelude.blsp`, so runtime behaviour, source positions, and the materialized
/// `prelude.blsp` copy are unchanged.
pub const PRELUDE: &str = concat!(
    include_str!("../../../std/prelude/core.blsp"),
    include_str!("../../../std/prelude/predicates.blsp"),
    include_str!("../../../std/prelude/map.blsp"),
    include_str!("../../../std/prelude/control.blsp"),
    include_str!("../../../std/prelude/match.blsp"),
    include_str!("../../../std/prelude/process.blsp"),
    include_str!("../../../std/prelude/seq.blsp"),
    include_str!("../../../std/prelude/string.blsp"),
    include_str!("../../../std/prelude/tools.blsp"),
    // Behaviour contracts are CORE (defbehaviour / %register-protocol / ops / *protocols*).
    // After tools.blsp, which defines the `swap-registry!` macro protocol uses.
    include_str!("../../../std/protocol.blsp"),
);

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

#[cfg(test)]
mod prelude_hygiene {
    //! Boot-image hygiene: the prelude is frozen at boot and its macros expand into
    //! arbitrary user contexts where a namespaced module (`table/`, `os/`, `proc/`, …)
    //! is NOT loaded; its function bodies likewise run before any module loads. So
    //! prelude *code* must reach for the `%`-primitive, never the module wrapper — a
    //! `table/new` leaking into a macro expansion is unbound at a user's call site, not
    //! at build (this cost real time: `with-err-str` and `%table-from-map` both shipped
    //! a `table/new` the boot image could not resolve). This lint catches it at build.
    use super::*;
    use crate::core::value::{self, ValueRef};

    // There is no allowed-module list any more. The prelude used to force-load `string`
    // and `seq` at boot, which made every `string/…` / `seq/…` reference resolve and cost
    // 12.1 ms of a 26 ms boot on every invocation (KI-61); those modules now load lazily,
    // so a qualified reference is only safe when something binds the name up front. The
    // three things that do are checked by name below — a registered primitive, a prelude
    // definition, or an `%autoload` declaration — which is stricter than a module allowlist
    // and needs no editing when a namespacing wave moves another name out of the prelude.

    /// Every name `builtins::register` binds — the always-available set, slash-named
    /// primitives included.
    fn registered_primitives() -> std::collections::HashSet<String> {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        crate::builtins::register(&mut heap, root);
        heap.env_chain_names(root)
            .into_iter()
            .map(value::symbol_name)
            .collect()
    }

    /// Collect every qualified `mod/name` symbol reachable in `v` as executable code.
    /// Skips `(quote …)` subtrees (inert data, not emitted code) but walks quasiquote —
    /// a `` `(… table/new …) `` template IS the code a macro emits. String docstrings are
    /// `Str` atoms and comments are reader trivia, so both are excluded for free.
    fn collect_qualified(heap: &Heap, v: Value, out: &mut Vec<String>) {
        match v.unpack() {
            ValueRef::Sym(s) => {
                let name = value::symbol_name(s);
                if let Some(slash) = name.find('/') {
                    // empty module = root-qualified `/name` or the `/` (division) op — skip.
                    if slash > 0 {
                        out.push(name);
                    }
                }
            }
            ValueRef::Pair(p) => {
                let (car, cdr) = heap.pair(p);
                if let ValueRef::Sym(s) = car.unpack() {
                    if value::symbol_name(s) == "quote" {
                        return;
                    }
                }
                collect_qualified(heap, car, out);
                collect_qualified(heap, cdr, out);
            }
            ValueRef::Vector(id) => {
                for item in heap.vector(id).to_vec() {
                    collect_qualified(heap, item, out);
                }
            }
            ValueRef::Map(id) => {
                for (k, val) in heap.map_entries(id) {
                    collect_qualified(heap, k, out);
                    collect_qualified(heap, val, out);
                }
            }
            ValueRef::Set(id) => {
                for e in heap.set_elems(id) {
                    collect_qualified(heap, e, out);
                }
            }
            _ => {}
        }
    }

    /// Every global a prelude form defines: the head symbol of a `def`-family form
    /// (including the `%defseq` and `defability`/`defrecord` definers, whose expansions
    /// bind their first argument). Qualified prelude names like `string/format` — defined
    /// in `std/prelude/string.blsp`, not in the `string` MODULE — land here.
    fn prelude_definitions(heap: &Heap, forms: &[Value]) -> std::collections::HashSet<String> {
        const DEFINERS: &[&str] = &[
            "def",
            "def-",
            "defn",
            "defn-",
            "defmacro",
            "%defseq",
            "defdyn",
            "defability",
            "defrecord",
            // `defmulti` binds a name exactly as the others do. It was missing until
            // 2026-08-28 and nothing noticed, because no prelude multimethod had a
            // slash in its name — the lint only inspects QUALIFIED references, so a bare
            // `num-add` never reached it. `num/add` did, and read as an unloaded module
            // wrapper.
            "defmulti",
        ];
        let mut out = std::collections::HashSet::new();
        for &form in forms {
            let ValueRef::Pair(p) = form.unpack() else {
                continue;
            };
            let (car, cdr) = heap.pair(p);
            let ValueRef::Sym(head) = car.unpack() else {
                continue;
            };
            if !DEFINERS.contains(&value::symbol_name(head).as_str()) {
                continue;
            }
            let ValueRef::Pair(rest) = cdr.unpack() else {
                continue;
            };
            if let ValueRef::Sym(name) = heap.pair(rest).0.unpack() {
                out.insert(value::symbol_name(name));
            }
        }
        out
    }

    /// The `(%autoload mod (name arity) …)` declarations in a prelude file, as
    /// `(mod/name, arity)`. Read out of the source rather than out of a live image so the
    /// two tests below can check the declaration itself: that one exists for every
    /// reference, and that its arity still matches the module.
    fn autoload_declarations(heap: &Heap, forms: &[Value]) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for &form in forms {
            let Ok(items) = heap.list_to_vec(form) else {
                continue;
            };
            let Some(&head) = items.first() else { continue };
            let ValueRef::Sym(h) = head.unpack() else {
                continue;
            };
            if value::symbol_name(h) != "%autoload" {
                continue;
            }
            let ValueRef::Sym(module) = items[1].unpack() else {
                continue;
            };
            let module = value::symbol_name(module);
            for &spec in &items[2..] {
                let Ok(pair) = heap.list_to_vec(spec) else {
                    continue;
                };
                let (Some(&name), Some(&arity)) = (pair.first(), pair.get(1)) else {
                    continue;
                };
                if let (ValueRef::Sym(n), Value::Int(a)) = (name.unpack(), arity) {
                    out.push((format!("{module}/{}", value::symbol_name(n)), a as usize));
                }
            }
        }
        out
    }

    #[test]
    fn prelude_code_references_no_unloaded_module_wrapper() {
        const FILES: &[(&str, &str)] = &[
            ("core.blsp", include_str!("../../../std/prelude/core.blsp")),
            (
                "predicates.blsp",
                include_str!("../../../std/prelude/predicates.blsp"),
            ),
            ("map.blsp", include_str!("../../../std/prelude/map.blsp")),
            (
                "control.blsp",
                include_str!("../../../std/prelude/control.blsp"),
            ),
            (
                "match.blsp",
                include_str!("../../../std/prelude/match.blsp"),
            ),
            (
                "process.blsp",
                include_str!("../../../std/prelude/process.blsp"),
            ),
            ("seq.blsp", include_str!("../../../std/prelude/seq.blsp")),
            (
                "string.blsp",
                include_str!("../../../std/prelude/string.blsp"),
            ),
            (
                "tools.blsp",
                include_str!("../../../std/prelude/tools.blsp"),
            ),
            ("protocol.blsp", include_str!("../../../std/protocol.blsp")),
        ];
        let primitives = registered_primitives();
        let mut heap = Heap::new();
        // Two passes: the whole prelude's definitions and autoload declarations have to be
        // known before any file's references can be judged, since a reference in `core.blsp`
        // may name something `tools.blsp` declares.
        let read: Vec<(&str, Vec<Value>)> = FILES
            .iter()
            .map(|(fname, src)| {
                let forms = syntax::reader::read_all(&mut heap, src)
                    .unwrap_or_else(|e| panic!("read {fname}: {e:?}"));
                (*fname, forms)
            })
            .collect();
        let mut defined = std::collections::HashSet::new();
        let mut autoloaded = std::collections::HashSet::new();
        for (_, forms) in &read {
            defined.extend(prelude_definitions(&heap, forms));
            autoloaded.extend(
                autoload_declarations(&heap, forms)
                    .into_iter()
                    .map(|(q, _)| q),
            );
        }
        let mut violations: Vec<String> = Vec::new();
        for (fname, forms) in &read {
            for &form in forms {
                let mut found = Vec::new();
                collect_qualified(&heap, form, &mut found);
                for q in found {
                    // Three ways a qualified name is bound with no module load: a
                    // slash-named kernel primitive (`file/slurp`, `string/split`), a
                    // prelude definition (`string/format` lives in the prelude, not in
                    // the `string` module), and an `%autoload` stub.
                    if primitives.contains(&q) || defined.contains(&q) || autoloaded.contains(&q) {
                        continue;
                    }
                    violations.push(format!("{fname}: {q}"));
                }
            }
        }
        violations.sort();
        violations.dedup();
        assert!(
            violations.is_empty(),
            "prelude code references a name nothing binds at boot. The modules the prelude \
             once force-loaded now load lazily (KI-61), so reach for the `%`-primitive, or \
             declare the name in the `%autoload` list in std/prelude/tools.blsp:\n  {}",
            violations.join("\n  ")
        );
    }

    /// The other half of the autoload contract: a declared arity that has drifted from its
    /// module would make `def`'s reload check announce an arity change on every load of that
    /// module, and would report a caller's arity error from inside the stub. A declared name
    /// the module does not define at all would loop until `%autoload-call`'s re-entry guard
    /// raised — a runtime failure this catches at build.
    ///
    /// Checks both: the loaded arity matches the declaration, and the loaded arglist is not
    /// still the stub's own generated `(a0 a1 …)` parameters (which would mean the module
    /// loaded without defining the name, and the count alone would agree).
    /// `->string` is defined TWICE by construction: once in `std/prelude/core.blsp` as the
    /// bootstrap implementation the prelude's own machinery calls (~60 sites, all of them
    /// before `defability Display` has been evaluated), and again as the `Display` impls for
    /// `:keyword`/`:symbol`, which restate the sigil rule because delegating to the name
    /// they have just rebound would recurse forever.
    ///
    /// Two statements of one rule can drift, and both failure modes are silent: the impls
    /// once shipped as `(->string [k] (->string k))` — an infinite loop, not a compile
    /// error — and a fix to one spelling that misses the other changes a value's display
    /// only after the ability loads. So pin the answer at both tiers.
    #[test]
    fn bootstrap_and_ability_agree() {
        let mut interp = Interp::new();
        let mut eval = |src: &str| -> String {
            let v = interp
                .eval_str(src)
                .unwrap_or_else(|e| panic!("{src}: {}", e.message));
            interp.print(v)
        };
        // The ability has taken over the name by now; this is the post-upgrade tier.
        for (expr, want) in [
            ("(->string :foo)", "\"foo\""),
            ("(->string 'foo)", "\"foo\""),
            ("(->string \"foo\")", "\"foo\""),
            ("(->string 42)", "\"42\""),
            ("(->string (type-of 1))", "\"int\""),
            // the sigil rule is exactly what distinguishes this from `str`/`pr-str`
            ("(str :foo)", "\":foo\""),
            ("(pr-str :foo)", "\":foo\""),
        ] {
            assert_eq!(eval(expr), want, "{expr}");
        }
        // And the bootstrap tier. The body is EXTRACTED from `core.blsp` rather than
        // copied here: a hard-coded copy would keep passing after someone edited the real
        // one, which is the exact drift this test exists to catch.
        let core = include_str!("../../../std/prelude/core.blsp");
        let marker = "(defn ->string (x)";
        let start = core
            .find(marker)
            .expect("core.blsp no longer defines the bootstrap `->string`");
        // Search only the defn's own lines — scanning the whole rest of the file would
        // happily find some *other* `(if …)` and test that instead, which is how this
        // check first "passed" a deliberate sabotage for the wrong reason.
        let body_line = core[start..]
            .lines()
            .take_while(|l| !l.starts_with(";;"))
            .find(|l| l.trim_start().starts_with("(if "))
            .expect(
                "bootstrap `->string` in core.blsp is no longer a single `(if …)` line — \
                 update this extraction (and check the ability impls still agree with it)",
            )
            .trim();
        // The line ends with the `defn`'s own closing paren(s) too; keep only as many as
        // the body itself opened.
        let mut body = body_line;
        while body.matches(')').count() > body.matches('(').count() {
            body = body[..body.len() - 1].trim_end();
        }
        let boot = format!("(fn (x) {body})");
        for (arg, want) in [(":foo", "\"foo\""), ("'foo", "\"foo\""), ("42", "\"42\"")] {
            assert_eq!(
                eval(&format!("({boot} {arg})")),
                want,
                "bootstrap tier disagrees for {arg}"
            );
        }
        // `name` is a user's word now, not the language's — ADR-166 reserved it for years.
        assert_eq!(
            eval("(bound? 'name)"),
            "false",
            "`name` is bound again — the point of folding it into `->string` was to free it"
        );
    }

    /// `builtins::numeric::num_multi_dispatch` maps an operator to a multimethod NAME as a
    /// bare string — `"+" => "num/add"` — and looks it up in the global table. That is
    /// ADR-251's recorded rename hazard in its purest form: a rename that updates the
    /// `defmulti` but not the table does not fail to compile and does not fail a test. It
    /// fails at a *user's* call site, the first time someone adds a record to a record, with
    /// "the `num/add` multimethod is not loaded" — for an operator that works fine on ints.
    ///
    /// This was one string table away from happening when the family moved off its `num-`
    /// hyphen prefix, so pin the two together.
    #[test]
    fn the_num_multimethods_the_kernel_names_all_exist() {
        let mut interp = Interp::new();
        for op in ["num/add", "num/sub", "num/mul", "num/div"] {
            let v = interp
                .eval_str(&format!("(bound? '{op})"))
                .unwrap_or_else(|e| panic!("{op}: {}", e.message));
            assert_eq!(
                interp.print(v),
                "true",
                "`{op}` is named by numeric.rs's operator table but is not bound — the \
                 `defmulti` in std/prelude/tools.blsp and that table have drifted apart"
            );
        }
        // And the table really is the source of those names, so a future edit to it is
        // caught here rather than by a user: assert the spelling the kernel uses.
        let src = include_str!("builtins/numeric.rs");
        for op in ["num/add", "num/sub", "num/mul", "num/div"] {
            assert!(
                src.contains(&format!("\"{op}\"")),
                "numeric.rs no longer names `{op}` — update this test with the table"
            );
        }
    }

    #[test]
    fn every_autoload_declaration_matches_its_module() {
        let mut heap = Heap::new();
        let src = include_str!("../../../std/prelude/tools.blsp");
        let forms = syntax::reader::read_all(&mut heap, src).expect("read tools.blsp");
        let declared = autoload_declarations(&heap, &forms);
        assert!(
            !declared.is_empty(),
            "no `%autoload` declarations found — the scanner has drifted from the macro's shape"
        );
        // A declaration that shadows a slash-named kernel primitive is the worst case:
        // the stub REPLACES an always-bound native with one that loads a module and
        // forwards to itself. Caught here rather than at the first call site.
        let primitives = registered_primitives();
        let mut interp = Interp::new();
        let mut problems: Vec<String> = Vec::new();
        for (qualified, arity) in declared {
            if primitives.contains(&qualified) {
                problems.push(format!(
                    "{qualified}: already a kernel primitive — the stub shadows it; drop the \
                     declaration"
                ));
                continue;
            }
            let module = &qualified[..qualified.find('/').unwrap()];
            interp
                .eval_str(&format!("(require-one '{module})"))
                .unwrap_or_else(|e| panic!("require {module}: {e:?}"));
            let arglist = interp
                .eval_str(&format!("(arglist {qualified})"))
                .map(|v| interp.print(v))
                .unwrap_or_else(|e| format!("<error: {}>", e.message));
            let stub_params = format!(
                "({})",
                (0..arity)
                    .map(|i| format!("a{i}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let count = arglist.split_whitespace().count();
            if arglist == stub_params {
                problems.push(format!(
                    "{qualified}: still the autoload stub after loading `{module}` — \
                     the module does not define it"
                ));
            } else if count != arity {
                problems.push(format!(
                    "{qualified}: declared arity {arity}, module defines {arglist}"
                ));
            }
        }
        assert!(
            problems.is_empty(),
            "autoload declarations in std/prelude/tools.blsp have drifted:\n  {}",
            problems.join("\n  ")
        );
    }
}

#[cfg(test)]
mod boot_cache_prune_tests {
    use super::boot_cache_prune;
    use std::io::Write;

    /// Create `n` `prelude-expanded-*.blsp` files with ascending mtimes and return their
    /// paths oldest-first. Ascending mtimes are what makes "keeps the NEWEST" testable.
    fn seed(dir: &std::path::Path, n: usize) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for i in 0..n {
            let p = dir.join(format!("prelude-expanded-{i:016x}.blsp"));
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(b"x").unwrap();
            // Stamp mtimes explicitly rather than relying on creation order: the
            // filesystem's timestamp granularity is coarse enough that files written in
            // one loop can share an mtime, which would make the ordering assertion
            // below pass or fail by luck.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs((n - i) as u64);
            set_mtime(&p, t);
            out.push(p);
        }
        out
    }

    /// Stamp `p`'s mtime. `std::fs::FileTimes` rather than the `filetime` crate — this is
    /// the only place in the workspace that needs it, and a dev-dependency for two tests
    /// is not worth it.
    fn set_mtime(p: &std::path::Path, t: std::time::SystemTime) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(t))
            .unwrap();
    }

    fn remaining(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.starts_with("prelude-expanded-") && n.ends_with(".blsp")
            })
            .count()
    }

    /// The regression this exists for. The prune used to bound by AGE ONLY, and the cache
    /// name hashes `build-id` (which embeds the binary's mtime), so every rebuild minted a
    /// new ~190 KB file and the 7-day rule deleted none of them: measured 4192 files /
    /// 732 MB on a dev machine, with the prune's own directory walk costing 7.6 ms — a
    /// whole warm boot — on every cache-writing boot.
    #[test]
    fn prune_bounds_the_cache_by_count_not_only_by_age() {
        let dir = std::env::temp_dir().join(format!("brood-prune-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // All freshly stamped and none stale, so an age-only prune removes NOTHING here —
        // which is exactly the bug. Sabotage-checked: reverting to the age-only body
        // leaves all 40 and fails this assertion.
        let files = seed(&dir, 40);
        let keep = dir.join("prelude-expanded-keep.blsp");
        std::fs::write(&keep, b"k").unwrap();

        boot_cache_prune(&dir, &keep);

        // 16 kept by the cap + `keep` itself, which is never a candidate.
        assert_eq!(remaining(&dir), 17, "count cap did not bound the directory");
        assert!(keep.exists(), "the caller's own fresh cache was deleted");
        // …and it kept the NEWEST, not an arbitrary 16: the oldest must be gone and the
        // newest must survive. A prune that keeps the wrong 16 costs a source boot on
        // every binary in use, which is the cost it exists to avoid.
        assert!(!files[0].exists(), "kept the oldest file");
        assert!(files[files.len() - 1].exists(), "deleted the newest file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The age rule is still a floor: under the count cap, a long-dead build goes anyway.
    #[test]
    fn prune_still_drops_a_stale_file_under_the_count_cap() {
        let dir = std::env::temp_dir().join(format!("brood-prune-age-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = dir.join("prelude-expanded-0000000000000001.blsp");
        let old = dir.join("prelude-expanded-0000000000000002.blsp");
        std::fs::write(&fresh, b"f").unwrap();
        std::fs::write(&old, b"o").unwrap();
        let ancient = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        set_mtime(&old, ancient);
        let keep = dir.join("prelude-expanded-keep.blsp");
        std::fs::write(&keep, b"k").unwrap();

        boot_cache_prune(&dir, &keep);

        assert!(fresh.exists(), "a fresh file under the cap was deleted");
        assert!(!old.exists(), "a month-old build survived the age floor");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
