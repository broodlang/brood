//! Where a language comes from: the grammars built in, and the ones loaded from a shared
//! library at runtime (declared, then opened at first use). The policy of where an
//! application keeps its grammars is Brood's.

use super::*;

/// `(tree-sitter-load-grammar path lang)` — load a tree-sitter grammar from a
/// shared library at runtime and register it under the language keyword `lang`,
/// so `tree-sitter-parse` can use it like a built-in one. Returns the grammar's
/// ABI version.
///
/// **Why this rather than a search path.** The kernel does not know where an
/// application keeps its grammars, and should not: a path convention is policy.
/// This takes the library the caller chose, which leaves "look in
/// `~/.config/bedit/grammars`, named for the file's mode" entirely in the editor,
/// and leaves the kernel with the mechanism it can actually own — dlopen the
/// library, find `tree_sitter_<lang>`, check the ABI, remember it.
///
/// **The library is leaked, deliberately.** A `Language` is a pointer into the
/// loaded object's static tables, so unmapping the library would dangle every
/// tree ever parsed with it. Grammars are loaded once and used for the life of
/// the process; `std::mem::forget` says that in one line, where an `Arc` holding
/// the `Library` alongside every `Language` would say it in many and still never
/// drop. Because the loader would then answer a second load of the same PATH with that
/// first object, the library is opened from a copy named for its content
/// ([`open_library`]): a library rebuilt in place and loaded again is the new build.
///
/// Failure is an ordinary error at every step — a missing file, a library with no
/// such grammar, a grammar built for an incompatible tree-sitter. None of them is
/// a crash. What this cannot make safe is a *hostile* library: it is native code
/// and runs with the process's privileges, exactly like an Emacs dynamic module,
/// so an application should load only from a directory its user controls.
pub fn load_grammar(path: &str, lang: &str) -> LispResult {
    let (abi, _) = load_library("%tree-sitter-load-grammar", path, lang)?;
    // An eager load supersedes a declaration of the same language: the caller has just
    // said which library is the grammar, and a pending one must not replace it at the next
    // parse, nor a failed one shadow it.
    crate::core::sync::lock(&DECLARED).remove(lang);
    Ok(Value::int(abi))
}

/// How a runtime grammar library was opened ([`open_library`]), reported by
/// [`grammar_info`] as `:loaded-via` (ADR-583).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoadedVia {
    /// A content-named copy beside the library, in its own directory: the same filesystem
    /// (so the same exec permission) and the same `$ORIGIN` for its RPATH.
    Beside,
    /// A content-named copy in the user's cache directory, the library's own directory not
    /// being writable.
    Cache,
    /// The library's own path, no copy being possible: a later load of a library rebuilt at
    /// that path may still be this one until the process restarts.
    InPlace,
}

impl LoadedVia {
    fn keyword(self) -> &'static str {
        match self {
            LoadedVia::Beside => "beside",
            LoadedVia::Cache => "cache",
            LoadedVia::InPlace => "in-place",
        }
    }
}

/// Held across one copy, open and clean-up, so two loads in this process never write or
/// remove a copy under each other.
static GRAMMAR_COPY_LOCK: Mutex<()> = Mutex::new(());

/// The user's cache directory for grammar copies, `$XDG_CACHE_HOME/brood/grammars` (else
/// `~/.cache/brood/grammars`), made owner-only — or accepted when it already is: a real
/// directory this user owns that no one else can write, so no one else can have planted a
/// library in it.
fn grammar_cache_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".cache"))
        })?;
    let dir = base.join("brood").join("grammars");
    std::fs::create_dir_all(dir.parent()?).ok()?;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return None;
        }
    }
    Some(dir)
}

/// Write `bytes` to `copy`, owner-only, through a staging name renamed into place — so a copy
/// of the same name still mapped from an earlier load is replaced by a new file, never
/// truncated under it — then dlopen it and remove the file (the mapping outlives it). `None`
/// when the copy could not be written or opened, for the caller to try the next place.
///
/// # Safety
/// As `libloading::Library::new`.
unsafe fn open_copy_at(copy: &std::path::Path, bytes: &[u8]) -> Option<libloading::Library> {
    use std::io::Write;
    let staging = copy.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o700);
    let written = options
        .open(&staging)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| std::fs::rename(&staging, copy));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
        return None;
    }
    // SAFETY: the caller's — the dlopen it asked for, of the bytes it named.
    let opened = unsafe { libloading::Library::new(copy) }.ok();
    let _ = std::fs::remove_file(copy);
    opened
}

/// Open the grammar library at `path` (ADR-583): from a copy named for its CONTENT beside it,
/// else from one in the user's cache directory, else — neither being writable or loadable —
/// from `path` itself. Answers the library (or the loader's error for `path`) and how it was
/// opened.
///
/// **Why a copy.** The dynamic loader keys what it has loaded by NAME: `dlopen` of a path it
/// already opened returns the object it has, whatever the file holds now. A library is never
/// unloaded here (see [`load_grammar`]), so loading a grammar, rebuilding it in place and
/// loading it again kept parsing with the first build until the process restarted — an
/// install, an update or a reload all reported success while the old grammar ran. A name
/// made of the content (and this process) gives the loader a new name exactly when the
/// content is new, and the same name when it is not, so an unchanged library loaded again is
/// the object already loaded rather than a second mapping.
///
/// **Why beside it.** A copy in the temporary directory fails on a `noexec` /tmp (a hardened
/// host, a container), and a grammar whose RPATH says `$ORIGIN` finds its sibling libraries
/// only from its own directory. Beside the original it is on the same filesystem with the
/// same `$ORIGIN`; the cache directory is the fallback for a read-only grammar directory, and
/// the path itself the last resort, reported as `:in-place`.
///
/// # Safety
/// As `libloading::Library::new`: the library's initialisers run, with the process's
/// privileges.
unsafe fn open_library(
    path: &str,
    bytes: &[u8],
    hash: &str,
) -> (Result<libloading::Library, libloading::Error>, LoadedVia) {
    let original = std::path::Path::new(path);
    let stem = original
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("grammar");
    let short_hash = hash.get(..16).unwrap_or(hash);
    let name = format!(".{stem}.{short_hash}.{}.so", std::process::id());
    let _held = crate::core::sync::lock(&GRAMMAR_COPY_LOCK);
    let beside = original.parent().map(|dir| {
        if dir.as_os_str().is_empty() {
            std::path::Path::new(".")
        } else {
            dir
        }
    });
    // SAFETY: the caller's, for each attempt.
    if let Some(dir) = beside
        && let Some(library) = unsafe { open_copy_at(&dir.join(&name), bytes) }
    {
        return (Ok(library), LoadedVia::Beside);
    }
    if let Some(dir) = grammar_cache_dir()
        && let Some(library) = unsafe { open_copy_at(&dir.join(&name), bytes) }
    {
        return (Ok(library), LoadedVia::Cache);
    }
    (
        unsafe { libloading::Library::new(path) },
        LoadedVia::InPlace,
    )
}

/// dlopen `path`, check the grammar's ABI and register it under `lang` — the work of
/// [`load_grammar`], shared with the lazy load of a declared grammar
/// ([`declared_language`]). `who` is the primitive the caller's errors name. Answers the ABI
/// version and the registered language.
fn load_library(
    who: &str,
    path: &str,
    lang: &str,
) -> Result<(i64, tree_sitter::Language), LispError> {
    // Brood keywords are spelled with hyphens and C identifiers cannot be, so `:c-sharp`
    // asks for `tree_sitter_c_sharp` — which is exactly what the tree-sitter-c-sharp grammar
    // exports. The library's entry point is named for the GRAMMAR, so `lang` names the
    // grammar rather than being a nickname for it; there is no aliasing here and shouldn't
    // be, since the mode that asks to parse `:c-sharp` is the same name on both sides.
    let symbol = format!("tree_sitter_{}", lang.replace('-', "_"));
    let bytes =
        std::fs::read(path).map_err(|e| LispError::runtime(format!("{who}:{path}: {e}")))?;
    // The gate (ADR-660): a quarantined library is refused here, before any of its code runs
    // in this process, and one never vetted is vetted first, in a child.
    let hash = super::vet::admit(path, lang, &bytes)?;
    // From here the library's code runs (its initialisers, its entry point, set_language's
    // scanner set-up): a crash in it names it, and quarantines it for the next start.
    let mark = super::vet::build_mark(lang, path, &hash);
    let _marked = crate::cli_support::enter_foreign_code(Some(mark));
    // SAFETY: dlopen'ing a caller-chosen library and calling the grammar constructor it
    // exports. The symbol type is tree-sitter's documented grammar entry point, and the
    // Language it yields is validated against the host's ABI range below before any use.
    // The library is opened from a private copy named for its CONTENT, never from `path`
    // itself: the dynamic loader hands back the object it already has for a name it has
    // seen, so a grammar rebuilt in place and loaded again from the same path would go on
    // parsing with the old one (ADR-583). See `open_library`.
    let (opened, via) = unsafe { open_library(path, &bytes, &hash) };
    let language = unsafe {
        let lib = opened.map_err(|e| LispError::runtime(format!("{who}:{path}: {e}")))?;
        let entry: libloading::Symbol<unsafe extern "C" fn() -> *const ()> = lib
            .get(symbol.as_bytes())
            .map_err(|e| LispError::runtime(format!("{who}:{path} exports no {symbol}: {e}")))?;
        let f = *entry;
        std::mem::forget(lib); // the grammar's tables must stay mapped — see above
        tree_sitter::Language::new(tree_sitter_language::LanguageFn::from_raw(f))
    };
    // `set_language` is the ABI gate: it range-checks the grammar's version against what
    // this tree-sitter can drive, so a grammar from another era is a message, not a crash.
    tree_sitter::Parser::new()
        .set_language(&language)
        .map_err(|e| {
            LispError::runtime(format!(
                "{who}:{path}: {lang} is not usable by this runtime \
                 (grammar ABI {}, this build drives {}..={}): {e}",
                language.abi_version(),
                tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION,
                tree_sitter::LANGUAGE_VERSION
            ))
        })?;
    let abi = language.abi_version() as i64;
    let previous_mark = super::vet::mark_of(lang);
    crate::core::sync::lock(&DYNAMIC).insert(lang.to_string(), language.clone());
    crate::core::sync::lock(&LOADED_FROM).insert(lang.to_string(), (path.to_string(), via));
    super::vet::set_mark(lang, mark);
    // A re-load replaces the entry, so a grammar can be swapped without restarting; the
    // parser pool is keyed by language name and holds parsers bound to the OLD language, so
    // it is cleared for this one rather than left to hand out stale parsers. Dropping one
    // runs the old library's scanner clean-up, under the old library's mark.
    let stale_parsers = crate::core::sync::lock(&PARSER_POOL).remove(lang);
    {
        let _old = crate::cli_support::enter_foreign_code(previous_mark);
        drop(stale_parsers);
    }
    // The tree cache is keyed by language name too, and holds trees SHAPED BY the old
    // grammar. An exact hit would hand one back for the new one, and an incremental
    // reparse would hand it to the new grammar's parser as the tree to edit from — which
    // is not merely stale but ill-formed, and fails the parse outright. Same reasoning as
    // the pool above; the cache simply arrived later than the comment did.
    crate::core::sync::lock(&TREE_CACHE).retain(|(l, _, _)| l != lang);
    // Held tree handles keep answering queries from the trees they have — those are valid
    // trees, merely from the old grammar — but none is reparsed FROM again, for the reason
    // above: a reparse after this sees a newer epoch for this language and parses afresh.
    *crate::core::sync::lock(&GRAMMAR_EPOCHS)
        .entry(lang.to_string())
        .or_insert(0) += 1;
    Ok((abi, language))
}

/// Grammars loaded at runtime by [`load_grammar`], keyed by language name. Consulted
/// BEFORE the compile-time arms, so loading one deliberately overrides a bundled grammar of
/// the same name — the point of being able to load one at all is to move faster than the
/// runtime's release cycle.
static DYNAMIC: LazyLock<Mutex<HashMap<String, tree_sitter::Language>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) fn dynamic_language(lang: &str) -> Option<tree_sitter::Language> {
    crate::core::sync::lock(&DYNAMIC).get(lang).cloned()
}

/// The library each grammar in [`DYNAMIC`] was loaded from, keyed by language name — what
/// [`grammar_info`] reports as `:source`, so "which grammar is this" has an answer a user
/// can go and look at. Written beside `DYNAMIC`, after it (never held together).
static LOADED_FROM: LazyLock<Mutex<HashMap<String, (String, LoadedVia)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(%tree-sitter-grammar-info lang)` — what the grammar `lang` parses with IS (ADR-583):
/// `{:lang :abi :version :name :source :loaded-via :named :anonymous :fields}`. `:named` and `:anonymous`
/// are the visible node kinds (aliases included, hidden and supertype rules not — no tree
/// ever holds one) as keyword sets, `:fields` the field names; `:version` is the grammar's
/// own `"major.minor.patch"` and `:name` its name, both nil before ABI 15, which did not
/// record them; `:source` the library path, or `:built-in`; `:loaded-via` how it was opened —
/// `:beside` / `:cache` (a content-named copy, see [`open_library`]) or `:in-place` (its own
/// path, when no copy could be made: a library rebuilt there may stay the old one until a
/// restart) — nil for a built-in grammar.
///
/// Loads the grammar when it is only declared — asking what a grammar contains is a use of
/// it — and errors as a parse would when there is none. The answer is the whole vocabulary a
/// language's face table, indent spec and injections may name, which is what makes a grammar
/// update that renamed a node DETECTABLE: the editor compares the names it wrote against this
/// set instead of finding out from a file that stopped colouring.
pub fn grammar_info(heap: &mut Heap, lang: &str) -> LispResult {
    let language = language_for(lang)?;
    let keyword = |name: &str| Value::keyword(value::intern(name));
    let mut named = Vec::new();
    let mut anonymous = Vec::new();
    for id in 0..language.node_kind_count() {
        let Ok(id) = u16::try_from(id) else { break };
        if !language.node_kind_is_visible(id) {
            continue;
        }
        let Some(kind) = language.node_kind_for_id(id) else {
            continue;
        };
        if language.node_kind_is_named(id) {
            named.push(keyword(kind));
        } else {
            anonymous.push(keyword(kind));
        }
    }
    let fields: Vec<Value> = (1..=language.field_count())
        .filter_map(|id| u16::try_from(id).ok())
        .filter_map(|id| language.field_name_for_id(id))
        .map(keyword)
        .collect();
    let named = heap.set_from_elems(named);
    let anonymous = heap.set_from_elems(anonymous);
    let fields = heap.set_from_elems(fields);
    let version = match language.metadata() {
        Some(meta) => heap.alloc_string_owned(format!(
            "{}.{}.{}",
            meta.major_version, meta.minor_version, meta.patch_version
        )),
        None => Value::Nil,
    };
    let name = match language.name() {
        Some(name) => heap.alloc_string(name),
        None => Value::Nil,
    };
    let (source, via) = if dynamic_language(lang).is_some() {
        match crate::core::sync::lock(&LOADED_FROM).get(lang).cloned() {
            Some((path, via)) => (heap.alloc_string_owned(path), keyword(via.keyword())),
            None => (keyword("loaded"), Value::Nil),
        }
    } else {
        (keyword("built-in"), Value::Nil)
    };
    Ok(heap.map_from_pairs(vec![
        (keyword("lang"), keyword(lang)),
        (keyword("abi"), Value::int(language.abi_version() as i64)),
        (keyword("version"), version),
        (keyword("name"), name),
        (keyword("source"), source),
        (keyword("loaded-via"), via),
        (keyword("named"), named),
        (keyword("anonymous"), anonymous),
        (keyword("fields"), fields),
    ]))
}

/// What a declared grammar is waiting on: its library, or the reason it did not load.
pub(super) enum Declared {
    /// Declared and not yet asked for: nothing has been opened.
    Pending { path: String },
    /// Asked for, and the load failed. Kept, not retried: a parse per keystroke would
    /// otherwise dlopen the same broken library per keystroke. A new declaration replaces it.
    Failed { path: String, message: String },
}

/// Grammars DECLARED by [`declare_grammar`] and not yet loaded (ADR-481), keyed by language
/// name. The first parse, query or tree that names one loads it ([`declared_language`]),
/// which moves it to [`DYNAMIC`]; a failed load stays here as the reason.
///
/// Lock order: this before [`DYNAMIC`] (and the pool and cache a load clears) — a lazy load
/// holds this lock across the dlopen, which is what makes it happen once.
pub(super) static DECLARED: LazyLock<Mutex<HashMap<String, Declared>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(%tree-sitter-declare-grammar path lang)` — say that `lang`'s grammar is the library at
/// `path`, WITHOUT loading it (ADR-481). The first parse, structural query or tree handle that
/// names `lang` dlopens it, once; until then the declaration costs a map entry. An editor
/// can therefore declare every grammar it has at startup and pay for the one the file it
/// opened needs, instead of dlopening them all before its first frame or racing a file
/// opened before a deferred loader got there.
///
/// A declaration replaces any earlier one of the same name, and a declaration of a language
/// already loaded is a RELOAD at its next use — the same swap [`load_grammar`] makes (parser
/// pool, tree cache and epoch), so replacing a library and declaring it again is how a
/// grammar is upgraded without a restart, lazily. A load that fails leaves the grammar the
/// language had before, if any, exactly as a failed [`load_grammar`] does; with none, the
/// parse fails with the load's message. [`grammar_status`] reports which.
pub fn declare_grammar(path: &str, lang: &str) -> LispResult {
    crate::core::sync::lock(&DECLARED).insert(
        lang.to_string(),
        Declared::Pending {
            path: path.to_string(),
        },
    );
    Ok(Value::Bool(true))
}

/// The grammar a DECLARATION of `lang` supplies, loading it if this is its first use:
/// `Ok(None)` when there is no declaration to honour (none, or one that failed while an
/// older grammar still serves), `Err` when the declared library failed and nothing else can
/// parse `lang`. The lock is held across the load, so two processes asking at once load it
/// once — the second finds it loaded.
fn declared_language(lang: &str) -> Result<Option<tree_sitter::Language>, LispError> {
    let mut declared = crate::core::sync::lock(&DECLARED);
    let path = match declared.get(lang) {
        None => return Ok(None),
        Some(Declared::Failed { message, .. }) => {
            return if has_grammar_without_declaration(lang) {
                Ok(None)
            } else {
                Err(LispError::runtime(message.clone()))
            };
        }
        Some(Declared::Pending { path }) => path.clone(),
    };
    match load_library("%tree-sitter-declare-grammar", &path, lang) {
        Ok((_, language)) => {
            declared.remove(lang);
            Ok(Some(language))
        }
        // the caller was killed while the library was being vetted: no verdict, and the
        // declaration stays pending for the next use
        Err(error) if error.is_kill_signal() => Err(error),
        Err(error) => {
            let message = format!(
                "the :{lang} grammar declared at {path} did not load: {}",
                error.message
            );
            declared.insert(
                lang.to_string(),
                Declared::Failed {
                    path: path.clone(),
                    message: message.clone(),
                },
            );
            if has_grammar_without_declaration(lang) {
                Ok(None)
            } else {
                Err(LispError::runtime(message))
            }
        }
    }
}

/// Whether `lang` parses without consulting a declaration: loaded earlier, or built in.
fn has_grammar_without_declaration(lang: &str) -> bool {
    dynamic_language(lang).is_some() || built_in_language(lang).is_some()
}

/// `(%tree-sitter-grammar-status lang)` — where `lang`'s grammar stands: `:declared`
/// (declared, not loaded yet), `:loaded` (loaded at runtime), `:built-in`, a failure value
/// carrying the load's message when the last declaration did not load, `{:quarantined reason
/// :library :sha256 :by}` when the declared library is quarantined (ADR-660: it crashed or hung
/// a vetting child, or crashed a runtime), or `nil` when the runtime has no grammar for it.
/// Asks; never loads.
pub fn grammar_status(heap: &mut Heap, lang: &str) -> LispResult {
    let keyword = |name: &str| Value::keyword(value::intern(name));
    let (path, failed) = match crate::core::sync::lock(&DECLARED).get(lang) {
        Some(Declared::Pending { path }) => (Some(path.clone()), None),
        Some(Declared::Failed { path, message }) => (Some(path.clone()), Some(message.clone())),
        None => (None, None),
    };
    if let Some(path) = &path
        && let Some(verdict) = super::vet::quarantine_of(path)
    {
        let reason = heap.alloc_string(&verdict.reason());
        let library = heap.alloc_string(path);
        let mut pairs = vec![
            (keyword("quarantined"), reason),
            (keyword("library"), library),
        ];
        for key in ["sha256", "by", "signal"] {
            if let Some(field) = verdict.field_value(heap, key) {
                pairs.push((keyword(key), field));
            }
        }
        return Ok(heap.map_from_pairs(pairs));
    }
    if let Some(message) = failed {
        return Ok(heap.alloc_failure(&message));
    }
    if path.is_some() {
        return Ok(keyword("declared"));
    }
    if dynamic_language(lang).is_some() {
        return Ok(keyword("loaded"));
    }
    if built_in_language(lang).is_some() {
        return Ok(keyword("built-in"));
    }
    Ok(Value::Nil)
}

/// `(%tree-sitter-languages)` — the languages this runtime can parse right now: every
/// grammar loaded at runtime, declared and not yet loaded, or built in, sorted. The question
/// an editor asks before it offers to fetch a grammar, or tells you a file's mode has none:
/// without it the only way to find out was to parse something and read the error. A
/// declared grammar counts — asking must not load it — and one whose load failed does not,
/// unless something else still parses its language.
pub fn languages(heap: &mut Heap) -> LispResult {
    #[allow(unused_mut)] // a build with no grammar feature never pushes a built-in
    let mut names: Vec<String> = crate::core::sync::lock(&DYNAMIC).keys().cloned().collect();
    let pending: Vec<(String, String)> = crate::core::sync::lock(&DECLARED)
        .iter()
        .filter_map(|(name, declared)| match declared {
            Declared::Pending { path } => Some((name.clone(), path.clone())),
            Declared::Failed { .. } => None,
        })
        .collect();
    // a quarantined library will not load, so its language is not one this runtime parses
    names.extend(
        pending
            .into_iter()
            .filter(|(_, path)| super::vet::quarantine_of(path).is_none())
            .map(|(name, _)| name),
    );
    #[cfg(feature = "treesit-ruby")]
    names.push("ruby".to_string());
    #[cfg(feature = "treesit-elixir")]
    names.push("elixir".to_string());
    names.sort();
    names.dedup();
    let keywords = names
        .iter()
        .map(|name| Value::keyword(crate::core::value::intern(name)))
        .collect();
    Ok(heap.alloc_vector(keywords))
}

/// The grammar for a language keyword's name: a declared one first (loading it on this, its
/// first use), then one loaded at runtime, then one built in.
pub(super) fn language_for(lang: &str) -> Result<tree_sitter::Language, LispError> {
    // A declaration is the newest word on what `lang`'s grammar is — a pending one is a
    // load or a reload waiting for its first use.
    if let Some(language) = declared_language(lang)? {
        return Ok(language);
    }
    // A grammar loaded at runtime wins over a bundled one of the same name: loading one is a
    // deliberate act, and the reason to do it is usually that the bundled grammar is behind.
    if let Some(language) = dynamic_language(lang) {
        return Ok(language);
    }
    built_in_language(lang).ok_or_else(|| {
        LispError::runtime(format!(
            "%tree-sitter-parse:no grammar for :{lang} — declare one with \
             (%tree-sitter-declare-grammar \"/path/to/libtree-sitter-{lang}.so\" :{lang}) \
             or load it now with %tree-sitter-load-grammar, or build it in with \
             --features treesit-{lang}"
        ))
    })
}

/// The grammar compiled into this runtime for `lang`, if any. Each arm is gated on its own
/// `treesit-<lang>` feature — the kernel ships no grammar by default (a `treesit` build with
/// no grammar feature has zero arms and reports every language as not built in). One cfg'd
/// arm per language is the single place a grammar plugs in (plus its `Cargo.toml` dep).
fn built_in_language(lang: &str) -> Option<tree_sitter::Language> {
    match lang {
        #[cfg(feature = "treesit-ruby")]
        "ruby" => Some(tree_sitter_ruby::LANGUAGE.into()),
        #[cfg(feature = "treesit-elixir")]
        "elixir" => Some(tree_sitter_elixir::LANGUAGE.into()),
        _ => None,
    }
}
