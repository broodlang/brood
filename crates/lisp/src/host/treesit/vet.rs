//! A grammar library is native code this process did not build, so a bad one — a scanner
//! that dereferences null, an infinite loop, a corrupt build — takes the whole runtime with it,
//! and with it an editor's every unsaved buffer. Two defences (ADR-660):
//!
//! - **Vetting.** Before this process dlopens a library it has no verdict for, a CHILD runtime
//!   (this binary, started by itself — [`vetting_entry`]) loads it and parses a corpus: the
//!   caller's (a grammar's own test corpus), and inputs made here to reach a scanner's corners
//!   (its own tokens in every arrangement, deep nesting, a long line, invalid UTF-8, NULs,
//!   unterminated strings), each reparsed after two edits. A child that dies or stops reporting
//!   progress QUARANTINES the library; one that finishes passes it. Verdicts are kept by the
//!   library's SHA-256, so a healthy grammar is vetted once, and a reinstalled one (new bytes)
//!   afresh.
//! - **Attribution.** Input no corpus covered can still crash the runtime. Every call into a
//!   loaded grammar's code is bracketed by a thread-local mark ([`grammar_mark`]) naming it, so
//!   the fatal-signal record names the grammar (` :grammar {…}`) and the handler writes its
//!   quarantine in the same breath. The next start refuses it and says why.
//!
//! Where the verdicts live: a quarantine is STATE (`$XDG_STATE_HOME/brood/grammar-verdicts`,
//! beside ADR-635's crash records), a pass is a CACHE (`$XDG_CACHE_HOME/brood/grammar-verdicts`)
//! — it follows from the library's bytes, and losing it costs one more vetting. One file per
//! library, `<sha256>.blsp`, a flat Brood map literal. A test run keeps both under its scratch
//! home ([`VerdictDirs::from`]): it never writes the user's cache.

use super::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Set in a vetting child's environment: the binary's `main` hands the run to [`child::run`].
const CHILD_ENV: &str = "BROOD_GRAMMAR_VET_CHILD";
const LIBRARY_ENV: &str = "BROOD_GRAMMAR_VET_LIBRARY";
const LANG_ENV: &str = "BROOD_GRAMMAR_VET_LANG";
const HASH_ENV: &str = "BROOD_GRAMMAR_VET_SHA256";
const BUDGET_ENV: &str = "BROOD_GRAMMAR_VET_PARSE_MS";
/// `0` turns AUTOMATIC vetting off — an embedder's choice, or a test that wants a library
/// loaded unvetted. A quarantine still holds.
const OPT_OUT_ENV: &str = "BROOD_GRAMMAR_VET";

/// How long a vetting child may go without reporting progress before it counts as hung.
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

/// A child that could not be loaded the way the parent would refuse it too (no entry point,
/// an ABI out of range, not a library): an ordinary load error, not a verdict.
const EXIT_REFUSED: i32 = 3;
/// A child that could not vet at all (its corpus unreadable, the library changed under it).
const EXIT_NOT_RUN: i32 = 4;

/// The executable a vetting child is started from — this binary — set by [`vetting_entry`].
/// Unset in a host that never called it (an embedder, `cargo test`): it cannot vet, and loads
/// as it always did.
static VET_COMMAND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The first call in `brood`'s and `nest`'s `main`. In a vetting child (this binary started by
/// [`run_vet`]) it vets and EXITS, before any runtime, panic hook or signal handler exists — a
/// child's crash is the verdict, not a crash record. Any other run notes that it can start such
/// a child.
pub fn vetting_entry() {
    if std::env::var_os(CHILD_ENV).is_some() {
        std::process::exit(child::run());
    }
    // `/proc/self/exe` is this binary even after an upgrade unlinked its path
    let command = if cfg!(target_os = "linux") {
        Some(PathBuf::from("/proc/self/exe"))
    } else {
        std::env::current_exe().ok()
    };
    if let Some(command) = command {
        let _ = VET_COMMAND.set(command);
    }
}

// ---- where verdicts live, and a library's hash ------------------------------------------

/// `<base>/brood/grammar-verdicts`, the base `$<variable>`, else `$HOME/<fallback…>` — an
/// absolute path only, read through `environment`.
fn verdict_dir(
    environment: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    variable: &str,
    fallback: &[&str],
) -> Option<PathBuf> {
    let set = |name: &str| {
        environment(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    set(variable)
        .or_else(|| {
            set("HOME").map(|home| fallback.iter().fold(home, |path, part| path.join(part)))
        })
        .map(|base| base.join("brood").join("grammar-verdicts"))
}

/// The two verdict directories, read from the environment each time — a test run moves the
/// state directory to its scratch home after startup.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct VerdictDirs {
    /// Quarantines: state, beside the crash records.
    quarantined: Option<PathBuf>,
    /// Passes: a cache.
    passed: Option<PathBuf>,
}

impl VerdictDirs {
    fn from_env() -> VerdictDirs {
        VerdictDirs::from(&|name: &str| std::env::var_os(name))
    }

    /// The directories `environment` gives. A TEST RUN (`BROOD_TEST_RUN=1`, ADR-602) writes
    /// nothing under the user's home: its passes go under the run's scratch home
    /// (`BROOD_TEST_SCRATCH`) beside its quarantines — `XDG_CACHE_HOME` stays the user's, as
    /// the stdlib image wants it shared — and a run that kept the real home
    /// (`BROOD_TEST_HOME=keep`) keeps its passes with its quarantines, never in the cache.
    fn from(environment: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> VerdictDirs {
        let quarantined = verdict_dir(environment, "XDG_STATE_HOME", &[".local", "state"]);
        if environment("BROOD_TEST_RUN").is_some_and(|value| value == "1") {
            let scratch = environment("BROOD_TEST_SCRATCH")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute());
            let passed = match scratch {
                Some(scratch) => Some(scratch.join("cache").join("brood").join("grammar-verdicts")),
                None => quarantined.clone(),
            };
            return VerdictDirs {
                quarantined,
                passed,
            };
        }
        VerdictDirs {
            quarantined,
            passed: verdict_dir(environment, "XDG_CACHE_HOME", &[".cache"]),
        }
    }
}

/// Lowercase hex SHA-256 — the same identity a grammar's manifest records (ADR-583).
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A library's hash by its path, length and modification time — so asking a declared
/// grammar's standing (`grammar-status`, every buffer an editor opens) stats the file rather
/// than hashing megabytes.
type HashKey = (u64, Option<std::time::SystemTime>);
static LIBRARY_HASHES: LazyLock<Mutex<HashMap<PathBuf, (HashKey, String)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn hash_key(path: &Path) -> Option<HashKey> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// The SHA-256 of the library at `path`, from the cache while the file is unchanged.
pub(super) fn library_hash(path: &str) -> Result<String, String> {
    let path = Path::new(path);
    let key = hash_key(path).ok_or_else(|| format!("{}: no such file", path.display()))?;
    if let Some((cached, hash)) = crate::core::sync::lock(&LIBRARY_HASHES).get(path)
        && *cached == key
    {
        return Ok(hash.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(remember_hash(path, &bytes))
}

/// The SHA-256 of `bytes`, just read from `path`, remembered for [`library_hash`].
pub(super) fn remember_hash(path: &Path, bytes: &[u8]) -> String {
    let hash = sha256_hex(bytes);
    if let Some(key) = hash_key(path) {
        crate::core::sync::lock(&LIBRARY_HASHES).insert(path.to_path_buf(), (key, hash.clone()));
    }
    hash
}

// ---- a verdict: one flat map literal ----------------------------------------------------

/// A verdict field's value: what the flat map literal holds.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Field {
    Keyword(String),
    Text(String),
    Int(i64),
}

/// A verdict on one library: `{:verdict :passed|:quarantined :by :vet|:crash :reason "…" :lang
/// :json :library "…" :sha256 "…" :time ms …}`, kept in the order written.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Verdict {
    fields: Vec<(String, Field)>,
}

impl Verdict {
    fn new() -> Verdict {
        Verdict { fields: Vec::new() }
    }

    fn with(mut self, key: &str, field: Field) -> Verdict {
        self.fields.retain(|(existing, _)| existing != key);
        self.fields.push((key.to_string(), field));
        self
    }

    fn keyword(self, key: &str, value: &str) -> Verdict {
        self.with(key, Field::Keyword(value.to_string()))
    }

    fn text(self, key: &str, value: &str) -> Verdict {
        self.with(
            key,
            Field::Text(crate::cli_support::literal_safe(value, 600)),
        )
    }

    fn get(&self, key: &str) -> Option<&Field> {
        self.fields
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, field)| field)
    }

    fn keyword_of(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Field::Keyword(value)) => Some(value),
            _ => None,
        }
    }

    fn text_of(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Field::Text(value)) => Some(value),
            _ => None,
        }
    }

    pub(super) fn quarantined(&self) -> bool {
        self.keyword_of("verdict") == Some("quarantined")
    }

    pub(super) fn passed(&self) -> bool {
        self.keyword_of("verdict") == Some("passed")
    }

    /// Why it is quarantined, in words.
    pub(super) fn reason(&self) -> String {
        let reason = self.text_of("reason").unwrap_or("it was quarantined");
        match self.text_of("signal") {
            Some(signal) if !reason.contains(signal) => format!("{reason} ({signal})"),
            _ => reason.to_string(),
        }
    }

    fn sha256(&self) -> Option<&str> {
        self.text_of("sha256")
    }

    /// The literal, one line.
    fn to_literal(&self) -> String {
        let mut out = String::from("{");
        for (index, (key, field)) in self.fields.iter().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push(':');
            out.push_str(key);
            out.push(' ');
            match field {
                Field::Keyword(value) => {
                    out.push(':');
                    out.push_str(value);
                }
                Field::Text(value) => {
                    out.push('"');
                    out.push_str(value);
                    out.push('"');
                }
                Field::Int(value) => out.push_str(&value.to_string()),
            }
        }
        out.push_str("}\n");
        out
    }

    /// Read a literal [`to_literal`](Self::to_literal) or the crash handler wrote: keywords,
    /// strings without escapes (every string is written `literal_safe`), integers. `None` for
    /// anything else.
    fn parse(text: &str) -> Option<Verdict> {
        let mut rest = text
            .trim()
            .strip_prefix('{')?
            .strip_suffix('}')?
            .trim_start();
        let mut verdict = Verdict::new();
        while !rest.is_empty() {
            let key_end = rest
                .find(char::is_whitespace)
                .filter(|_| rest.starts_with(':'))?;
            let key = &rest[1..key_end];
            rest = rest[key_end..].trim_start();
            let field = if let Some(after) = rest.strip_prefix('"') {
                let close = after.find('"')?;
                let value = &after[..close];
                rest = &after[close + 1..];
                Field::Text(value.to_string())
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let token = &rest[..end];
                rest = &rest[end..];
                match token.strip_prefix(':') {
                    Some(keyword) if !keyword.is_empty() => Field::Keyword(keyword.to_string()),
                    _ => Field::Int(token.parse().ok()?),
                }
            };
            verdict.fields.push((key.to_string(), field));
            rest = rest.trim_start();
        }
        Some(verdict)
    }

    /// The field `key` as a Brood value, if the verdict has it.
    pub(super) fn field_value(&self, heap: &mut Heap, key: &str) -> Option<Value> {
        Some(match self.get(key)? {
            Field::Keyword(name) => Value::keyword(value::intern(name)),
            Field::Text(text) => heap.alloc_string(text),
            Field::Int(number) => Value::int(*number),
        })
    }

    /// As a Brood map.
    pub(super) fn to_value(&self, heap: &mut Heap) -> Value {
        self.to_value_with(heap, Vec::new())
    }

    /// As a Brood map, with `extra` entries the flat literal cannot hold (a boolean).
    fn to_value_with(&self, heap: &mut Heap, extra: Vec<(Value, Value)>) -> Value {
        let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(self.fields.len() + extra.len());
        for (key, _) in &self.fields {
            if let Some(field) = self.field_value(heap, key) {
                pairs.push((Value::keyword(value::intern(key)), field));
            }
        }
        pairs.extend(extra);
        heap.map_from_pairs(pairs)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The fields every verdict on `library` starts with.
fn verdict_about(lang: &str, library: &str, hash: &str) -> Verdict {
    Verdict::new()
        .keyword("lang", lang)
        .text("library", library)
        .text("sha256", hash)
        .with("time", Field::Int(now_ms()))
        .text("brood", env!("CARGO_PKG_VERSION"))
}

// ---- the store ----------------------------------------------------------------------

fn read_verdict_file(dir: Option<&Path>, hash: &str) -> Option<Verdict> {
    let text = std::fs::read_to_string(dir?.join(format!("{hash}.blsp"))).ok()?;
    Verdict::parse(&text)
}

/// The verdict kept on the library with `hash`: a quarantine first, then a pass.
pub(super) fn stored_verdict_in(dirs: &VerdictDirs, hash: &str) -> Option<Verdict> {
    read_verdict_file(dirs.quarantined.as_deref(), hash)
        .filter(Verdict::quarantined)
        .or_else(|| read_verdict_file(dirs.passed.as_deref(), hash).filter(Verdict::passed))
}

pub(super) fn stored_verdict(hash: &str) -> Option<Verdict> {
    stored_verdict_in(&VerdictDirs::from_env(), hash)
}

/// Write `content` to `path` owner-only, through a staging name renamed into place.
fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)?;
    }
    let staging = path.with_extension(format!("partial-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options
        .open(&staging)
        .and_then(|mut file| file.write_all(content.as_bytes()))
        .and_then(|()| std::fs::rename(&staging, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    written
}

/// Keep `verdict`: a quarantine replaces any pass, a pass goes to the cache. Best-effort — a
/// verdict that cannot be written costs another vetting, never the load.
pub(super) fn store_verdict_in(dirs: &VerdictDirs, verdict: &Verdict) {
    let Some(hash) = verdict.sha256() else {
        return;
    };
    let name = format!("{hash}.blsp");
    if verdict.quarantined() {
        if let Some(dir) = &dirs.quarantined {
            let _ = write_private(&dir.join(&name), &verdict.to_literal());
        }
        if let Some(dir) = &dirs.passed {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    } else if verdict.passed()
        && let Some(dir) = &dirs.passed
    {
        let _ = write_private(&dir.join(&name), &verdict.to_literal());
    }
}

fn store_verdict(verdict: &Verdict) {
    store_verdict_in(&VerdictDirs::from_env(), verdict)
}

/// Remove every verdict on the library with `hash`; true when there was one.
pub(super) fn forget_verdict_in(dirs: &VerdictDirs, hash: &str) -> bool {
    let name = format!("{hash}.blsp");
    [&dirs.quarantined, &dirs.passed]
        .into_iter()
        .flatten()
        .filter(|dir| std::fs::remove_file(dir.join(&name)).is_ok())
        .count()
        > 0
}

// ---- the mark: which grammar's code this thread is running -------------------------------

/// The mark of each runtime-loaded grammar, by language: the library, its hash and the file a
/// crash in it writes. Built when the library is loaded and leaked, as the library is.
static GRAMMAR_MARKS: LazyLock<Mutex<HashMap<String, &'static crate::cli_support::ForeignCode>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The mark for `lang`'s library at `path` with `hash`: its crash-record fragment, and its
/// quarantine as the crash file. Leaked: the signal handler may read it at any time after.
pub(super) fn build_mark(
    lang: &str,
    path: &str,
    hash: &str,
) -> &'static crate::cli_support::ForeignCode {
    let library = crate::cli_support::literal_safe(path, 300);
    let about = format!(":lang :{lang} :library \"{library}\" :sha256 \"{hash}\"");
    let mut mark = crate::cli_support::ForeignCode::new(format!(" :grammar {{{about}}}"));
    if let Some(dir) = VerdictDirs::from_env().quarantined {
        mark = mark.with_crash_file(
            &dir.join(format!("{hash}.blsp")),
            format!(
                "{{:verdict :quarantined :by :crash :reason \"it crashed a runtime\" {about} \
                 :brood \"{}\"",
                env!("CARGO_PKG_VERSION")
            ),
            "}\n".to_string(),
        );
    }
    Box::leak(Box::new(mark))
}

/// The mark of `lang`'s grammar as loaded now, if it was loaded from a library.
pub(super) fn mark_of(lang: &str) -> Option<&'static crate::cli_support::ForeignCode> {
    crate::core::sync::lock(&GRAMMAR_MARKS).get(lang).copied()
}

/// `lang`'s grammar is now the one `mark` names.
pub(super) fn set_mark(lang: &str, mark: &'static crate::cli_support::ForeignCode) {
    crate::core::sync::lock(&GRAMMAR_MARKS).insert(lang.to_string(), mark);
}

/// Mark this thread as running `lang`'s grammar code until the guard drops — around every parse
/// and parser set-up, where a grammar's lexer and external scanner run. A built-in grammar has
/// no mark (and nothing to quarantine).
pub(super) fn grammar_mark(lang: &str) -> crate::cli_support::ForeignGuard {
    crate::cli_support::enter_foreign_code(mark_of(lang))
}

// ---- vetting: the parent's side --------------------------------------------------------

/// How a vetting ended.
#[derive(Debug)]
pub(super) enum Outcome {
    /// It parsed every input: `inputs` of them.
    Passed { inputs: i64 },
    /// It crashed or hung.
    Quarantined { reason: String },
    /// It failed to load as the parent's own load would (no entry point, an ABI out of range).
    Refused { reason: String },
    /// No verdict could be reached: no way to start a child, or it could not vet.
    NotRun { reason: String },
}

/// The corpus framed for the child's stdin: each input as `<length>\n<bytes>`.
pub(super) fn encode_corpus(corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for input in corpus {
        out.extend_from_slice(input.len().to_string().as_bytes());
        out.push(b'\n');
        out.extend_from_slice(input);
    }
    out
}

pub(super) fn decode_corpus(mut bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut corpus = Vec::new();
    while !bytes.is_empty() {
        let newline = bytes.iter().position(|&byte| byte == b'\n')?;
        let length: usize = std::str::from_utf8(&bytes[..newline]).ok()?.parse().ok()?;
        let rest = &bytes[newline + 1..];
        if rest.len() < length {
            return None;
        }
        corpus.push(rest[..length].to_vec());
        bytes = &rest[length..];
    }
    Some(corpus)
}

/// What the child was doing, from its last progress line, in words.
fn describe_progress(line: &str) -> String {
    let mut words = line.split_whitespace();
    let stage = words.next().unwrap_or("");
    let index = words.next().unwrap_or("?");
    let label = words.next().unwrap_or("");
    match stage {
        "load" => "while loading".to_string(),
        "set-up" => "while being set up".to_string(),
        "parse" => format!("parsing input {index} ({label})"),
        "walk" => format!("walking the tree of input {index} ({label})"),
        "reparse" => format!("reparsing input {index} ({label}) after an edit"),
        "" => "before it reported anything".to_string(),
        _ => format!("at `{line}`"),
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(|signal| {
        let name = match signal {
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGBUS => "SIGBUS",
            libc::SIGILL => "SIGILL",
            libc::SIGFPE => "SIGFPE",
            libc::SIGABRT => "SIGABRT",
            libc::SIGKILL => "SIGKILL",
            libc::SIGTERM => "SIGTERM",
            _ => return format!("signal {signal}"),
        };
        name.to_string()
    })
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

/// Vet `lang`'s library at `path` (whose bytes hash to `hash`) in a child: the corpus, then the
/// inputs the child makes. A child that reports nothing for `timeout` is killed and counts as
/// hung. Blocks the calling worker (marked dirty-blocked meanwhile); errs only when the caller
/// is killed, taking the child with it.
pub(super) fn run_vet(
    path: &str,
    lang: &str,
    hash: &str,
    corpus: &[Vec<u8>],
    timeout: Duration,
) -> Result<Outcome, LispError> {
    use std::io::{BufRead, Read, Write};
    use std::process::{Command, Stdio};
    let Some(command) = VET_COMMAND.get() else {
        return Ok(Outcome::NotRun {
            reason: "this runtime cannot start a vetting child (only the brood and nest \
                     binaries, and the bundles they build, can)"
                .to_string(),
        });
    };
    let budget = (timeout / 4).max(Duration::from_millis(50));
    let mut cmd = Command::new(command);
    cmd.env(CHILD_ENV, "1")
        .env(LIBRARY_ENV, path)
        .env(LANG_ENV, lang)
        .env(HASH_ENV, hash)
        .env(BUDGET_ENV, budget.as_millis().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::host::subprocess::reset_inherited_signals(&mut cmd);
    #[cfg(unix)]
    crate::host::subprocess::new_session(&mut cmd);
    let _dirty = crate::process::dirty_block();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(Outcome::NotRun {
                reason: format!("the vetting child did not start: {error}"),
            });
        }
    };
    // Three helper threads, each ending when its pipe closes — which the child's exit (or the
    // kill below) guarantees, so every one is joined before this returns.
    let payload = encode_corpus(corpus);
    let writer = child.stdin.take().map(|mut stdin| {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&payload);
        })
    });
    let started = Instant::now();
    let progress = std::sync::Arc::new(Mutex::new((String::new(), started)));
    let reader = child.stdout.take().map(|stdout| {
        let progress = std::sync::Arc::clone(&progress);
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                *crate::core::sync::lock(&progress) = (line, Instant::now());
            }
        })
    });
    let errors = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.take(16 * 1024).read_to_string(&mut text);
            text
        })
    });
    let mut hung = false;
    let mut killed = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        let quiet_since = crate::core::sync::lock(&progress).1;
        if crate::process::capture_kill_pending() {
            killed = true;
        } else if quiet_since.elapsed() >= timeout {
            hung = true;
        }
        if killed || hung {
            let _ = child.kill();
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    if let Some(reader) = reader {
        let _ = reader.join();
    }
    let stderr = errors
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    if killed {
        return Err(LispError::kill_signal());
    }
    let last = crate::core::sync::lock(&progress).0.clone();
    let doing = describe_progress(&last);
    let complaint = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if hung {
        return Ok(Outcome::Quarantined {
            reason: format!(
                "it stopped answering for {} ms {doing}",
                timeout.as_millis()
            ),
        });
    }
    let Some(status) = status else {
        return Ok(Outcome::NotRun {
            reason: "the vetting child could not be waited for".to_string(),
        });
    };
    if let Some(inputs) = last.strip_prefix("ok ").filter(|_| status.success()) {
        return Ok(Outcome::Passed {
            inputs: inputs.trim().parse().unwrap_or(0),
        });
    }
    if let Some(signal) = signal_of(&status) {
        return Ok(Outcome::Quarantined {
            reason: format!("it crashed ({signal}) {doing}"),
        });
    }
    Ok(match status.code() {
        Some(EXIT_REFUSED) => Outcome::Refused { reason: complaint },
        Some(EXIT_NOT_RUN) => Outcome::NotRun { reason: complaint },
        Some(code) => Outcome::Quarantined {
            reason: if complaint.is_empty() {
                format!("it failed (exit status {code}) {doing}")
            } else {
                format!("it failed (exit status {code}) {doing}: {complaint}")
            },
        },
        None => Outcome::NotRun {
            reason: "the vetting child ended without a status".to_string(),
        },
    })
}

/// The verdict an outcome is: stored for a pass or a quarantine, answered for every one.
fn outcome_verdict(outcome: Outcome, lang: &str, path: &str, hash: &str) -> Verdict {
    let about = verdict_about(lang, path, hash);
    match outcome {
        Outcome::Passed { inputs } => {
            let verdict = about
                .keyword("verdict", "passed")
                .keyword("by", "vet")
                .with("inputs", Field::Int(inputs));
            store_verdict(&verdict);
            verdict
        }
        Outcome::Quarantined { reason } => {
            let verdict = about
                .keyword("verdict", "quarantined")
                .keyword("by", "vet")
                .text("reason", &reason);
            store_verdict(&verdict);
            verdict
        }
        Outcome::Refused { reason } => about.keyword("verdict", "refused").text("reason", &reason),
        Outcome::NotRun { reason } => about.keyword("verdict", "unvetted").text("reason", &reason),
    }
}

/// The message a load of a quarantined library fails with.
fn quarantine_error(lang: &str, path: &str, verdict: &Verdict) -> LispError {
    LispError::runtime(format!(
        "the :{lang} grammar at {path} is quarantined: {} — not loaded. A reinstalled \
         library is vetted afresh; (editor/treesit/forget-grammar-verdict path) loads this \
         one anyway",
        verdict.reason()
    ))
}

/// Whether a load with no verdict vets first: this binary can start a child, and
/// `BROOD_GRAMMAR_VET` is not `0`.
fn automatic_vetting() -> bool {
    VET_COMMAND.get().is_some()
        && std::env::var(OPT_OUT_ENV).map_or(true, |value| value.trim() != "0")
}

/// The gate in front of every dlopen of a grammar library in this process: `bytes` are the
/// library's at `path`. A quarantined library is refused with its reason; one with no verdict
/// is vetted first (when this binary can), and a pass or quarantine kept. Answers the hash.
pub(super) fn admit(path: &str, lang: &str, bytes: &[u8]) -> Result<String, LispError> {
    let hash = remember_hash(Path::new(path), bytes);
    if let Some(verdict) = stored_verdict(&hash) {
        return if verdict.quarantined() {
            Err(quarantine_error(lang, path, &verdict))
        } else {
            Ok(hash)
        };
    }
    if !automatic_vetting() {
        return Ok(hash);
    }
    let outcome = run_vet(
        path,
        lang,
        &hash,
        &[],
        Duration::from_millis(DEFAULT_TIMEOUT_MS),
    )?;
    let verdict = outcome_verdict(outcome, lang, path, &hash);
    if verdict.quarantined() {
        Err(quarantine_error(lang, path, &verdict))
    } else {
        // a refusal is reported by the load that follows, as before; an unvetted library
        // loads as it always did, still attributed should it crash
        Ok(hash)
    }
}

/// The quarantine on the library at `path`, if it has one — what `grammar-status` reports for
/// a declared grammar. Never loads anything.
pub(super) fn quarantine_of(path: &str) -> Option<Verdict> {
    let hash = library_hash(path).ok()?;
    stored_verdict(&hash).filter(Verdict::quarantined)
}

/// `(%tree-sitter-vet-grammar path lang opts)` — vet the library at `path` as `lang`'s grammar
/// in a child (ADR-660) and answer the verdict: `{:verdict :passed|:quarantined|:refused
/// |:unvetted :reason … :by :vet|:crash :lang :library :sha256 :time :inputs :cached}`. A kept
/// verdict is answered (`:cached true`) unless `force`. Loads nothing into this process.
pub fn vet_grammar(
    heap: &mut Heap,
    path: &str,
    lang: &str,
    corpus: &[Vec<u8>],
    timeout: Duration,
    force: bool,
) -> LispResult {
    let bytes = std::fs::read(path)
        .map_err(|e| LispError::runtime(format!("%tree-sitter-vet-grammar:{path}: {e}")))?;
    let hash = remember_hash(Path::new(path), &bytes);
    let cached = Value::keyword(value::intern("cached"));
    if !force && let Some(verdict) = stored_verdict(&hash) {
        return Ok(verdict.to_value_with(heap, vec![(cached, Value::Bool(true))]));
    }
    let outcome = run_vet(path, lang, &hash, corpus, timeout)?;
    Ok(outcome_verdict(outcome, lang, path, &hash)
        .to_value_with(heap, vec![(cached, Value::Bool(false))]))
}

/// `(%tree-sitter-grammar-verdict path)` — the verdict kept on the library at `path` as it is
/// now, or nil (never vetted, or vetted as other bytes).
pub fn grammar_verdict(heap: &mut Heap, path: &str) -> LispResult {
    Ok(
        match library_hash(path)
            .ok()
            .and_then(|hash| stored_verdict(&hash))
        {
            Some(verdict) => verdict.to_value(heap),
            None => Value::Nil,
        },
    )
}

/// `(%tree-sitter-forget-grammar-verdict path)` — drop the verdict on the library at `path`, so
/// its next load vets it again; true when there was one.
pub fn forget_grammar_verdict(path: &str) -> LispResult {
    Ok(Value::Bool(match library_hash(path) {
        Ok(hash) => forget_verdict_in(&VerdictDirs::from_env(), &hash),
        Err(_) => false,
    }))
}

// ---- vetting: the child's side ---------------------------------------------------------

mod child {
    use super::*;
    use std::io::{Read, Write};

    /// One stage reached, for the parent's watchdog and its account of where things stopped.
    fn progress(line: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }

    pub(super) fn run() -> i32 {
        match vet() {
            Ok(inputs) => {
                progress(&format!("ok {inputs}"));
                0
            }
            Err((code, message)) => {
                eprintln!("{message}");
                code
            }
        }
    }

    fn setting(name: &str) -> Result<String, (i32, String)> {
        std::env::var(name).map_err(|_| (EXIT_NOT_RUN, format!("{name} is not set")))
    }

    fn vet() -> Result<usize, (i32, String)> {
        let path = setting(LIBRARY_ENV)?;
        let lang = setting(LANG_ENV)?;
        let expected = setting(HASH_ENV)?;
        let budget = Duration::from_millis(
            std::env::var(BUDGET_ENV)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_TIMEOUT_MS / 4),
        );
        let mut payload = Vec::new();
        std::io::stdin()
            .read_to_end(&mut payload)
            .map_err(|e| (EXIT_NOT_RUN, format!("the corpus could not be read: {e}")))?;
        let corpus = decode_corpus(&payload)
            .ok_or_else(|| (EXIT_NOT_RUN, "the corpus is malformed".to_string()))?;
        let bytes = std::fs::read(&path).map_err(|e| (EXIT_REFUSED, format!("{path}: {e}")))?;
        if sha256_hex(&bytes) != expected {
            return Err((
                EXIT_NOT_RUN,
                format!("{path} changed while it was being vetted"),
            ));
        }
        progress("load");
        let symbol = format!("tree_sitter_{}", lang.replace('-', "_"));
        // SAFETY: what vetting is for — running the library's initialisers and entry point in a
        // process whose death is the answer.
        let language = unsafe {
            let library = libloading::Library::new(&path)
                .map_err(|e| (EXIT_REFUSED, format!("{path}: {e}")))?;
            let entry: libloading::Symbol<unsafe extern "C" fn() -> *const ()> = library
                .get(symbol.as_bytes())
                .map_err(|e| (EXIT_REFUSED, format!("{path} exports no {symbol}: {e}")))?;
            let function = *entry;
            std::mem::forget(library);
            tree_sitter::Language::new(tree_sitter_language::LanguageFn::from_raw(function))
        };
        progress("set-up");
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).map_err(|e| {
            (
                EXIT_REFUSED,
                format!(
                    "{path}: {lang} is not usable by this runtime (grammar ABI {}): {e}",
                    language.abi_version()
                ),
            )
        })?;
        let mut inputs = generated_inputs(&language);
        inputs.extend(
            corpus
                .into_iter()
                .enumerate()
                .map(|(index, input)| (format!("corpus-{index}"), input)),
        );
        for (index, (label, input)) in inputs.iter().enumerate() {
            progress(&format!("parse {index} {label}"));
            let Some(tree) = parse(&mut parser, input, None, budget) else {
                continue; // over budget: slow is not a fault
            };
            progress(&format!("walk {index} {label}"));
            walk(&tree);
            progress(&format!("reparse {index} {label}"));
            reparse_after_edits(&mut parser, input, &tree, budget);
        }
        Ok(inputs.len())
    }

    /// Parse `input`, giving up (not failing) past `budget` — a grammar slow on garbage is not
    /// the fault being looked for; a scanner that never returns is, and the parent's watchdog
    /// sees that.
    fn parse(
        parser: &mut tree_sitter::Parser,
        input: &[u8],
        old: Option<&tree_sitter::Tree>,
        budget: Duration,
    ) -> Option<tree_sitter::Tree> {
        let deadline = Instant::now() + budget;
        let mut within_budget = |_: &tree_sitter::ParseState| {
            if Instant::now() >= deadline {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        };
        let tree = parser.parse_with_options(
            &mut |byte: usize, _point: tree_sitter::Point| -> &[u8] {
                if byte >= input.len() {
                    &[]
                } else {
                    &input[byte..]
                }
            },
            old,
            Some(tree_sitter::ParseOptions::new().progress_callback(&mut within_budget)),
        );
        parser.reset();
        tree
    }

    /// Visit every node (up to a bound), reading what an editor reads: its kind, whether it is
    /// named, its field — a corrupt symbol table shows here.
    fn walk(tree: &tree_sitter::Tree) {
        let mut cursor = tree.walk();
        let mut visited = 0usize;
        loop {
            let node = cursor.node();
            std::hint::black_box((node.kind(), node.is_named(), cursor.field_name()));
            visited += 1;
            if visited > 200_000 {
                return;
            }
            if cursor.goto_first_child() || cursor.goto_next_sibling() {
                continue;
            }
            loop {
                if !cursor.goto_parent() {
                    return;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn point_at(text: &[u8], byte: usize) -> tree_sitter::Point {
        let before = &text[..byte.min(text.len())];
        let row = before.iter().filter(|&&b| b == b'\n').count();
        let column = before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(before.len(), |newline| before.len() - newline - 1);
        tree_sitter::Point::new(row, column)
    }

    /// Reparse incrementally after an insertion in the middle and after deleting the middle
    /// third — where an external scanner's serialised state is read back.
    fn reparse_after_edits(
        parser: &mut tree_sitter::Parser,
        input: &[u8],
        tree: &tree_sitter::Tree,
        budget: Duration,
    ) {
        let middle = input.len() / 2;
        let inserted: &[u8] = b" x(\"";
        let edits = [
            (middle, middle, inserted),
            (input.len() / 3, (2 * input.len()) / 3, &b""[..]),
        ];
        for (start, old_end, replacement) in edits {
            let mut text = input[..start].to_vec();
            text.extend_from_slice(replacement);
            text.extend_from_slice(&input[old_end..]);
            let new_end = start + replacement.len();
            let mut edited = tree.clone();
            edited.edit(&tree_sitter::InputEdit {
                start_byte: start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: point_at(input, start),
                old_end_position: point_at(input, old_end),
                new_end_position: point_at(&text, new_end),
            });
            if let Some(reparsed) = parse(parser, &text, Some(&edited), budget) {
                walk(&reparsed);
            }
        }
    }

    /// The grammar's own anonymous tokens (`def`, `(`, `<<~`), printable and short — the
    /// characters most likely to reach its lexer's and scanner's special cases.
    fn tokens(language: &tree_sitter::Language) -> Vec<Vec<u8>> {
        (0..language.node_kind_count())
            .filter_map(|id| u16::try_from(id).ok())
            .filter(|&id| language.node_kind_is_visible(id) && !language.node_kind_is_named(id))
            .filter_map(|id| language.node_kind_for_id(id))
            .filter(|kind| !kind.is_empty() && kind.len() <= 32 && !kind.contains(char::is_control))
            .take(400)
            .map(|kind| kind.as_bytes().to_vec())
            .collect()
    }

    fn joined(parts: &[Vec<u8>], separator: &[u8]) -> Vec<u8> {
        parts.join(separator)
    }

    /// The inputs made for every grammar: its tokens in several arrangements, nesting, a long
    /// line, invalid UTF-8, NULs, unterminated strings and comments, CRLF, many lines.
    pub(super) fn generated_inputs(language: &tree_sitter::Language) -> Vec<(String, Vec<u8>)> {
        let tokens = tokens(language);
        let mut reversed = tokens.clone();
        reversed.reverse();
        let spaced = joined(&tokens, b" ");
        let nest = |open: &str, close: &str, depth: usize| {
            let mut text = open.repeat(depth).into_bytes();
            text.push(b'x');
            text.extend(close.repeat(depth).into_bytes());
            text
        };
        let long_line = {
            let unit = if spaced.is_empty() {
                b"a + ".to_vec()
            } else {
                spaced.clone()
            };
            let mut text = Vec::new();
            while text.len() < 64 * 1024 {
                text.extend_from_slice(&unit);
                text.push(b' ');
            }
            text
        };
        let invalid_utf8 = {
            let mut text: Vec<u8> = (0x80..=0xFFu8).collect();
            text.extend_from_slice(&spaced);
            for _ in 0..64 {
                text.extend_from_slice(&[
                    0xC0, 0xAF, 0xED, 0xA0, 0x80, 0xF8, 0x88, 0x80, 0xFF, 0xFE,
                ]);
            }
            text
        };
        let noise = {
            // a fixed LCG: the same bytes on every run, so a verdict is reproducible
            let mut state: u32 = 0x5eed;
            (0..8192)
                .map(|_| {
                    state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (state >> 16) as u8
                })
                .collect::<Vec<u8>>()
        };
        let mut many_lines = "\n".repeat(20_000).into_bytes();
        many_lines.extend_from_slice(&spaced);
        let mut inputs: Vec<(String, Vec<u8>)> = vec![
            ("empty".to_string(), Vec::new()),
            ("whitespace".to_string(), b" \n\t\r\n  \n".to_vec()),
            ("tokens".to_string(), spaced.clone()),
            ("tokens-packed".to_string(), joined(&tokens, b"")),
            ("tokens-lines".to_string(), joined(&tokens, b"\n")),
            ("tokens-reversed".to_string(), joined(&reversed, b" ")),
            ("tokens-crlf".to_string(), joined(&tokens, b"\r\n")),
            ("tokens-nul".to_string(), joined(&tokens, b"\0")),
            ("nesting-parens".to_string(), nest("(", ")", 1500)),
            ("nesting-brackets".to_string(), nest("[", "]", 1500)),
            ("nesting-braces".to_string(), nest("{", "}", 1500)),
            ("nesting-mixed".to_string(), nest("([{", "}])", 500)),
            (
                "nesting-unclosed".to_string(),
                "({[".repeat(1000).into_bytes(),
            ),
            ("long-line".to_string(), long_line),
            ("long-word".to_string(), vec![b'a'; 64 * 1024]),
            ("invalid-utf8".to_string(), invalid_utf8),
            ("noise".to_string(), noise),
            ("many-lines".to_string(), many_lines),
        ];
        for (index, opener) in [
            "\"abc",
            "'abc",
            "`abc",
            "/* abc",
            "<<EOF\nabc",
            "<<~EOF\nabc",
            "r#\"abc",
            "\"\"\"abc",
            "#{abc",
            "${abc",
            "<!-- abc",
            "%{abc",
            "\\",
        ]
        .iter()
        .enumerate()
        {
            let mut text = opener.as_bytes().to_vec();
            text.push(b' ');
            text.extend_from_slice(&spaced);
            inputs.push((format!("unterminated-{index}"), text));
        }
        inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "brood-grammar-vet-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn dirs_under(root: &Path) -> VerdictDirs {
        VerdictDirs {
            quarantined: Some(root.join("state")),
            passed: Some(root.join("cache")),
        }
    }

    #[test]
    fn a_test_run_keeps_every_verdict_out_of_the_user_s_home() {
        let environment = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| std::ffi::OsString::from(value))
            }
        };
        let ordinary = VerdictDirs::from(&environment(&[("HOME", "/home/u")]));
        assert_eq!(
            ordinary,
            VerdictDirs {
                quarantined: Some(PathBuf::from("/home/u/.local/state/brood/grammar-verdicts")),
                passed: Some(PathBuf::from("/home/u/.cache/brood/grammar-verdicts")),
            }
        );
        // armed: the state home is the scratch's, and the passes go under the scratch too,
        // though XDG_CACHE_HOME is still the user's
        let armed = VerdictDirs::from(&environment(&[
            ("HOME", "/home/u"),
            ("XDG_CACHE_HOME", "/home/u/.cache"),
            ("BROOD_TEST_RUN", "1"),
            ("BROOD_TEST_SCRATCH", "/tmp/brood-test-home-1"),
            ("XDG_STATE_HOME", "/tmp/brood-test-home-1/state"),
        ]));
        assert_eq!(
            armed,
            VerdictDirs {
                quarantined: Some(PathBuf::from(
                    "/tmp/brood-test-home-1/state/brood/grammar-verdicts"
                )),
                passed: Some(PathBuf::from(
                    "/tmp/brood-test-home-1/cache/brood/grammar-verdicts"
                )),
            }
        );
        // armed with the real home kept: passes stay with the quarantines, not in the cache
        let kept = VerdictDirs::from(&environment(&[
            ("HOME", "/home/u"),
            ("BROOD_TEST_RUN", "1"),
            ("XDG_STATE_HOME", "/tmp/t/state"),
        ]));
        assert_eq!(kept.passed, kept.quarantined);
        assert_eq!(
            kept.passed,
            Some(PathBuf::from("/tmp/t/state/brood/grammar-verdicts"))
        );
    }

    #[test]
    fn a_verdict_round_trips_through_its_literal() {
        let verdict = verdict_about("json", "/g/libtree-sitter-json.so", "ab12")
            .keyword("verdict", "quarantined")
            .keyword("by", "vet")
            .text(
                "reason",
                "it crashed (SIGSEGV) parsing input 3 (\"nesting\")",
            );
        let text = verdict.to_literal();
        let read = Verdict::parse(&text).expect("the literal reads back");
        assert_eq!(read, verdict);
        assert!(read.quarantined());
        assert_eq!(
            read.reason(),
            "it crashed (SIGSEGV) parsing input 3 ('nesting')"
        );
    }

    #[test]
    fn the_crash_handler_s_file_reads_as_a_quarantine() {
        // what `build_mark`'s crash file and the handler's fields write, byte for byte
        let text = "{:verdict :quarantined :by :crash :reason \"it crashed a runtime\" :lang :json \
                    :library \"/g/x.so\" :sha256 \"ab12\" :brood \"0.1.0\" :signal \"SIGSEGV\" \
                    :time 1762560000000 :pid 4242}\n";
        let verdict = Verdict::parse(text).expect("reads");
        assert!(verdict.quarantined());
        assert_eq!(verdict.sha256(), Some("ab12"));
        assert_eq!(verdict.reason(), "it crashed a runtime (SIGSEGV)");
        assert_eq!(verdict.get("pid"), Some(&Field::Int(4242)));
    }

    #[test]
    fn not_a_verdict_is_none() {
        assert_eq!(Verdict::parse("{:verdict"), None);
        assert_eq!(Verdict::parse("{:a \"unclosed}"), None);
        assert_eq!(Verdict::parse("{:a b}"), None);
        assert_eq!(Verdict::parse("{}"), Some(Verdict::new()));
    }

    #[test]
    fn the_store_keeps_a_quarantine_over_a_pass_and_forgets_both() {
        let root = scratch("store");
        let dirs = dirs_under(&root);
        assert_eq!(stored_verdict_in(&dirs, "ab12"), None);
        let passed = verdict_about("json", "/g/x.so", "ab12")
            .keyword("verdict", "passed")
            .with("inputs", Field::Int(31));
        store_verdict_in(&dirs, &passed);
        assert!(root.join("cache").join("ab12.blsp").exists());
        assert_eq!(stored_verdict_in(&dirs, "ab12"), Some(passed));
        let quarantined = verdict_about("json", "/g/x.so", "ab12")
            .keyword("verdict", "quarantined")
            .text("reason", "it hung");
        store_verdict_in(&dirs, &quarantined);
        // the pass is gone, the quarantine kept as state
        assert!(!root.join("cache").join("ab12.blsp").exists());
        assert_eq!(stored_verdict_in(&dirs, "ab12"), Some(quarantined));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("state").join("ab12.blsp"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "a verdict is owner-only");
        }
        assert!(forget_verdict_in(&dirs, "ab12"));
        assert_eq!(stored_verdict_in(&dirs, "ab12"), None);
        assert!(!forget_verdict_in(&dirs, "ab12"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_corpus_frames_any_bytes() {
        let corpus = vec![b"".to_vec(), b"a\nb".to_vec(), vec![0, 0xFF, b'\n', 7]];
        assert_eq!(decode_corpus(&encode_corpus(&corpus)), Some(corpus));
        assert_eq!(decode_corpus(b"5\nabc"), None);
        assert_eq!(decode_corpus(b""), Some(Vec::new()));
    }

    #[test]
    fn progress_reads_as_where_it_stopped() {
        assert_eq!(describe_progress("load"), "while loading");
        assert_eq!(
            describe_progress("parse 8 nesting-parens"),
            "parsing input 8 (nesting-parens)"
        );
        assert_eq!(describe_progress(""), "before it reported anything");
    }

    #[test]
    fn a_mark_names_the_grammar_in_the_record_while_its_code_runs() {
        let mark = build_mark("vetmarktest", "/g/libtree-sitter-vetmarktest.so", "ab12");
        let fragment = String::from_utf8(mark.record_fragment().to_vec()).unwrap();
        assert_eq!(
            fragment,
            " :grammar {:lang :vetmarktest :library \"/g/libtree-sitter-vetmarktest.so\" \
             :sha256 \"ab12\"}"
        );
        set_mark("vetmarktest", mark);
        assert!(crate::cli_support::current_foreign_code().is_none());
        {
            let _outer = grammar_mark("vetmarktest");
            let current = crate::cli_support::current_foreign_code().expect("marked");
            assert!(std::ptr::eq(current, mark));
            {
                // a grammar with no mark (built in) clears it, and gives it back after
                let _inner = grammar_mark("vetmarktest-built-in");
                assert!(crate::cli_support::current_foreign_code().is_none());
            }
            assert!(crate::cli_support::current_foreign_code().is_some());
        }
        assert!(crate::cli_support::current_foreign_code().is_none());
        crate::core::sync::lock(&GRAMMAR_MARKS).remove("vetmarktest");
    }

    #[test]
    fn a_library_s_hash_is_its_sha256_and_follows_a_rewrite() {
        let root = scratch("hash");
        std::fs::create_dir_all(&root).unwrap();
        let library = root.join("libtree-sitter-x.so");
        std::fs::write(&library, b"abc").unwrap();
        let path = library.to_string_lossy().into_owned();
        assert_eq!(
            library_hash(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::write(&library, b"abcd").unwrap();
        assert_ne!(
            library_hash(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
