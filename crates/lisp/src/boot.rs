//! **Boot**: how a runtime comes to hold the prelude — the shared code region every
//! process reads and the global bindings that seed each one. Built once per OS process,
//! lazily, by whichever of three paths applies (fastest first): the **prelude image**
//! (ADR-314), the expanded-prelude **text cache** (ADR-138), or the full **source boot**
//! (read + expand + eval + freeze), which also writes the two artifacts for next time.
//! [`boot_source`] says which path THIS process took; the `Interp` in `lib.rs` only
//! seeds itself from [`SHARED`].

use std::sync::{Arc, LazyLock};

use crate::core::heap::{Heap, SharedCode};
use crate::core::value::{Symbol, Value};
use crate::{builtins, core, eval, syntax};

pub(crate) mod image; // the startup-image mechanism (ADR-218): sectioned stdlib image + the prelude image

/// The shared code region (prelude closures, code data, builtins) plus the
/// global bindings to seed each process's global env. Built once, lazily.
pub(crate) struct SharedBundle {
    pub(crate) code: Arc<SharedCode>,
    pub(crate) bindings: Vec<(Symbol, Value)>,
    /// The prelude's module-private names (ADR-146), captured from the builder heap
    /// where `%mark-private` recorded them. Seeds each live runtime's private set,
    /// since the prelude is inserted (not re-evaluated) and clean names can't be
    /// re-derived. Parallel to `bindings`.
    pub(crate) private: Vec<Symbol>,
    /// The prelude's stability metadata (ADR-283) — the `(meta …)` facts recorded in the
    /// builder heap. Carried for the same reason `private` is: the prelude is inserted
    /// into each live runtime rather than re-evaluated, so nothing re-runs
    /// `%register-meta` there. Parallel to `bindings`.
    pub(crate) meta: Vec<(Symbol, core::heap::NameMeta)>,
}

pub(crate) static SHARED: LazyLock<SharedBundle> = LazyLock::new(|| {
    // Fast path: boot from the expanded-prelude cache (ReadyToRun-lite). The
    // full source boot costs ~31 ms, ~27 ms of which is macro-EXPANSION of the
    // prelude (measured 2026-07-19; see the devlog) — parse, eval, and freeze
    // together are ~4 ms. So the cache stores the *post-compile* (expanded +
    // resolved + static-quasiquote) forms as plain text, keyed by `system/build-id`
    // (the prelude is `include_str!`'d, so any binary change invalidates), and
    // a warm boot skips `eval::macros::compile` entirely. Any mismatch or
    // failure falls back to the source boot, which rewrites the cache.
    if std::env::var_os("BROOD_NO_BOOT_CACHE").is_none() {
        // ADR-314: the prelude image rebuilds the bindings structurally, skipping the
        // read + eval the text cache still pays. Tried first; any miss falls through to
        // the text cache, and that to the source boot, so a bad artifact costs a slower
        // boot and never a wrong one.
        // DEFAULT ON since 2026-09-04 (ADR-314); `BROOD_NO_PRELUDE_IMAGE=1` opts out to the
        // text cache. Two earlier attempts to make it the default failed on the same day they
        // shipped, each on a fact the evaluation RECORDS rather than binds: KI-105 (a stale
        // stdlib section directory restored from the image — `%std-image-reinstall!` clears it
        // before installing) and KI-106 (the registry-name set was not carried, so a multi-file
        // `nest check` lost every derived multimethod mirror — the image writes and re-marks
        // it now). Both are fixed with sabotage-verified guards, the boot differential compares
        // the registry set, and `make check-imaged` runs the project's own checker gate with
        // the image on. Measured: `startup` -11%, no regression on 30 rows.
        if std::env::var_os("BROOD_NO_PRELUDE_IMAGE").is_none() {
            if let Some(bundle) = boot_from_prelude_image() {
                set_boot_source(BOOT_PRELUDE_IMAGE);
                return bundle;
            }
        }
        if let Some(bundle) = boot_from_cache() {
            set_boot_source(BOOT_TEXT_CACHE);
            return bundle;
        }
    }
    set_boot_source(BOOT_SOURCE);
    boot_from_source()
});

/// **The install's own bookkeeping** — globals that record which artifacts *this process*
/// loaded, rather than anything a module or the prelude defines.
///
/// They differ between an imaged and a source boot **by design**: that is the whole content
/// of these names, and item 2 of this area's cleanup made one of them (`%boot-source`) more
/// visible rather than less. A boot differential must therefore skip them, and the reason it
/// skips them matters: *because of what they are*, not because the arms disagree about them.
/// Excluding a global because two arms disagree is excluding the evidence — that is how
/// ADR-314's prelude differential passed with the bug in its own exclusion list, and how
/// `*std-image-installed*` sat in this set for a day while being exactly what it hid.
///
/// The facts these carry are asserted elsewhere, positively: `%boot-source` and the suite
/// summary line say which artifacts a run used, and `stdimage_reporting.rs` drives all three
/// states end to end. So they are covered, just not by equality.
///
/// One definition, in the runtime, because the tests that need it live in three different
/// crates and a per-crate copy is the "fixed in one file, left in two" failure this repo
/// already keeps a shared harness to avoid.
pub const INSTALL_BOOKKEEPING: &[&str] = &[
    "*image-sources*",
    "*image-path*",
    "*image-sections*",
    "*std-image-file*",
    "*std-image-sections*",
    "*std-image-installed*",
    "*std-impls*",
    "*std-regs*",
    "*std-require-edges*",
];

/// Which of the three boot paths actually ran, as an atomic so it can be read after the
/// fact from anywhere without threading it through the bundle.
///
/// **This exists because "was that run imaged?" had no answer, and guessing it wrong is the
/// single most expensive habit in this area.** Every rebuild changes `build-id`, so the
/// FIRST run after any build is a source boot and every run after it is not — which means a
/// developer verifying an image fix sees the un-imaged path exactly when they are least
/// expecting it. That has now corrupted the diagnosis of an image bug repeatedly: ADR-314
/// records it happening to a session already caught by it twice, and three separate "it is
/// fixed" readings during KI-106 were cold boots. `BROOD_BOOT_TRACE=1` could always show it,
/// but only to someone who armed it BEFORE the run and already suspected the answer. A fact
/// you must predict in order to observe is not a diagnostic.
// 0 is "not yet decided" — the shared prelude is built lazily, so a reader that beats it
// gets `unknown` rather than a plausible-looking lie.
static BOOT_SOURCE_KIND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const BOOT_PRELUDE_IMAGE: u8 = 1;
const BOOT_TEXT_CACHE: u8 = 2;
const BOOT_SOURCE: u8 = 3;

fn set_boot_source(kind: u8) {
    BOOT_SOURCE_KIND.store(kind, std::sync::atomic::Ordering::Relaxed);
}

/// How this process's prelude arrived: `"prelude-image"`, `"boot-cache"`, `"source"`, or
/// `"unknown"` before the shared prelude has been built. Read by `%boot-source`.
pub fn boot_source() -> &'static str {
    match BOOT_SOURCE_KIND.load(std::sync::atomic::Ordering::Relaxed) {
        BOOT_PRELUDE_IMAGE => "prelude-image",
        BOOT_TEXT_CACHE => "boot-cache",
        BOOT_SOURCE => "source",
        _ => "unknown",
    }
}

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
/// `MAX_KEEP` most recently modified BUILDS (plus `keep`'s) and delete the rest,
/// and separately drop anything older than `MAX_AGE`.
///
/// **A build, not a file — because a build now has two artifacts.** ADR-314 added
/// `prelude-expanded-<hash>.img` beside the `.blsp`, keyed identically
/// (`prelude_image_path` is the text cache's path `.with_extension("img")`). This
/// function matched on `.blsp` alone, so it pruned the text caches and left every
/// image behind, and the failure the count cap was written to fix came straight
/// back in the new artifact: **1057 `.img` files / 450 MB** measured on this repo's
/// dev machine on 2026-09-05, against 18 `.blsp` correctly held at the cap. Grouping
/// by file STEM fixes it for this pair and for the next one: a build is the unit,
/// every file sharing a stem lives or dies with it, and an artifact added later is
/// carried as soon as it is written beside its siblings.
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
    /// ~190 KB of text plus ~400 KB of image each, this bounds the prelude cache
    /// at ~9 MB rather than at whatever a week of rebuilding produces.
    const MAX_KEEP: usize = 16;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Skip `keep`'s whole BUILD, not just the one file: `keep` is this binary's own
    // freshly-written text cache, and its `.img` sibling shares the stem and is just
    // as live. Deleting the image out from under the binary that wrote it costs a
    // source boot for no reason.
    let keep_stem = keep.file_stem().map(|s| s.to_os_string());
    // stem -> (newest mtime among its files, all its files)
    let mut found: std::collections::HashMap<
        std::ffi::OsString,
        (std::time::SystemTime, Vec<std::path::PathBuf>),
    > = std::collections::HashMap::new();
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("prelude-expanded-") {
            continue;
        }
        if !(name.ends_with(".blsp") || name.ends_with(".img")) {
            continue;
        }
        let Some(stem) = p.file_stem().map(|s| s.to_os_string()) else {
            continue;
        };
        if keep_stem.as_ref() == Some(&stem) {
            continue;
        }
        let Ok(modified) = e.metadata().and_then(|m| m.modified()) else {
            // Unreadable metadata: treat as ancient so it sorts to the drop end
            // rather than occupying a keep slot it cannot justify.
            let _ = std::fs::remove_file(&p);
            continue;
        };
        let slot = found
            .entry(stem)
            .or_insert((modified, Vec::with_capacity(2)));
        // A build is as fresh as its freshest artifact: the image is written after the
        // text cache, so taking the older of the pair would age every build by the gap.
        if modified > slot.0 {
            slot.0 = modified;
        }
        slot.1.push(p);
    }
    // Newest build first, then every build past the cap goes, plus anything stale.
    let mut builds: Vec<(std::time::SystemTime, Vec<std::path::PathBuf>)> =
        found.into_values().collect();
    builds.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    for (i, (modified, paths)) in builds.iter().enumerate() {
        let stale = modified.elapsed().ok().is_some_and(|age| age > MAX_AGE);
        if i >= MAX_KEEP || stale {
            for p in paths {
                let _ = std::fs::remove_file(p);
            }
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

/// The prelude image for THIS binary, beside the text cache and keyed the same way.
fn prelude_image_path() -> Option<std::path::PathBuf> {
    let p = boot_cache_path()?;
    Some(p.with_extension("img"))
}

/// Boot the shared bundle by **materialising** the prelude's bindings (ADR-314) rather
/// than reading and evaluating its forms. `None` for any miss — absent, stale, truncated,
/// or a value that will not decode — and the caller falls back to the text cache.
///
/// The tail is identical to the other two paths: a builder heap with the natives
/// registered, the bindings installed, then `freeze_as_shared_code`. Only *how the
/// bindings arrive* differs, which is what keeps the three paths interchangeable.
fn boot_from_prelude_image() -> Option<SharedBundle> {
    // Stand aside under coverage, exactly as the stdlib image does (ADR-281): coverage
    // instruments the COMPILER, and a materialised binding is never compiled, so an imaged
    // prelude would report as uninstrumented and — worse — take the attribution machinery
    // down a path the source boot never takes. The text cache still evaluates real forms,
    // so falling through to it keeps coverage honest.
    if std::env::var_os("BROOD_COVERAGE").is_some() {
        return None;
    }
    let t_start = web_time::Instant::now();
    let path = prelude_image_path()?;
    let mut heap = Heap::new();
    let root = heap.new_env(None);
    heap.set_global(root);
    builtins::register(&mut heap, root);
    let t_register = t_start.elapsed();
    // Parity with the text path: materialise the on-disk prelude copy the def sites name,
    // so stdlib `M-.` can actually open the file the image points at.
    heap.set_current_file(prelude_source_path());
    let n = image::load_prelude_image(&mut heap, root, &path, &builtins::build_id_string())?;
    heap.set_current_file(None);
    // REPLAY what the prelude's evaluation DID, not just what it recorded. The prelude has a
    // top-level form that runs `%std-image-install`, and the imaged path never evaluates it —
    // so without this the boot restores a *snapshot* of a previous install: `*image-sources*`
    // comes back holding the section directory of whatever stdlib image existed when this
    // prelude image was written. That file is keyed on `stdlib-id`, so a rebuild with
    // different module coverage (a lean `nest` vs a full one — exactly what
    // `scripts/build-std-image.sh` can do) reuses the same PATH with different offsets, and
    // every section read then lands on garbage. Observed as `unbound symbol: io/puts` on a
    // tree where nothing was wrong with `io`.
    //
    // Re-running the install is the faithful replay: it is what the source and text paths do
    // at this point, and it overwrites the stale directory with the current one.
    //
    // The trace line is emitted HERE rather than left to the prelude's own `when` form: that
    // form is Brood, this replay is Rust, and with the image default-on the Brood one never
    // runs. `BROOD_IMAGE_TRACE` is documented as the thing to reach for before believing any
    // image measurement, so a default boot that silently stops printing its install line
    // would take the diagnostic down on exactly the path everyone now uses.
    let trace = std::env::var_os("BROOD_IMAGE_TRACE").is_some();
    let t_install = web_time::Instant::now();
    let install = syntax::reader::read_all(&mut heap, "(%std-image-reinstall!)").ok()?;
    let mut sections = Value::Nil;
    for form in install {
        if let Ok(v) = eval::eval(&mut heap, form, root) {
            sections = v;
        }
    }
    if trace {
        eprintln!(
            "[image] install: {} sections, {} ns (replayed under the prelude image)",
            crate::syntax::printer::display(&heap, sections),
            t_install.elapsed().as_nanos()
        );
    }
    let t_load = t_start.elapsed();
    // The image carries no gensym counter of its own: the names baked into its closures
    // were minted by the caching boot, so the floor the text cache records applies here
    // too. Read it from that file's header if it is present; a missing one only means a
    // runtime `gensym` starts lower, which is safe (it can still never collide, because
    // the image's own names are already interned).
    if let Some(text_path) = boot_cache_path() {
        if let Ok(head) = std::fs::read_to_string(&text_path) {
            if let Some(line) = head.lines().next() {
                if let Some(rest) = line.strip_prefix(&boot_cache_header_prefix()) {
                    if let Ok(g) = rest.trim().parse::<u64>() {
                        core::value::gensym_floor(g);
                    }
                }
            }
        }
    }
    let private = heap.private_names_snapshot();
    let name_meta = heap.name_meta_snapshot();
    let t_pre_freeze = t_start.elapsed();
    let (code, bindings) = heap.freeze_as_shared_code(root);
    if std::env::var_os("BROOD_BOOT_TRACE").is_some() {
        eprintln!(
            "[boot] register={:?} image={:?} ({n} entries) freeze={:?} — total={:?} (prelude image)",
            t_register,
            t_load - t_register,
            t_start.elapsed() - t_pre_freeze,
            t_start.elapsed()
        );
    }
    Some(SharedBundle {
        code: Arc::new(code),
        bindings,
        private,
        meta: name_meta,
    })
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
    // Snapshot what registration alone bound: the prelude image may omit exactly these,
    // because the warm path calls `register` too. Taken here rather than derived from the
    // values later — "is a native" is not the same question (a prelude `def` can bind one).
    let builtin_names: std::collections::HashSet<core::value::Symbol> =
        heap.env_chain_names(root).into_iter().collect();
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
    // ADR-314: write the prelude image before the freeze consumes the builder heap. The
    // cold boot is allowed to be slow — it runs once per binary build — so this is pure
    // addition; every later boot skips the read+eval this one just did. Best-effort: a
    // failure leaves the next boot on the text-cache path, which is where it was before.
    if cache_ok {
        if let Some(img) = prelude_image_path() {
            let _ = image::write_prelude_image(
                &mut heap,
                root,
                &builtin_names,
                &img,
                &builtins::build_id_string(),
            );
        }
    }
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
pub(crate) fn prelude_source_path() -> Option<String> {
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
    use crate::Interp;

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
            // Every build writes BOTH artifacts (ADR-314), so the fixture must too —
            // seeding only the text cache is what let the image leak go unnoticed.
            let img = p.with_extension("img");
            std::fs::File::create(&img)
                .unwrap()
                .write_all(b"i")
                .unwrap();
            set_mtime(&img, std::time::SystemTime::now());
            // Stamp mtimes explicitly rather than relying on creation order: the
            // filesystem's timestamp granularity is coarse enough that files written in
            // one loop can share an mtime, which would make the ordering assertion
            // below pass or fail by luck.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs((n - i) as u64);
            set_mtime(&p, t);
            set_mtime(&img, t);
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
        count_ext(dir, ".blsp")
    }

    /// Files with `ext` left in `dir`. Counting `.img` separately is the point: the
    /// prune matched `.blsp` alone for as long as the image existed, so the text caches
    /// were bounded and the images grew without limit — 1057 files / 450 MB when found.
    fn count_ext(dir: &std::path::Path, ext: &str) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.starts_with("prelude-expanded-") && n.ends_with(ext)
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
        let keep_img = keep.with_extension("img");
        std::fs::write(&keep_img, b"k").unwrap();

        boot_cache_prune(&dir, &keep);

        // 16 kept by the cap + `keep` itself, which is never a candidate.
        assert_eq!(remaining(&dir), 17, "count cap did not bound the directory");
        assert!(keep.exists(), "the caller's own fresh cache was deleted");
        // …and it kept the NEWEST, not an arbitrary 16: the oldest must be gone and the
        // newest must survive. A prune that keeps the wrong 16 costs a source boot on
        // every binary in use, which is the cost it exists to avoid.
        assert!(!files[0].exists(), "kept the oldest file");
        assert!(files[files.len() - 1].exists(), "deleted the newest file");
        // The image half is bounded by the same cap. Before the stem grouping this read
        // 41: every image survived because the prune only ever matched `.blsp`.
        assert_eq!(
            count_ext(&dir, ".img"),
            17,
            "the prelude IMAGES were not bounded — the artifact the count cap forgot"
        );
        // A dropped build takes BOTH its files, never one: an orphaned image is dead
        // weight no boot will ever read, since its text-cache sibling is gone.
        assert!(
            !files[0].with_extension("img").exists(),
            "dropped a build's text cache but kept its image"
        );
        // …and the caller's own build keeps both halves.
        assert!(
            keep_img.exists(),
            "deleted the image belonging to the caller's own fresh cache"
        );

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
        std::fs::write(fresh.with_extension("img"), b"f").unwrap();
        std::fs::write(old.with_extension("img"), b"o").unwrap();
        let ancient = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        set_mtime(&old, ancient);
        set_mtime(&old.with_extension("img"), ancient);
        let keep = dir.join("prelude-expanded-keep.blsp");
        std::fs::write(&keep, b"k").unwrap();

        boot_cache_prune(&dir, &keep);

        assert!(fresh.exists(), "a fresh file under the cap was deleted");
        assert!(!old.exists(), "a month-old build survived the age floor");
        assert!(
            !old.with_extension("img").exists(),
            "the stale build's image outlived its text cache"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
