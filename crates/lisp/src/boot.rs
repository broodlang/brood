//! **Boot**: how a runtime comes to hold the prelude — the shared code region every
//! process reads and the global bindings that seed each one. Built once per OS process,
//! lazily, by one of two paths: the **prelude image** (ADR-314), which materialises the
//! prelude's bindings from disk, or the full **source boot** (read + expand + eval +
//! freeze), which also writes the image for next time. [`boot_source`] says which path
//! THIS process took; the `Interp` in `lib.rs` only seeds itself from [`SHARED`].

use std::sync::{Arc, LazyLock};

use crate::core::heap::{Heap, SharedCode};
use crate::core::value::{Symbol, Value};
use crate::{builtins, core, eval, syntax};

pub(crate) mod image; // the startup-image mechanism (ADR-218): sectioned stdlib image + the prelude image
pub use image::{MEMORY_IMAGE_PREFIX, memory_image, provide_memory_image};

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
    /// The prelude's `defdyn` marks, captured from the builder heap for the reason
    /// `private` is: each live runtime owns its own dynamic set (KI-224) and the
    /// prelude's `defdyn`s never re-run there.
    pub(crate) dynamics: Vec<Symbol>,
    /// The kind index (modules, macro names) of the stdlib image the builder heap installed.
    /// Each live runtime owns its index (KI-226), and the boot's install never re-runs
    /// there, so it is seeded from this.
    pub(crate) image_kinds: (Vec<Symbol>, Vec<Symbol>),
    /// The prelude's stability metadata (ADR-283) — the `(meta …)` facts recorded in the
    /// builder heap. Carried for the same reason `private` is: the prelude is inserted
    /// into each live runtime rather than re-evaluated, so nothing re-runs
    /// `%register-meta` there. Parallel to `bindings`.
    pub(crate) meta: Vec<(Symbol, core::heap::NameMeta)>,
}

pub(crate) static SHARED: LazyLock<SharedBundle> = LazyLock::new(|| {
    // The prelude image (ADR-314) materialises the prelude's bindings structurally from
    // `~/.cache/brood/prelude-expanded-<build-id-hash>.img`; any miss — no file, a
    // different build, a torn or undecodable artifact, a failed replay — falls through to
    // the full source boot, which rewrites the image. A bad artifact therefore costs a
    // slower boot and never a wrong one. `BROOD_NO_PRELUDE_IMAGE=1` neither reads nor
    // writes it.
    //
    // The trap is a fact the evaluation RECORDS rather than binds (KI-105, KI-106): the
    // image restores bindings only, so every such fact (the gensym floor, the registry
    // set, …) is carried explicitly, and `prelude_image_matches_source.rs` compares image
    // against source.
    if std::env::var_os("BROOD_NO_PRELUDE_IMAGE").is_none()
        && let Some(bundle) = boot_from_prelude_image()
    {
        set_boot_source(BOOT_PRELUDE_IMAGE);
        return bundle;
    }
    set_boot_source(BOOT_SOURCE);
    boot_from_source()
});

/// A prelude image the host handed over (`provide_prelude_image`), taken by the first boot.
static PROVIDED_PRELUDE_IMAGE: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);
/// Set by `capture_prelude_image`: the source boot keeps the image bytes it would write.
static CAPTURE_PRELUDE_IMAGE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static CAPTURED_PRELUDE_IMAGE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// Boot the prelude from `bytes` — a prelude image this same build wrote — instead of from a
/// file in the cache directory: for a host that has no cache directory, a browser tab,
/// which ships the image beside the program. Call before the first runtime is made. An
/// image another build wrote fails its fingerprint and the boot falls back to the source,
/// as a stale file does.
pub fn provide_prelude_image(bytes: Vec<u8>) {
    *crate::core::sync::lock(&PROVIDED_PRELUDE_IMAGE) = Some(bytes);
}

fn take_provided_prelude_image() -> Option<Vec<u8>> {
    crate::core::sync::lock(&PROVIDED_PRELUDE_IMAGE).take()
}

/// Boot the prelude from source and answer the prelude image that boot produces, for a
/// build that ships it (`provide_prelude_image`). `None` when the prelude was already booted
/// before this was asked, or the encoding failed.
pub fn capture_prelude_image() -> Option<Vec<u8>> {
    CAPTURE_PRELUDE_IMAGE.store(true, std::sync::atomic::Ordering::Release);
    std::sync::LazyLock::force(&SHARED);
    CAPTURED_PRELUDE_IMAGE.get().cloned()
}

/// **The install's own bookkeeping** — globals that record which artifacts *this process*
/// loaded, rather than anything a module or the prelude defines.
///
/// They differ between an imaged and a source boot **by design**, so a boot differential
/// skips them — *because of what they are*, never merely because two arms disagree about
/// them (excluding a disagreement is excluding the evidence). The facts they carry are
/// asserted positively elsewhere: `%boot-source`, the suite summary line, and
/// `stdimage_reporting.rs`. One definition here because tests in three crates need it.
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

/// Which of the two boot paths actually ran, as an atomic so it can be read after the
/// fact from anywhere without threading it through the bundle.
///
/// It answers "was that run imaged?" after the fact: every rebuild changes `build-id`, so
/// the FIRST run after any build is a source boot and every later one is not, and a
/// developer verifying an image fix sees the un-imaged path exactly when least expecting it.
// 0 is "not yet decided" — the shared prelude is built lazily, so a reader that beats it
// gets `unknown` rather than a plausible-looking lie.
static BOOT_SOURCE_KIND: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const BOOT_PRELUDE_IMAGE: u8 = 1;
const BOOT_SOURCE: u8 = 2;

fn set_boot_source(kind: u8) {
    BOOT_SOURCE_KIND.store(kind, std::sync::atomic::Ordering::Relaxed);
}

/// How this process's prelude arrived: `"prelude-image"`, `"source"`, or `"unknown"`
/// before the shared prelude has been built. Read by `%boot-source`.
pub fn boot_source() -> &'static str {
    match BOOT_SOURCE_KIND.load(std::sync::atomic::Ordering::Relaxed) {
        BOOT_PRELUDE_IMAGE => "prelude-image",
        BOOT_SOURCE => "source",
        _ => "unknown",
    }
}

/// The prelude image for THIS binary:
/// `~/.cache/brood/prelude-expanded-<hash-of-build-id>.img`. Per-binary, because the
/// prelude is `include_str!`'d and any binary change invalidates it; the fingerprint
/// inside the file is checked too, so a hash collision cannot serve another build's image.
fn prelude_image_path() -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    builtins::build_id_string().hash(&mut h);
    Some(cache_dir()?.join(format!("prelude-expanded-{:016x}.img", h.finish())))
}

/// Brood's cache directory: `$XDG_CACHE_HOME/brood`, else `$HOME/.cache/brood`; `None`
/// when neither gives one. A relative or empty `XDG_CACHE_HOME` is ignored, as the XDG
/// spec says, and an empty `HOME` gives nothing.
///
/// The one rule for every cache Brood writes — the prelude image, its source copy, the
/// run-check cache, `nest release`'s runtimes — and `std/tool/stdimage.blsp`'s
/// `cache-dir` mirrors it. Five hand copies disagreed on relative and empty values, so
/// one process could look for the stdlib image where another never wrote it.
pub fn cache_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let set = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    set("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|base| base.join("brood"))
}

/// Best-effort prune of OTHER builds' prelude images: keep the `MAX_KEEP` most recent,
/// drop anything older than `MAX_AGE`, never touch `keep` (the one just written).
///
/// Bounded by COUNT and not only by age, for the reason the stdlib image's `prune` records:
/// on a machine that is editing the standard library — the only kind that writes these —
/// every rebuild mints a new file and nothing is ever old enough for an age rule to catch.
fn prelude_image_prune(dir: &std::path::Path, keep: &std::path::Path) {
    prune_cache_entries(dir, keep, |name| {
        name.starts_with("prelude-expanded-") && name.ends_with(".img")
    });
}

/// The count-and-age prune [`prelude_image_prune`] describes, over the entries of `dir`
/// whose name `matches` — a file or a directory — never touching `keep`.
fn prune_cache_entries(
    dir: &std::path::Path,
    keep: &std::path::Path,
    matches: impl Fn(&str) -> bool,
) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    const MAX_KEEP: usize = 16;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !matches(&name) || path == keep {
            continue;
        }
        match entry.metadata().and_then(|metadata| metadata.modified()) {
            Ok(modified) => others.push((modified, path)),
            // Unreadable metadata: not worth keeping around.
            Err(_) => remove_cache_entry(&path),
        }
    }
    others.sort_unstable_by_key(|other| std::cmp::Reverse(other.0));
    for (rank, (modified, path)) in others.iter().enumerate() {
        let stale = modified.elapsed().ok().is_some_and(|age| age > MAX_AGE);
        if rank >= MAX_KEEP || stale {
            remove_cache_entry(path);
        }
    }
}

fn remove_cache_entry(path: &std::path::Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

/// Run the prelude's `(%std-image-reinstall!)` in the builder heap, returning its result
/// (the installed section count). `None` if reading or evaluating it fails — the imaged
/// boot then falls back to the source boot rather than continue without the replay, which
/// would leave a stale stdlib section directory installed (KI-105).
fn replay_std_image_install(heap: &mut Heap, root: core::value::EnvId) -> Option<Value> {
    let forms = syntax::reader::read_all(heap, "(%std-image-reinstall!)").ok()?;
    let mut sections = Value::Nil;
    for form in forms {
        sections = eval::eval(heap, form, root).ok()?;
    }
    Some(sections)
}

/// Boot the shared bundle by **materialising** the prelude's bindings (ADR-314) rather
/// than reading and evaluating its forms. `None` for any miss — absent, stale, truncated,
/// a value that will not decode, or a failed install replay — and the caller falls back to
/// the source boot.
///
/// Both ends are shared with the source boot — [`builder_heap`] and [`finish_bundle`] — so
/// only *how the bindings arrive* differs, which is what keeps the two paths interchangeable.
fn boot_from_prelude_image() -> Option<SharedBundle> {
    // Stand aside under coverage, exactly as the stdlib image does (ADR-281): coverage
    // instruments the COMPILER, and a materialised binding is never compiled, so an imaged
    // prelude would report as uninstrumented. The source boot evaluates real forms.
    if std::env::var_os("BROOD_COVERAGE").is_some() {
        return None;
    }
    let started = web_time::Instant::now();
    let bytes = match take_provided_prelude_image() {
        Some(bytes) => bytes,
        None => std::fs::read(prelude_image_path()?).ok()?,
    };
    let (mut heap, root) = builder_heap();
    let registered = started.elapsed();
    // Like the source boot: materialise the on-disk prelude copy the def sites name, so
    // stdlib `M-.` can actually open the file the image points at.
    heap.set_current_file(prelude_source_path());
    let (entries, gensym_floor) =
        image::load_prelude_image_bytes(&mut heap, root, bytes, &builtins::build_id_string())?;
    heap.set_current_file(None);
    // REPLAY what the prelude's evaluation DID, not just what it recorded. The prelude has a
    // top-level form that runs `%std-image-install`, and the imaged path never evaluates it —
    // so without this `*image-sources*` comes back holding the section directory of whatever
    // stdlib image existed when this prelude image was written. That file is keyed on
    // `stdlib-id`, so a rebuild with different module coverage reuses the same PATH with
    // different offsets, and every section read lands on garbage (KI-105). The trace line is
    // emitted here because the prelude's own Brood `when` form never runs on this path.
    let trace = crate::diagnostics::debug_flags::image_trace();
    let install_started = web_time::Instant::now();
    let sections = replay_std_image_install(&mut heap, root)?;
    if trace {
        eprintln!(
            "[image] install: {} sections, {} ns (replayed under the prelude image)",
            crate::syntax::printer::display(&heap, sections),
            install_started.elapsed().as_nanos()
        );
    }
    let loaded = started.elapsed();
    // The names baked into the image's closures were minted by the boot that wrote it, so
    // a runtime `gensym` must start ABOVE that counter or a fresh name could collide with a
    // baked one. The image carries the floor in its header.
    core::value::gensym_floor(gensym_floor);
    let before_freeze = started.elapsed();
    let bundle = finish_bundle(heap, root);
    if crate::diagnostics::debug_flags::boot_trace() {
        eprintln!(
            "[boot] register={:?} image={:?} ({entries} entries) freeze={:?} — total={:?} (prelude image)",
            registered,
            loaded - registered,
            started.elapsed() - before_freeze,
            started.elapsed()
        );
    }
    Some(bundle)
}

/// The throwaway heap both boot paths build the prelude in: a fresh root env, made the
/// global one, with every native registered.
fn builder_heap() -> (Heap, core::value::EnvId) {
    let mut heap = Heap::new();
    let root = heap.new_env(None);
    heap.set_global(root);
    builtins::register(&mut heap, root);
    (heap, root)
}

/// Snapshot the builder heap's name tables and freeze it into the shared bundle — the end
/// of both boot paths, so a field added to [`SharedBundle`] is filled once, the same way
/// for each (the `meta` field was once added to one path only).
fn finish_bundle(heap: Heap, root: core::value::EnvId) -> SharedBundle {
    let private = heap.private_names_snapshot();
    let dynamics = heap.dynamic_names();
    let image_kinds = heap.image_kinds_snapshot();
    let meta = heap.name_meta_snapshot();
    let (code, bindings) = heap.freeze_as_shared_code(root);
    SharedBundle {
        code: Arc::new(code),
        bindings,
        private,
        dynamics,
        image_kinds,
        meta,
    }
}

/// The full source boot: parse + macro-expand + eval + freeze the prelude,
/// then (best-effort) write the prelude image for the next boot.
fn boot_from_source() -> SharedBundle {
    let t_start = web_time::Instant::now();
    // Build the prelude + builtins in a throwaway builder heap, then relocate it
    // all into the shared region. Done once for the whole process.
    let (mut heap, root) = builder_heap();
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
    let write_image = std::env::var_os("BROOD_NO_PRELUDE_IMAGE").is_none();
    for (form, pos) in forms {
        // Record definition sites from the raw form first — catches `defn`/`defmacro`
        // before lowering discards their source positions — and again from the expanded
        // form below, so a def-like macro (`defseq`) whose expansion IS a `defn` still
        // gets its call-site position. Both are no-ops for a non-definition or no file.
        heap.note_definition(form, pos);
        // Compile pass (expand macros, then namespace-resolve — a no-op here since
        // the prelude is the root namespace), then evaluate. Form-by-form so a
        // macro defined by one form is visible to the next.
        let t_e = web_time::Instant::now();
        let form = eval::macros::compile(&mut heap, form, root)
            // Name the form: a prelude that fails to expand is a prelude being edited, and
            // "unbound symbol: x" without the form that reached `x` is a bisection.
            .unwrap_or_else(|e| panic!("prelude expand: {} (form at {:?})", e, pos));
        let d = t_e.elapsed();
        if d.as_micros() > 300 && crate::diagnostics::debug_flags::boot_trace() {
            eprintln!("[boot-form] {:?} at {:?}", d, pos);
        }
        t_expand += d;
        heap.note_definition(form, pos);
        eval::eval(&mut heap, form, root).unwrap_or_else(|e| panic!("prelude: {}", e));
    }
    heap.set_current_file(None);
    let t_eval = t_mark.elapsed();
    let t_mark = web_time::Instant::now();
    // ADR-314: write the prelude image before the freeze consumes the builder heap. The
    // cold boot is allowed to be slow — it runs once per binary build — so this is pure
    // addition; every later boot skips the read+eval this one just did. Best-effort: a
    // failure leaves the next boot on the source path, which is where it was before.
    if CAPTURE_PRELUDE_IMAGE.load(std::sync::atomic::Ordering::Acquire) {
        let image = image::prelude_image_bytes(
            &mut heap,
            root,
            &builtin_names,
            &builtins::build_id_string(),
            core::value::gensym_counter(),
        );
        if let Ok(image) = image {
            let _ = CAPTURED_PRELUDE_IMAGE.set(image);
        }
    }
    if write_image && let Some(image_path) = prelude_image_path() {
        let _ = image::write_prelude_image(
            &mut heap,
            root,
            &builtin_names,
            &image_path,
            &builtins::build_id_string(),
            core::value::gensym_counter(),
        )
        .map(|()| {
            image_path
                .parent()
                .map(|directory| prelude_image_prune(directory, &image_path))
        });
    }
    let bundle = finish_bundle(heap, root);
    let t_freeze = t_mark.elapsed();
    if crate::diagnostics::debug_flags::boot_trace() {
        eprintln!(
            "[boot] builtins={:?} read={:?} expand={:?} eval={:?} freeze={:?} total={:?} (source boot{})",
            t_builtins,
            t_read,
            t_expand,
            t_eval - t_expand,
            t_freeze,
            t_start.elapsed(),
            if write_image { ", image written" } else { "" }
        );
    }
    bundle
}

/// The standard prelude, written in Brood and baked into the binary. Split across
/// `std/prelude/*.blsp` for navigability and concatenated **in order** here — the pieces are
/// bare-root prelude source, so evaluation order is load-bearing (macros before use, forward
/// references). **This list is the authoritative order**; a new prelude file must be added at
/// the right position here.
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
    include_str!("../../../std/prelude/modules.blsp"),
    include_str!("../../../std/prelude/abilities.blsp"),
    // Installs the stdlib image: must follow everything a `require` could need.
    include_str!("../../../std/prelude/image-install.blsp"),
    // Runtime contracts (ADR-381) come LAST among the prelude files: a prelude closure whose
    // body calls a macro defined after it is deferred to the tree-walker for good, and every
    // contract check runs through this machinery.
    include_str!("../../../std/prelude/contracts.blsp"),
    // Behaviour contracts are CORE (defbehaviour / %register-protocol / ops / *protocols*).
    // After modules.blsp, which defines the `swap-registry!` macro protocol uses.
    include_str!("../../../std/protocol.blsp"),
);

/// The repo-relative paths of the files [`PRELUDE`] concatenates, in the same order —
/// pinned to it by `prelude_paths_match_the_concatenation`. The checker reads it to know
/// that a `def` in one of these files is the definition of a reserved name, not a
/// redefinition of it.
pub const PRELUDE_PATHS: &[&str] = &[
    "std/prelude/core.blsp",
    "std/prelude/predicates.blsp",
    "std/prelude/map.blsp",
    "std/prelude/control.blsp",
    "std/prelude/match.blsp",
    "std/prelude/process.blsp",
    "std/prelude/seq.blsp",
    "std/prelude/string.blsp",
    "std/prelude/tools.blsp",
    "std/prelude/modules.blsp",
    "std/prelude/abilities.blsp",
    "std/prelude/image-install.blsp",
    "std/prelude/contracts.blsp",
    "std/protocol.blsp",
];

/// Is `file` one of the prelude's own source files? Compared by trailing path
/// components, so an absolute path into any checkout matches.
pub fn is_prelude_source(file: &str) -> bool {
    let file = std::path::Path::new(file);
    PRELUDE_PATHS
        .iter()
        .any(|relative| file.ends_with(relative))
}

/// Materialize the embedded prelude to a stable, read-only-ish cache file and
/// return its path — the file the prelude's def-sites point at, so tools (the
/// LSP's `M-.`) can open the standard library's source. The prelude is
/// `include_str!`'d, so it has no source file at runtime; this writes one copy
/// to `$XDG_CACHE_HOME/brood/prelude-source-<hash>/prelude.blsp` (falling back to
/// `~/.cache`), where `<hash>` is the prelude TEXT's. Editing it has no effect — it's
/// a navigation artefact, not a load path.
///
/// Keyed by content because every Brood on the machine shares the cache: one
/// unkeyed `prelude.blsp` was rewritten by whichever binary ran last, so a binary
/// whose def-sites point into it read another prelude's lines. The doc catalogue
/// maps those sites onto section headers and filed `match` under `case` while an
/// older build ran beside a newer one (2026-11-03). A keyed path names one text
/// for good.
///
/// A cache dir that cannot be written (a read-only `HOME` in a container) falls
/// back to the system temp dir: the doc catalogue derives categories from these
/// definition sites, so a missing file would empty the reference, not just
/// navigation. Returns `None` only if neither can be written; the caller treats
/// that as "stdlib navigation unavailable" and carries on.
pub(crate) fn prelude_source_path() -> Option<String> {
    cache_dir()
        .and_then(|dir| write_prelude_copy(&dir))
        .or_else(temp_dir_prelude_copy)
}

#[cfg(not(target_arch = "wasm32"))]
fn temp_dir_prelude_copy() -> Option<String> {
    write_prelude_copy(&std::env::temp_dir().join("brood"))
}

/// The browser playground has no filesystem, and `std::env::temp_dir` panics there.
#[cfg(target_arch = "wasm32")]
fn temp_dir_prelude_copy() -> Option<String> {
    None
}

fn write_prelude_copy(dir: &std::path::Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    PRELUDE.hash(&mut h);
    let own = dir.join(format!("prelude-source-{:016x}", h.finish()));
    std::fs::create_dir_all(&own).ok()?;
    let path = own.join("prelude.blsp");
    // Write when absent, or when a partial write (a killed process) left it short:
    // the name fixes the content, so a copy that matches is never written again.
    let complete = std::fs::read(&path).is_ok_and(|existing| existing == PRELUDE.as_bytes());
    if !complete {
        // Write beside it and rename into place, so a reader never sees half a file.
        let partial = own.join(format!("prelude.blsp.{}", std::process::id()));
        std::fs::write(&partial, PRELUDE).ok()?;
        std::fs::rename(&partial, &path).ok()?;
        prune_cache_entries(dir, &own, |name| name.starts_with("prelude-source-"));
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
    use crate::Interp;
    use crate::core::value::{self, ValueRef};

    #[test]
    fn prelude_paths_match_the_concatenation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let concatenated: String = PRELUDE_PATHS
            .iter()
            .map(|relative| {
                std::fs::read_to_string(root.join(relative))
                    .unwrap_or_else(|error| panic!("read {relative}: {error}"))
            })
            .collect();
        assert!(
            concatenated == PRELUDE,
            "PRELUDE_PATHS no longer lists the files PRELUDE concatenates, in order"
        );
        assert!(is_prelude_source("/any/checkout/std/prelude/core.blsp"));
        assert!(is_prelude_source("std/protocol.blsp"));
        assert!(!is_prelude_source("/project/std/prelude/core.blsp.bak"));
        assert!(!is_prelude_source("/project/src/core.blsp"));
    }

    // There is no allowed-module list: since ADR-335 a qualified reference loads its module
    // at the first lookup that misses. What the lint checks: every qualified name the
    // prelude references is either bound at boot (a registered primitive, a prelude
    // definition) or names a module this binary embeds — so a typo cannot hide behind
    // "loads later".

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
                if let ValueRef::Sym(s) = car.unpack()
                    && value::symbol_name(s) == "quote"
                {
                    return;
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

    #[test]
    fn prelude_code_references_no_unloaded_module_wrapper() {
        // Every prelude file, from the one authoritative list, so a file added there is
        // checked here without a second list to keep in step.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let sources: Vec<(&str, String)> = PRELUDE_PATHS
            .iter()
            .map(|path| {
                let src = std::fs::read_to_string(root.join(path))
                    .unwrap_or_else(|e| panic!("read {path}: {e}"));
                (*path, src)
            })
            .collect();
        let primitives = registered_primitives();
        let mut heap = Heap::new();
        // Two passes: the whole prelude's definitions have to be
        // known before any file's references can be judged, since a reference in `core.blsp`
        // may name something `abilities.blsp` declares.
        let read: Vec<(&str, Vec<Value>)> = sources
            .iter()
            .map(|(fname, src)| {
                let forms = syntax::reader::read_all(&mut heap, src)
                    .unwrap_or_else(|e| panic!("read {fname}: {e:?}"));
                (*fname, forms)
            })
            .collect();
        let mut defined = std::collections::HashSet::new();
        for (_, forms) in &read {
            defined.extend(prelude_definitions(&heap, forms));
        }
        let mut violations: Vec<String> = Vec::new();
        for (fname, forms) in &read {
            for &form in forms {
                let mut found = Vec::new();
                collect_qualified(&heap, form, &mut found);
                for q in found {
                    // Two ways a qualified name is bound with no module load: a slash-named
                    // kernel primitive (`file/slurp`, `string/split`) and a prelude definition
                    // (`string/format` lives in the prelude, not in the `string` module).
                    if primitives.contains(&q) || defined.contains(&q) {
                        continue;
                    }
                    // Everything else loads at the first lookup that misses (ADR-335) — which
                    // is only a load if the module EXISTS. A typo (`strng/join`) or a module
                    // that was renamed out from under the prelude would surface as `unbound
                    // symbol` at a user's call, exactly the failure this lint is for; require
                    // the module to be one this binary embeds.
                    let module = &q[..q.rfind('/').unwrap_or(0)];
                    if crate::builtins::modules::is_embedded_module(module) {
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
            "prelude code references a qualified name whose module this binary does not embed, \
             and nothing binds it at boot. A reference into a real std module loads that module \
             on first use (ADR-335); this is none of those — a typo, or a module that moved. \
             Reach for the `%`-primitive, or fix the name:\n  {}",
            violations.join("\n  ")
        );
    }
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
                 `defmulti` in std/prelude/abilities.blsp and that table have drifted apart"
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
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    /// The imaged boot's install replay must FAIL the imaged boot when the replay itself
    /// fails, so `boot_from_prelude_image` (which `?`s it) falls back to the source boot
    /// rather than run on a stale section directory. A builder heap whose bindings lack
    /// `%std-image-reinstall!` (natives only, no prelude) is that failure on demand.
    #[test]
    fn a_failed_install_replay_fails_the_imaged_boot() {
        let (mut heap, root) = builder_heap();
        assert!(
            replay_std_image_install(&mut heap, root).is_none(),
            "an install replay that raised was treated as a successful boot step"
        );
    }
}

#[cfg(test)]
mod prelude_copy_tests {
    use super::*;

    /// The copy is named by its text, so a cache shared with another build can never hand
    /// this binary's def-sites someone else's lines; a copy cut short (a killed writer) at
    /// this build's name is completed, not trusted.
    #[test]
    fn the_prelude_copy_is_named_by_its_text_and_completed_if_short() {
        let dir = std::env::temp_dir().join(format!("brood-prelude-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Another build's unkeyed copy, as older binaries still write it: never read.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prelude.blsp"), "(an older prelude)").unwrap();
        let path = write_prelude_copy(&dir).expect("a writable cache gets a copy");
        assert!(
            path.contains("prelude-source-"),
            "the copy is not keyed: {path}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PRELUDE);
        std::fs::write(&path, &PRELUDE[..100]).unwrap();
        assert_eq!(write_prelude_copy(&dir).as_deref(), Some(path.as_str()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PRELUDE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod prelude_image_prune_tests {
    use super::prelude_image_prune;

    fn seed(dir: &std::path::Path, n: usize) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for i in 0..n {
            let p = dir.join(format!("prelude-expanded-{i:016x}.img"));
            std::fs::write(&p, b"i").unwrap();
            // Stamp mtimes explicitly: filesystem timestamp granularity is coarse enough
            // that files written in a tight loop can share one, and the prune's ordering
            // is what these tests assert.
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs((n - i) as u64);
            set_mtime(&p, t);
            out.push(p);
        }
        out
    }
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
                n.starts_with("prelude-expanded-") && n.ends_with(".img")
            })
            .count()
    }

    #[test]
    fn prune_bounds_the_images_by_count_not_only_by_age() {
        let dir = std::env::temp_dir().join(format!("brood-prune-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = seed(&dir, 40);
        let keep = dir.join("prelude-expanded-keep.img");
        std::fs::write(&keep, b"k").unwrap();
        prelude_image_prune(&dir, &keep);
        assert_eq!(remaining(&dir), 17, "count cap did not bound the directory");
        assert!(keep.exists(), "the caller's own fresh image was deleted");
        assert!(!files[0].exists(), "kept the oldest image");
        assert!(files[files.len() - 1].exists(), "deleted the newest image");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_still_drops_a_stale_image_under_the_count_cap() {
        let dir = std::env::temp_dir().join(format!("brood-prune-age-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fresh = dir.join("prelude-expanded-0000000000000001.img");
        let old = dir.join("prelude-expanded-0000000000000002.img");
        std::fs::write(&fresh, b"f").unwrap();
        std::fs::write(&old, b"o").unwrap();
        let ancient = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        set_mtime(&old, ancient);
        let keep = dir.join("prelude-expanded-keep.img");
        std::fs::write(&keep, b"k").unwrap();
        prelude_image_prune(&dir, &keep);
        assert!(fresh.exists(), "a fresh image under the cap was deleted");
        assert!(
            !old.exists(),
            "a month-old build's image survived the age floor"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod builder_scope_tests {
    use super::*;

    /// The source boot compiles every prelude form on the BUILDER heap, whose global scope
    /// is a frame, not the shared table. ADR-423's templates expand to `/when`-style heads,
    /// and `compile` must expand them there: asked of `EnvId::GLOBAL`, `macro_head_id` missed
    /// the builder's definitions, every prelude function using a prelude macro kept a raw
    /// call, was deferred to the tree-walker for good, and loading any std module cost
    /// +16M instructions (2026-10-23, found by the benchmark refresh). Asserted through
    /// `compile`, the entry point the boot loop calls.
    #[test]
    fn the_builder_heap_expands_a_root_escaped_macro_head() {
        let (mut heap, root) = builder_heap();
        let def = syntax::reader::read_one(&mut heap, "(def mm (%make-macro (fn (x) x)))")
            .expect("parse");
        eval::eval(&mut heap, def, root).expect("define the macro in the builder frame");
        let call = syntax::reader::read_one(&mut heap, "(/mm 5)").expect("parse");
        let compiled = eval::macros::compile(&mut heap, call, root).expect("compile");
        assert_eq!(
            syntax::printer::print(&heap, compiled),
            "5",
            "`(/mm 5)` must expand at compile time on the builder heap"
        );
    }
}
