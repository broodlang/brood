//! **Every process-wide table is process-wide on purpose.**
//!
//! Several runtimes in one OS process is a real configuration — an embedder with several
//! `Interp`s, `cargo test` threads, the nightly sanitizer jobs — and state that belongs to a
//! runtime belongs on `RuntimeCode`, never in a `static`. KI-224 (the held image file, the
//! `defdyn` marks) and KI-226 (the module-absence memo, the image kind index) were each a
//! `static` holding one runtime's facts, found only when two runtimes happened to share a
//! process.
//!
//! This gate makes the choice explicit. Every `static` in `crates/lisp/src` whose type holds a
//! lock or a shared append-only vector — the shapes a table takes — must be listed in
//! [`PROCESS_WIDE`] with the reason it is not per runtime; a listed entry that no longer
//! exists fails too, so the list stays the truth. Counters, flags and `OnceLock`s of plain
//! values are not tables and are not listed; `thread_local!`s are per thread.
//!
//! Adding a table? Ask first whether it records something about ONE runtime: its globals,
//! its load path, the images it opened, its modules. If so it is a `RuntimeCode` field.

use std::path::{Path, PathBuf};

mod support;
use support::rust_files;

/// The allowed reasons, so an entry has to commit to one.
#[derive(Clone, Copy, Debug)]
enum Reason {
    /// Keyed by pid, handle id, symbol id or another identity unique across the whole OS
    /// process: runtimes cannot collide in it, and processes of different runtimes may
    /// legitimately address each other (messages cross runtimes as copies).
    ProcessUniqueKeys,
    /// One instance per OS process by design (the scheduler's workers, the timer thread, the
    /// network reactor, the JIT compiler thread, the GUI event loop, the node — ADR-033's
    /// "one node per OS process").
    OneMachine,
    /// Facts about the binary itself — its baked-in std sources, curated tables, compiled
    /// grammars — identical for every runtime it hosts.
    FactOfTheBinary,
    /// Diagnostics: profiling, coverage, audits and once-only warnings, read by a person.
    Diagnostic,
    /// A cache whose key fully determines its value, so sharing it is only a speed-up.
    PureCache,
    /// A speed decision keyed by a name: an entry another runtime made can only cost this
    /// runtime time, never change a result.
    SpeedHeuristic,
}

use Reason::*;

/// `(path under crates/lisp/src, static name, reason)`.
const PROCESS_WIDE: &[(&str, &str, Reason)] = &[
    // The symbol interner: symbols are the same `u32` in every runtime, which is what lets
    // a message cross between runtimes without re-interning.
    ("core/value.rs", "IDS", OneMachine),
    ("core/value.rs", "NAMES", OneMachine),
    ("core/value.rs", "NAME_HASHES", OneMachine),
    // Process tables keyed by pid; pids are unique across the OS process.
    ("process/mailbox.rs", "REGISTRY", ProcessUniqueKeys),
    ("process/monitor.rs", "MONITORS", ProcessUniqueKeys),
    ("process/links.rs", "LINKS", ProcessUniqueKeys),
    ("process/links.rs", "REMOTE_LINKS", ProcessUniqueKeys),
    ("core/table.rs", "REGISTRY", ProcessUniqueKeys),
    // Tree-sitter parse-tree handles (ADR-443): a handle is a process-unique id, and a tree
    // is sent between processes (a task parses it off the loop and gives it back); each is
    // owned by a pid and freed when that process exits (ADR-495).
    ("host/treesit.rs", "TREE_HANDLES", ProcessUniqueKeys),
    // The table registry's reusable slots: a slot index plus its generation is a handle,
    // unique across the OS process like the registry it indexes (ADR-418).
    ("core/table.rs", "FREE_SLOTS", ProcessUniqueKeys),
    ("host/net.rs", "REGISTRY", ProcessUniqueKeys),
    ("host/subprocess.rs", "REGISTRY", ProcessUniqueKeys),
    ("host/wasm.rs", "REGISTRY", ProcessUniqueKeys),
    // Compiled components keyed by the BLAKE3 of their source bytes: the bytes decide the code,
    // and each load still gets a store and instance of its own.
    ("host/wasm.rs", "COMPILED", PureCache),
    // The scheduler, timer and stdin are one per machine; sysmon observes them.
    ("process/timer.rs", "TIMERS", OneMachine),
    ("process/sysmon.rs", "MONITORS", OneMachine),
    ("builtins/io.rs", "STDIN_READER", OneMachine),
    // The terminal's input forwarder (ADR-491): one thread reading the one tty this OS process
    // has, delivering to whichever process asked for its input.
    ("builtins/terminal/native.rs", "FORWARDER", OneMachine),
    // What the one tty shows now, cell by cell, and its colour depth (ADR-586): the next
    // frame writes only what differs from it, whichever runtime draws.
    ("builtins/terminal/native.rs", "TERMINAL_SCREEN", OneMachine),
    // The net reactor multiplexes every socket in the OS process on one thread.
    ("host/net.rs", "STARTING", OneMachine),
    // One audio thread owns the output device for the whole OS process.
    ("host/audio.rs", "STARTING", OneMachine),
    ("builtins/filesystem.rs", "LOCK", OneMachine),
    // A signal and its disposition belong to the OS process, and so does a trap of one.
    ("builtins/os.rs", "SIGNAL_TRAPS", OneMachine),
    // The allocator's count, and so the soft memory limit, is the OS process's.
    ("process/pressure.rs", "PRESSURE", OneMachine),
    // The node: one per OS process (ADR-033).
    ("dist.rs", "NODE", OneMachine),
    ("dist.rs", "NODES", OneMachine),
    ("dist.rs", "NAMES", OneMachine),
    ("dist.rs", "NODE_MONITORS", OneMachine),
    ("dist.rs", "LISTEN_ADDRS", OneMachine),
    ("dist.rs", "DIAL_ROUTES", OneMachine),
    ("dist.rs", "PENDING_DIALS", OneMachine),
    // Guest links (ADR-415): the grants the node accepts. A connected guest's standing is
    // its link's (`Conn::guest`), not a table (KI-286).
    ("dist/guest.rs", "GRANTS", OneMachine),
    ("process/monitor.rs", "PENDING_REMOTE", OneMachine),
    // Native code lives in one executable mapping for the process, keyed by code pointer.
    ("jit.rs", "GLOBAL_JIT", OneMachine),
    (
        "eval/compile/jit_runtime/compiler.rs",
        "JIT_ARM_KEEPALIVE",
        ProcessUniqueKeys,
    ),
    (
        "eval/compile/jit_lower.rs",
        "JIT_INLINE_CHUNK_KEEPALIVE",
        OneMachine,
    ),
    (
        "eval/compile/jit_lower/i64.rs",
        "I64_TOO_DEEP",
        SpeedHeuristic,
    ),
    // Host services that exist once: the GUI event loop and its windows, the clipboard,
    // dynamically loaded grammars.
    ("host/gui/backend.rs", "G", OneMachine),
    ("host/gui/backend.rs", "S", OneMachine),
    ("host/gui/backend.rs", "W", OneMachine),
    // The GUI thread's exit gate: one GUI thread per OS process, shut once at its exit.
    ("host/gui/backend.rs", "EXIT_GATE", OneMachine),
    // the offscreen painter's one thread (gui/render-frame, ADR-609): text shaping cannot
    // move between threads, so every runtime in the process sends its frames to this one
    ("host/gui/backend/offscreen.rs", "T", OneMachine),
    // The parsed memo fragments of each window (ADR-470), by window id — an id the one GUI
    // event loop hands out — and stamped with the pid that drew them: a draw from any other
    // process, of this runtime or another, finds no entry and parses afresh.
    (
        "builtins/terminal/native/fragments.rs",
        "CACHES",
        ProcessUniqueKeys,
    ),
    // The terminal keyboard: the kitty flags pushed, the input read ahead and the keys down.
    // A process has one controlling terminal, whichever runtime polls it.
    (
        "builtins/terminal/native/keyboard.rs",
        "TERMINAL_KEYBOARD",
        OneMachine,
    ),
    // The `brood-tab` page's display outbox: one page drains one wasm instance.
    ("host/tab.rs", "OUTBOX", OneMachine),
    // The prelude image a host hands over, taken by the one shared prelude boot (`SHARED`)
    // every runtime in the process starts from.
    ("boot.rs", "PROVIDED_PRELUDE_IMAGE", OneMachine),
    // The `mem:` images: the in-memory stand-in for the file system a host without one
    // provides (a tab), keyed by path as files are, read alike by every runtime.
    ("boot/image.rs", "MEMORY_IMAGES", OneMachine),
    ("host/clipboard.rs", "CLIPBOARD", OneMachine),
    // the clipboard's reader, apart from the owner so a wedged read blocks no write, and
    // replaced past a deadline so it blocks no later read for good (ADR-536)
    ("host/clipboard.rs", "READERS", OneMachine),
    ("host/treesit.rs", "DYNAMIC", OneMachine),
    // Grammars declared and not yet loaded (ADR-481): the same machine-wide table as
    // DYNAMIC, one step earlier.
    ("host/treesit.rs", "DECLARED", OneMachine),
    // Where each grammar in DYNAMIC was loaded from (ADR-583): written beside it, as wide.
    ("host/treesit.rs", "LOADED_FROM", OneMachine),
    // How many times each language's grammar has loaded, which a held tree's reparse checks
    // (ADR-583): a fact about the machine-wide DYNAMIC table, so as wide as it.
    ("host/treesit.rs", "GRAMMAR_EPOCHS", OneMachine),
    // Serialises the private copies a grammar is dlopened from (ADR-583): the dynamic loader
    // and the temporary directory are the process's, so the lock is too.
    ("host/treesit.rs", "GRAMMAR_COPY_LOCK", OneMachine),
    // Facts of the binary: the std sources baked into it, and the signatures derived from
    // them (the stdlib id is a build constant).
    ("types/check.rs", "STD_REQUIRES", FactOfTheBinary),
    ("types/check/sigs.rs", "CACHE", FactOfTheBinary),
    ("eval/derive.rs", "SIGS", FactOfTheBinary),
    // Caches whose key determines the value.
    ("builtins/regex_native.rs", "CACHES", PureCache),
    ("builtins/regex_native.rs", "LEXERS", PureCache),
    // The rope-search engines (ADR-540): compiled from a pattern's text and nothing else,
    // like CACHES beside it — the same pattern gives the same engine in every runtime.
    ("builtins/rope_regex.rs", "ENGINES", PureCache),
    ("host/treesit.rs", "TREE_CACHE", PureCache),
    ("host/treesit.rs", "PARSER_POOL", PureCache),
    ("host/gui.rs", "NAMES", PureCache),
    // Diagnostics.
    ("diagnostics/coverage.rs", "B", Diagnostic),
    ("diagnostics/coverage.rs", "HITS", Diagnostic),
    ("diagnostics/coverage.rs", "LINES", Diagnostic),
    ("diagnostics/profile.rs", "SAMPLES", Diagnostic),
    ("core/heap/features_audit.rs", "EVER_BOUND", Diagnostic),
    // The macro-shadow warning's once-per-definition set (ADR-423): it decides only whether
    // a line goes to stderr, which is the OS process's, so one runtime can only spare
    // another a duplicate line.
    ("eval/macros/resolve.rs", "WARNED", Diagnostic),
    // The tier-0 escape audit's sites already written (ADR-416 stage 1).
    ("eval/compile/vm_run_bc.rs", "SEEN", Diagnostic),
    ("core/heap/vm_cache.rs", "PUBLISHED_FAST_LINKS", Diagnostic),
    ("process/scheduler/pool.rs", "QUANTUM_LEDGER", Diagnostic),
    ("eval/compile/closure.rs", "SEEN", Diagnostic),
    ("dist.rs", "SEEN", Diagnostic),
];

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The name of the `static` declared on `line`, if it declares one.
fn declared_static(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    for visibility in ["pub(crate) ", "pub(super) ", "pub "] {
        if let Some(stripped) = rest.strip_prefix(visibility) {
            rest = stripped;
            break;
        }
    }
    let rest = rest.strip_prefix("static ")?;
    let end = rest.find(':')?;
    let name = rest[..end].trim();
    (!name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        }))
    .then_some(name)
}

/// Is a declared type a table — something holding a lock or a shared append-only vector?
fn is_table(declared_type: &str) -> bool {
    !declared_type.contains("Cell<")
        && ["Mutex", "RwLock", "boxcar", "Registry"]
            .iter()
            .any(|shape| declared_type.contains(shape))
}

/// Every table `static` as `(path, name)`, outside `thread_local!` blocks.
fn table_statics() -> Vec<(String, String)> {
    let root = source_root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read a source file");
        let relative = file
            .strip_prefix(&root)
            .expect("under the source root")
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        let mut thread_local_depth: i32 = 0;
        for (index, line) in lines.iter().enumerate() {
            if thread_local_depth > 0 || line.contains("thread_local!") {
                thread_local_depth += line.matches('{').count() as i32;
                thread_local_depth -= line.matches('}').count() as i32;
                continue;
            }
            let Some(name) = declared_static(line) else {
                continue;
            };
            // The type runs from the colon to the initialiser or the end of the item.
            let mut declared_type = line[line.find(':').expect("a typed static") + 1..].to_string();
            let mut next = index + 1;
            while !declared_type.contains('=') && !declared_type.contains(';') && next < lines.len()
            {
                declared_type.push_str(lines[next]);
                next += 1;
            }
            let declared_type = declared_type.split('=').next().unwrap_or_default();
            if is_table(declared_type) {
                found.push((relative.clone(), name.to_string()));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_process_wide_table_says_why() {
    let found = table_statics();
    assert!(
        found.len() > 20,
        "the scan found only {} table statics under {} — the scanner is broken, not the tree",
        found.len(),
        source_root().display()
    );
    let unlisted: Vec<String> = found
        .iter()
        .filter(|(path, name)| {
            !PROCESS_WIDE
                .iter()
                .any(|(listed_path, listed_name, _)| listed_path == path && listed_name == name)
        })
        .map(|(path, name)| format!("{path} {name}"))
        .collect();
    assert!(
        unlisted.is_empty(),
        "a process-wide table that does not say why it is not per runtime. If it records \
         anything about ONE runtime (its globals, load path, images, modules), make it a \
         `RuntimeCode` field instead (KI-224, KI-226); otherwise add it to PROCESS_WIDE with \
         its reason:\n{}",
        unlisted.join("\n")
    );
}

#[test]
fn every_listed_table_still_exists() {
    let found = table_statics();
    let stale: Vec<String> = PROCESS_WIDE
        .iter()
        .filter(|(path, name, _)| {
            !found
                .iter()
                .any(|(found_path, found_name)| found_path == path && found_name == name)
        })
        .map(|(path, name, reason)| format!("{path} {name} ({reason:?})"))
        .collect();
    assert!(
        stale.is_empty(),
        "PROCESS_WIDE lists a table the source no longer declares — remove the entry:\n{}",
        stale.join("\n")
    );
}

#[test]
fn the_scanner_sees_a_table_and_skips_a_counter() {
    assert_eq!(
        declared_static("pub(super) static REGISTRY: LazyLock<Registry> ="),
        Some("REGISTRY")
    );
    assert_eq!(
        declared_static("    static CACHE: OnceLock<RwLock<HashMap<Symbol, Sig>>> ="),
        Some("CACHE")
    );
    assert_eq!(declared_static("let static_value = 1;"), None);
    assert!(is_table("OnceLock<RwLock<HashMap<Symbol, Sig>>>"));
    assert!(is_table("LazyLock<boxcar::Vec<String>>"));
    assert!(!is_table("AtomicU64"));
    assert!(!is_table("std::sync::OnceLock<bool>"));
    assert!(!is_table("RefCell<HashMap<String, Symbol>>"));
}
