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
    // Two boot paths (2026-09-12; there were three until the ADR-138 expanded-prelude TEXT
    // cache was deleted). The prelude image (ADR-314) materialises the prelude's bindings
    // structurally from `~/.cache/brood/prelude-expanded-<build-id-hash>.img`; any miss —
    // no file, a different build, a torn or undecodable artifact — falls through to the
    // full source boot, which rewrites the image. A bad artifact therefore costs a slower
    // boot and never a wrong one.
    //
    // `BROOD_NO_PRELUDE_IMAGE=1` is the ONE knob: neither read nor written. (It used to
    // sit beside `BROOD_NO_BOOT_CACHE`, and the two meant different subsets of a
    // three-artifact chain; with one artifact there is one switch.)
    //
    // The image shipped default-on twice before and was reverted the same day both times
    // (KI-105, KI-106), each on a fact the evaluation RECORDS rather than binds. Both are
    // fixed with sabotage-verified guards and the boot differential
    // (`prelude_image_matches_source.rs`) compares image against source. The gensym floor
    // is now such a fact carried IN the image header — it used to be read from the text
    // cache's header, which is the coupling that made the text cache impossible to delete.
    if std::env::var_os("BROOD_NO_PRELUDE_IMAGE").is_none() {
        if let Some(bundle) = boot_from_prelude_image() {
            set_boot_source(BOOT_PRELUDE_IMAGE);
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

/// Which of the two boot paths actually ran, as an atomic so it can be read after the
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
    use std::path::PathBuf;
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    builtins::build_id_string().hash(&mut h);
    Some(
        base.join("brood")
            .join(format!("prelude-expanded-{:016x}.img", h.finish())),
    )
}

/// Best-effort prune of OTHER builds' prelude images: keep the `MAX_KEEP` most recent,
/// drop anything older than `MAX_AGE`, never touch `keep` (the one just written).
///
/// Bounded by COUNT and not only by age, for the reason the stdlib image's `prune` records:
/// on a machine that is editing the standard library — the only kind that writes these —
/// every rebuild mints a new file and nothing is ever old enough for an age rule to catch.
/// One artifact per build now; this used to prune a `.blsp`/`.img` PAIR by shared stem and
/// once dropped the text half while keeping the image (boot.rs history), a hazard that
/// disappeared with the second artifact.
fn prelude_image_prune(dir: &std::path::Path, keep: &std::path::Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    const MAX_KEEP: usize = 16;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut others: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("prelude-expanded-") && name.ends_with(".img")) || p == keep {
            continue;
        }
        match e.metadata().and_then(|m| m.modified()) {
            Ok(modified) => others.push((modified, p)),
            // Unreadable metadata: not worth keeping around.
            Err(_) => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    others.sort_unstable_by_key(|a| std::cmp::Reverse(a.0));
    for (i, (modified, p)) in others.iter().enumerate() {
        let stale = modified.elapsed().ok().is_some_and(|age| age > MAX_AGE);
        if i >= MAX_KEEP || stale {
            let _ = std::fs::remove_file(p);
        }
    }
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
    let (n, gensym_floor) =
        image::load_prelude_image(&mut heap, root, &path, &builtins::build_id_string())?;
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
    // The names baked into the image's closures were minted by the boot that wrote it, so
    // a runtime `gensym` must start ABOVE that counter or a fresh name could collide with a
    // baked one. The image carries the floor in its header (it used to be read from the
    // deleted text cache's header — the one fact that tied the two artifacts together).
    core::value::gensym_floor(gensym_floor);
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

/// The full source boot: parse + macro-expand + eval + freeze the prelude,
/// then (best-effort) write the prelude image for the next boot.
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
    let write_image = std::env::var_os("BROOD_NO_PRELUDE_IMAGE").is_none();
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
        // Records the RAW form's definition sites (LSP navigation into the prelude is
        // identical on both boot paths because of this). Its returned names fed the
        // deleted text cache; the side effect is what matters.
        heap.note_definition_recording(form, pos);
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
    // failure leaves the next boot on the source path, which is where it was before.
    if write_image {
        if let Some(img) = prelude_image_path() {
            let _ = image::write_prelude_image(
                &mut heap,
                root,
                &builtin_names,
                &img,
                &builtins::build_id_string(),
                core::value::gensym_counter(),
            )
            .map(|()| img.parent().map(|d| prelude_image_prune(d, &img)));
        }
    }
    let (code, bindings) = heap.freeze_as_shared_code(root);
    let t_freeze = t_mark.elapsed();
    if std::env::var_os("BROOD_BOOT_TRACE").is_some() {
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
