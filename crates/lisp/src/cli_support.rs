//! Tiny CLI helpers shared between the `brood` (language) and `nest` (project)
//! binaries. Lives in the brood lib so neither crate depends on the other —
//! both already embed `brood::Interp`.
//!
//! What goes here is *mechanism* the two binaries genuinely share (error
//! formatting, common flag parsing). Anything binary-specific stays in
//! `crates/cli/` or `crates/nest/`.

use crate::Interp;
use crate::error::LispError;
use std::path::Path;

/// The version line both binaries report from `--version`: `"<semver> (<git sha>)"`.
///
/// The sha matters more than the semver on a dev box, because **a stale binary fails by
/// agreeing with you**. There are three `brood` binaries in a normal working tree —
/// `target/release/`, `target/release-fast/` (what `make release-brood` writes) and
/// `$PREFIX/bin` (what `make install` writes) — and nothing about a wrong one looks wrong: an
/// A/B against it reads +0.0%, a lowering-witness diff comes back empty, and a binary that
/// predates a flag *ignores* that flag rather than rejecting it — which reads as a finding
/// rather than a mistake about the binary. The semver alone cannot tell two builds apart;
/// with the sha, `make doctor` diffs it against `git rev-parse --short HEAD` and says so.
///
/// `BROOD_GIT_SHA` is set by this crate's `build.rs`, so it is only in scope here — hence a
/// const in the shared crate rather than `env!` at each binary's clap attribute.
pub const VERSION_LINE: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("BROOD_GIT_SHA"), ")");

/// Format a Unix-epoch milliseconds value as a human-readable UTC timestamp
/// (`YYYY-MM-DD HH:MM:SS UTC`) so a crash-dump entry tells you *when* the run
/// was at a glance — `.brood_crash_dump` is append-only, so several runs pile up
/// and the raw epoch-ms is hard to tell apart. Pure std (Hinnant's
/// civil-from-days), no `chrono`.
fn fmt_utc_ms(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    // civil_from_days: days since 1970-01-01 → (year, month, day), UTC.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

/// The first thing `brood`'s and `nest`'s `main` do: default to a backtrace on panic (an
/// explicit `RUST_BACKTRACE=0` still wins) — a bare panic from a heap index or a
/// use-after-GC tripwire is far easier to triage with one — and append every panic to
/// `.brood_crash_dump` ([`install_crash_dump`]). Call before any thread spawns: setting an
/// environment variable is only sound while the process is single-threaded.
pub fn install_panic_reporting() {
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        // SAFETY: both binaries call this first in `main`, before any thread exists that
        // could read the environment concurrently (see the doc above).
        unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    }
    install_crash_dump();
}

/// The process-wide setup every run shares, before any interpreter exists: the worker cap,
/// `BROOD_MEM_LIMIT`/`BROOD_MEM_SOFT_LIMIT` (ADR-043), and a warning when the heap is
/// stressed or retuned, so a benchmark cannot silently measure one.
pub fn init_runtime(max_parallel: Option<usize>) {
    if let Some(n) = max_parallel {
        crate::process::set_max_parallel(n);
    }
    crate::core::alloc::init_limits_from_env();
    warn_nondefault_gc_env();
}

/// The process arguments after argv[0], lossily decoded. For a binary that reads argv
/// before clap validates it: `std::env::args()` panics outright on a non-UTF-8
/// argument, which is a filename the OS allows.
pub fn args_lossy() -> Vec<String> {
    std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

/// Install a panic hook that appends a full crash report (message + location +
/// backtrace) to `.brood_crash_dump` in the working directory, *in addition* to
/// the normal stderr output. A Rust panic in these binaries is almost always a
/// kernel-level fault — a use-after-GC tripwire, a heap index, a runtime invariant
/// — and the one-line stderr message often scrolls past (especially under a TUI /
/// `nest run` animation). The dump captures it durably with a backtrace
/// (`force_capture`, so it works even without `RUST_BACKTRACE`). Appends, so a
/// burst of worker-thread panics all land. Best-effort: a write failure is
/// swallowed (we never want the crash handler to itself panic).
///
/// **Caveat:** this catches Rust *panics*, not `SIGSEGV` (e.g. a coroutine
/// stack overflow) — a signal handler writing from an async-signal context is a
/// separate, much hairier mechanism, deliberately not done here.
pub fn install_crash_dump() {
    let prior = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Preserve the normal behaviour first (stderr message / default trace).
        prior(info);
        use std::io::Write;
        let bt = std::backtrace::Backtrace::force_capture();
        let when = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let thread = std::thread::current();
        let mut body = String::new();
        body.push_str("\n=== brood crash dump ===\n");
        body.push_str(&format!(
            "version: {} ({})\n",
            env!("CARGO_PKG_VERSION"),
            env!("BROOD_GIT_SHA")
        ));
        body.push_str(&format!(
            "when:    {} ({when} ms since epoch)\n",
            fmt_utc_ms(when)
        ));
        body.push_str(&format!(
            "thread:  {}\n",
            thread.name().unwrap_or("<unnamed>")
        ));
        body.push_str(&format!("panic:   {info}\n"));
        body.push_str(&format!("backtrace:\n{bt}\n"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(".brood_crash_dump")
            && f.write_all(body.as_bytes()).is_ok()
        {
            // Point the user at the dump (the panic message itself already
            // went to stderr via `prior`).
            eprintln!("brood: crash report appended to .brood_crash_dump");
        }
    }));
}

/// Append a **green process's death** to `.brood_crash_dump`, the same durable place a
/// Rust panic goes.
///
/// The runtime already prints `process N died: …` to stderr, but **libtest captures a
/// test's stderr and discards it when the test never completes** (KI-72), and the same
/// loss happens under a TUI, under `nest run --watch`'s animation, and anywhere else
/// stderr is a scrolling region — the reasoning behind `.brood_crash_dump` for panics.
///
/// A green process dying from a Brood error is not a Rust panic, so the panic hook never
/// covered it — and it is the *more* likely of the two to be lost, because a crashing
/// child in a fan-out is silent to its parent unless something links or monitors it.
///
/// Best-effort and never fatal: a process that cannot write its own death notice must
/// still die quietly rather than take the runtime with it.
pub fn dump_process_death(pid_descr: &str, reason: &str) {
    use std::io::Write;
    let when = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let body = format!(
        "\n=== brood process death ===\nversion: {} ({})\nwhen:    {} ({when} ms since epoch)\nprocess: {pid_descr}\nreason:  {reason}\n",
        env!("CARGO_PKG_VERSION"),
        env!("BROOD_GIT_SHA"),
        fmt_utc_ms(when),
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(".brood_crash_dump")
    {
        let _ = f.write_all(body.as_bytes());
    }
}

/// Print a one-line stderr notice if any non-default GC env knob is active, so a
/// performance benchmark can't silently measure a stressed or retuned heap. Prints
/// nothing in the normal (all-default) case, so there's zero noise on a real run.
/// Called once at binary startup. `BROOD_GC_STRESS` is the dangerous one (collect
/// at *every* safepoint — order-of-magnitude slower); the tuning knobs and trace
/// just change behaviour from the shipped defaults. (`BROOD_GC_VERIFY` is debug-only
/// and inert in a release build, but it's still worth flagging that it's set.)
pub fn warn_nondefault_gc_env() {
    const KNOBS: &[&str] = &[
        "BROOD_GC_STRESS",
        "BROOD_GC_VERIFY",
        "BROOD_GC_TRACE",
        "BROOD_GC_FLOOR",
        "BROOD_GC_TENURE",
        "BROOD_GC_MAJOR",
        // The shared-RUNTIME-region compaction floor (ADR-091) and the GC-block
        // trace — both retune/observe collection, so they belong on this list.
        "BROOD_RT_GC_FLOOR",
        "BROOD_TRACE_GCBLOCK",
        // Not GC knobs per se, but they change run behaviour and so make a
        // benchmark non-representative: the memory cap forces extra collections
        // as it's approached, and the stack budget bounds non-tail recursion.
        "BROOD_MEM_LIMIT",
        "BROOD_STACK_BUDGET",
    ];
    let active: Vec<String> = KNOBS
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| format!("{k}={v}")))
        .collect();
    if !active.is_empty() {
        eprintln!(
            "brood: note — non-default GC config active ({}); not representative for benchmarking",
            active.join(", ")
        );
    }
}

// ANSI SGR sequences for the colored diagnostic (used only when `use_color()`
// says stderr is an interactive terminal). Kept as bare `&str` — a diagnostic on
// the cold error path doesn't warrant pulling `crossterm`'s writer in, and the
// plain-text output is byte-for-byte unchanged when color is off.
const C_RESET: &str = "\x1b[0m";
const C_BOLD: &str = "\x1b[1m";
const C_DIM: &str = "\x1b[2m";
const C_ERR: &str = "\x1b[1;31m"; // bold red — the `error:` label and the caret
const C_HINT: &str = "\x1b[1;36m"; // bold cyan — the `hint:` label

/// Whether to emit ANSI color on stderr: only when stderr is a real terminal and
/// the caller hasn't opted out via `NO_COLOR` (the cross-tool convention —
/// https://no-color.org; any value, even empty, disables). Piping stderr to a
/// file or an LSP/MCP consumer therefore stays plain and editor-parseable.
fn use_color() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Print an error as a GNU `FILE:LINE:COL: message` line (editor-parseable),
/// followed — when the file and position are known — by the offending source
/// line and a caret under the column. See `docs/tooling.md`. When stderr is a
/// terminal (and `NO_COLOR` is unset) the `error:` label and caret are bold red,
/// the `hint:` label bold cyan, and the version footer dimmed — rustc-style — so
/// the actionable parts stand out; the `FILE:LINE:COL:` prefix stays uncolored so
/// it remains copy-pasteable and machine-parseable.
pub fn report_error(e: &LispError) {
    let color = use_color();
    let located = e.located();
    let mut out = String::new();
    if color {
        // Colorize just the `<kind> error:` label within the located line; the
        // `FILE:LINE:COL:` prefix and the message keep their plain text. The label
        // is produced by `Display` (via `ErrorKind::label`) and always precedes the
        // message, so the first occurrence is the label, never part of the path.
        let label = e.kind.label();
        match located.find(label) {
            Some(idx) => {
                let (prefix, rest) = located.split_at(idx);
                let message = &rest[label.len()..];
                out.push_str(&format!(
                    "{prefix}{C_ERR}{label}{C_RESET}{C_BOLD}{message}{C_RESET}\n"
                ));
            }
            None => out.push_str(&format!("{located}\n")),
        }
    } else {
        out.push_str(&format!("{located}\n"));
    }
    if let (Some(file), Some(pos)) = (&e.file, e.pos)
        && let Ok(src) = std::fs::read_to_string(file)
        && let Some(line) = src.lines().nth(pos.line.saturating_sub(1) as usize)
    {
        out.push_str(&format!("    {line}\n"));
        let pad = " ".repeat(pos.col.saturating_sub(1) as usize);
        if color {
            out.push_str(&format!("    {pad}{C_ERR}^{C_RESET}\n"));
        } else {
            out.push_str(&format!("    {pad}^\n"));
        }
    }
    // Surface the fix hint (the C-style-call nudge, the deep-recursion suggestion,
    // …) — it's the actionable half of the error, and the CLI is where a human / LLM
    // most often reads it. Structured consumers (MCP / LSP) get it via `to_value_map`.
    if let Some(hint) = &e.hint {
        if color {
            out.push_str(&format!("    {C_HINT}hint:{C_RESET} {hint}\n"));
        } else {
            out.push_str(&format!("    hint: {hint}\n"));
        }
    }
    // The call trace (innermost frame first): `at <fn> (<file>:<line>:<col>)`,
    // where the location is the call site that entered the frame. Anonymous
    // frames print as `<fn>`. Dimmed so the error line stays the focal point.
    for f in &e.trace {
        let name = f.name.unwrap_or("<fn>");
        let loc = match (&f.file, f.pos) {
            (Some(file), Some(p)) => {
                format!(
                    " ({}:{}:{})",
                    crate::error::display_path(file),
                    p.line,
                    p.col
                )
            }
            (Some(file), None) => format!(" ({})", crate::error::display_path(file)),
            (None, Some(p)) => format!(" ({}:{})", p.line, p.col),
            (None, None) => String::new(),
        };
        if color {
            out.push_str(&format!("    {C_DIM}at {name}{loc}{C_RESET}\n"));
        } else {
            out.push_str(&format!("    at {name}{loc}\n"));
        }
    }
    let footer = format!(
        "    brood {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("BROOD_GIT_SHA")
    );
    if color {
        out.push_str(&format!("{C_DIM}{footer}{C_RESET}\n"));
    } else {
        out.push_str(&format!("{footer}\n"));
    }
    write_stderr(&out);
}

/// Write `s` to real stderr without panicking on a **broken pipe** — the
/// downstream reader closed early (`nest check … | head`, a pager quit, a
/// scrolled-away TUI). The `eprintln!`/`eprint!` macros panic on `EPIPE`
/// ("failed printing to stderr: Broken pipe"), which then fires the crash-dump
/// hook and aborts the process — a spurious "crash" for what is normal Unix
/// pipe teardown. Mirror `builtins::io::write_stderr`: on a broken pipe exit
/// quietly (as the default SIGPIPE disposition would); drop any other write
/// error, since an error reporter has nowhere better to send it. Both binaries
/// use these for CLI output that a `| head`/pager can truncate.
pub fn write_stderr(s: &str) {
    write_stream(&mut std::io::stderr(), s);
}

/// The stdout twin of [`write_stderr`] — same broken-pipe discipline for the
/// streamed lines the CLI sends to stdout (e.g. `brood --check`'s stdout sink). Any
/// other failure (a full disk) is fatal: stdout is the result, and a run that lost it
/// must not exit 0 (KI-244).
pub fn write_stdout(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    match out.write_all(s.as_bytes()).and_then(|_| out.flush()) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(e) => {
            write_stderr(&format!("brood: cannot write to stdout: {e}\n"));
            std::process::exit(1);
        }
    }
}

fn write_stream(w: &mut impl std::io::Write, s: &str) {
    if let Err(e) = w.write_all(s.as_bytes()).and_then(|_| w.flush())
        && e.kind() == std::io::ErrorKind::BrokenPipe
    {
        std::process::exit(0);
    }
}

/// Run `f` on a freshly-spawned thread sized to `process::WORKER_STACK_BYTES`,
/// and join it. Both binaries' `main` need this: the tree-walking evaluator
/// recurses one (heavy, in debug) Rust frame per non-tail call, and the OS
/// default main-thread stack (~8 MiB) is too small for the stack-budget guard
/// (ADR-043) to be *uniform* with the worker stacks — deep non-tail
/// recursion on the root thread would overflow before the guard fires. Sizing
/// the root thread to `WORKER_STACK_BYTES` makes the guard behave identically on
/// the root thread and inside spawned processes.
///
/// `f` typically calls `std::process::exit` itself (the exit-code path), so
/// the return value is usually unused — but it's threaded through for callers
/// that return normally. `name` is the thread name (shown in panics / dumps).
/// Each caller keeps its own unique pre-steps (the RUST_BACKTRACE default,
/// `install_crash_dump`, `nest`'s bundle pre-check) at the call site, before
/// this.
pub fn run_on_main_stack<T, F>(name: &str, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(crate::process::WORKER_STACK_BYTES)
        .spawn(f)
        .unwrap_or_else(|error| {
            // Nothing has run yet, so there is nothing to report but the cause.
            eprintln!(
                "brood: cannot start the {name} thread ({error}); {}",
                crate::process::THREAD_LIMIT_HINT
            );
            std::process::exit(1)
        });
    // Not a plain `join`, because the thread we are standing on is a resource: macOS will
    // only run an AppKit event loop on the process MAIN thread, and winit has no escape
    // hatch there the way Wayland/X11 do (KI-125). Since the runtime already lives on the
    // thread spawned above, the main thread is free — so hand it to the GUI backend, which
    // joins on our behalf and, on a platform that needs it, hosts the event loop right here
    // if a window is ever opened. Where a dedicated GUI thread is fine this is exactly the
    // `join` it replaces, and the no-gui build's stub is literally that.
    crate::host::gui::host_main_thread(handle, name)
}

/// Arm runtime contracts for this process when `BROOD_CONTRACTS` is unset — the `nest run` /
/// `nest test` dev-mode default (ADR-381) — without exporting the variable to the
/// processes the program starts. Call before the interpreter boots.
pub fn arm_contracts_by_default() {
    crate::builtins::contracts::arm_by_default();
}

/// The per-run scratch home `nest test` armed ([`arm_test_run`]), removed at the end of
/// the run by [`remove_test_scratch_home`].
static TEST_SCRATCH_HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// How old an earlier run's scratch home must be before a new run purges it: a run of the
/// same suite beside this one (another checkout, another terminal) keeps its own.
const TEST_SCRATCH_STALE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Mark this process as a test run, and give it a private scratch home for program state
/// (ADR-602). Call before any thread or interpreter exists — it sets environment variables.
///
/// * `BROOD_TEST_RUN=1` — what `test/running?` answers from: the runner says it, rather than
///   a program guessing from what happens to be loaded. Inherited, so a program a test
///   starts is part of the run too.
/// * Unless `BROOD_TEST_HOME=keep` (or `keep_home`, `nest test --real-home`): a fresh
///   directory under the temp root — `BROOD_TEST_SCRATCH`, `test/scratch-dir` — with
///   `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_DATA_HOME` pointed at `config/`, `state/`
///   and `data/` inside it, so no program's tests write the user's real config, history or
///   data by accident. `BROOD_USER_CONFIG_HOME` keeps the real config base, for the one
///   reader that wants the user's own: `nest`'s tooling preferences. `XDG_CACHE_HOME` is
///   left alone — Brood's own caches (the stdlib image, grammars, the check cache) live
///   there, and a cache is regenerable; a fresh one per run would cost a cold start each.
///
/// Earlier runs' scratch homes older than an hour are purged first.
pub fn arm_test_run(keep_home: bool) {
    // SAFETY (all set_var below): called before any thread or interpreter is created.
    unsafe { std::env::set_var("BROOD_TEST_RUN", "1") };
    let keep = keep_home || std::env::var_os("BROOD_TEST_HOME").is_some_and(|v| v == "keep");
    if keep {
        return;
    }
    let root = std::env::temp_dir();
    purge_stale_test_scratch(&root);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch = root.join(format!("brood-test-home-{}-{nanos:x}", std::process::id()));
    let made = ["config", "state", "data"]
        .iter()
        .all(|sub| std::fs::create_dir_all(scratch.join(sub)).is_ok());
    if !made {
        eprintln!(
            "nest test: could not make a scratch home under {}; running with the real one",
            root.display()
        );
        return;
    }
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let real_config = set("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            set("HOME")
                .or_else(|| {
                    if cfg!(windows) {
                        set("USERPROFILE")
                    } else {
                        None
                    }
                })
                .map(|home| std::path::PathBuf::from(home).join(".config"))
        });
    unsafe {
        if let Some(real) = real_config {
            std::env::set_var("BROOD_USER_CONFIG_HOME", real);
        }
        std::env::set_var("BROOD_TEST_SCRATCH", &scratch);
        std::env::set_var("XDG_CONFIG_HOME", scratch.join("config"));
        std::env::set_var("XDG_STATE_HOME", scratch.join("state"));
        std::env::set_var("XDG_DATA_HOME", scratch.join("data"));
    }
    let _ = TEST_SCRATCH_HOME.set(scratch);
}

/// Remove the scratch home [`arm_test_run`] made, if it made one. Best-effort; a run that
/// dies first leaves it for the next run's purge.
pub fn remove_test_scratch_home() {
    if let Some(scratch) = TEST_SCRATCH_HOME.get() {
        let _ = std::fs::remove_dir_all(scratch);
    }
}

fn purge_stale_test_scratch(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("brood-test-home-") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > TEST_SCRATCH_STALE);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Warn once if this binary's baked-in standard library is **older than the tree it is
/// running in** — a trap that comes in several disguises.
///
/// `std/**/*.blsp` is `include_str!`'d into the binary, so an edited module is invisible to
/// the running program while sitting plainly on disk. The failure does not look like
/// staleness: a function the file clearly defines reports as *unbound*, a docstring fix
/// "doesn't take", a suite reads as failing when it passes — each read as a fact about the
/// code rather than about the binary.
///
/// The binary already knows the answer — `BROOD_STDLIB_HASH` is a content hash of every
/// baked-in `.blsp`, computed by `build.rs`. This recomputes it from the tree and compares.
/// It is only called from **development entry points** (`--test` and `nest test`), where the
/// ~1 ms of hashing is nothing against a test run, and it says nothing at all when there is
/// no tree to compare against — an installed binary run anywhere is not stale, it is
/// installed.
pub fn warn_if_stdlib_is_stale() {
    if let Some(root) = stale_stdlib_root() {
        eprintln!(
            "brood: this binary's baked-in std/ is OLDER than {} — it will ignore your \
             .blsp edits (rebuild, or `make doctor`)",
            root.display()
        );
    }
}

/// **Refuse** to run a checker from a binary whose baked-in standard library is older than
/// the tree it is running in. Exits 2 with the reason.
///
/// The checker resolves every `:use`d std module from the binary's baked-in copy, so a
/// `nest` built before a `sig` edit checks every caller against the OLD declaration: the
/// edit that should have turned a file red leaves it green, and the edit that fixed a
/// finding keeps reporting it. Unlike a test run, where the warning above at least sits
/// beside a result that is usually still right, a checker verdict from the wrong std is
/// wrong in both directions and reads exactly like a right one — so this entry point does
/// not warn, it declines. Nothing to compare against (an installed binary outside a
/// checkout) is not stale.
pub fn refuse_if_stdlib_is_stale(command: &str) {
    if let Some(root) = stale_stdlib_root() {
        eprintln!(
            "{command}: this binary's baked-in std/ is OLDER than {} — its verdict would be \
             against the OLD declarations, not the ones on disk, so it is refused. Rebuild \
             the binary (or `make doctor`) and run again.",
            root.display()
        );
        std::process::exit(2);
    }
}

/// The checkout this process runs in, when its `std/**/*.blsp` hashes differently from what
/// this binary baked in; `None` when they agree or there is no checkout to compare against.
fn stale_stdlib_root() -> Option<std::path::PathBuf> {
    let root = repo_root()?; // not inside a brood checkout: nothing to be stale against
    let tree = stdlib_tree_hash(&root)?;
    (tree != env!("BROOD_STDLIB_HASH")).then_some(root)
}

/// The workspace root above `cwd`: the nearest ancestor holding both `Cargo.toml` and a
/// `std/` directory. `None` when the process is not inside a brood checkout.
fn repo_root() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("std").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// The content hash of `std/**/*.blsp` under `root`, in the hex form `BROOD_STDLIB_HASH`
/// carries.
///
/// **This must mirror `crates/lisp/build.rs` exactly** — same FNV constants, same sorted
/// order, same "relative path bytes then file bytes" per file. `build.rs` cannot share code
/// with the crate it builds, so the algorithm is written twice; `the_runtime_and_build_time_
/// stdlib_hashes_agree` is the gate that keeps the two copies honest, and it is a real gate
/// because `cargo test` rebuilds first, so the two are always comparable when it runs.
#[doc(hidden)]
pub fn stdlib_tree_hash_for_test(root: &Path) -> Option<String> {
    stdlib_tree_hash(root)
}

fn stdlib_tree_hash(root: &Path) -> Option<String> {
    fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().is_some_and(|x| x == "blsp") {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    collect(&root.join("std"), &mut files);
    if files.is_empty() {
        return None;
    }
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        for chunk in [
            f.strip_prefix(root)
                .unwrap_or(f)
                .to_string_lossy()
                .as_bytes(),
            &std::fs::read(f).unwrap_or_default(),
        ] {
            mix_chunk(&mut hash, chunk);
        }
    }
    Some(format!("{hash:x}"))
}

/// FNV-1a, but over 8-byte WORDS rather than bytes, with the chunk's length mixed in first
/// and a byte tail for the remainder. **Mirrored verbatim in `crates/lisp/build.rs`** — see
/// `stdlib_tree_hash`'s note above; `the_runtime_and_build_time_stdlib_hashes_agree` is the
/// gate that keeps the two honest.
///
/// Words, not bytes, because hashing all of `std/` a byte at a time was a double-digit
/// share of a release `brood --check` on a one-line file (KI-150); the same function shape
/// over 1/8 the iterations removes most of it.
///
/// The length is mixed in first because a word tail makes concatenation
/// ambiguity easier to reach than a pure byte stream did (a path's last partial word and a
/// file's first bytes no longer simply run together), and it costs one multiply per chunk.
fn mix_chunk(hash: &mut u64, chunk: &[u8]) {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    *hash ^= chunk.len() as u64;
    *hash = hash.wrapping_mul(PRIME);
    let (words, tail) = chunk.as_chunks::<8>();
    for w in words {
        *hash ^= u64::from_le_bytes(*w);
        *hash = hash.wrapping_mul(PRIME);
    }
    for b in tail {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(PRIME);
    }
}

/// Read a source file or exit non-zero with a uniform `"{prog}: cannot read
/// {path}: {e}"` diagnostic. The repeated read-or-die block at every CLI file
/// entry point (`brood --test`, `brood <file>`, `nest test`). `prog` is the
/// program label for the message (e.g. `"brood"`, `"nest test"`).
pub fn read_source_or_exit(prog: &str, path: &Path) -> String {
    match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) => {
            eprintln!("{prog}: cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    }
}

/// Evaluate a file's source with `(current-file)` set to its path (restored
/// after), so runtime-error / test locations carry the file and load-time def
/// sites are recorded for `(source-location …)` (ADR-031) — the same as the
/// `load` builtin does for `require`d modules. Shared by both binaries' file
/// runners.
pub fn eval_file(interp: &mut Interp, path: &str, src: &str) -> Result<(), LispError> {
    let prev = interp.heap.set_current_file(Some(path.to_string()));
    let result = interp.eval_source(src);
    interp.heap.set_current_file(prev);
    result.map(|_| ())
}

/// Restores the terminal on drop — the panic-path backstop for a full-screen
/// TUI (`nest observe`, a bundled `ui-run` app). The Brood `term-leave` is the
/// normal teardown; this fires on an unwind so a crash never leaves the
/// terminal in raw mode / the alternate screen. (`std::process::exit` skips
/// Drop, so callers scope the guard so it drops *before* reporting an error and
/// exiting.) Restores escape sequences too — see [`RawTermGuard`] for the
/// inline-REPL variant that deliberately doesn't.
pub struct FullTermGuard;
impl Drop for FullTermGuard {
    fn drop(&mut self) {
        crate::builtins::restore_terminal();
    }
}

/// Like [`FullTermGuard`] but for the *inline* editor (`term-raw-enter`, the
/// REPL line editor): only leaves raw mode, writing no escape sequences, so a
/// piped (non-TTY) stdout stays clean on exit. The Brood `term-raw-leave` is
/// the normal teardown.
///
/// The single deliberate divergence between the two guards is this one call —
/// `restore_raw` (no escapes) vs `FullTermGuard`'s `restore_terminal` (full
/// teardown). The REPL must not emit alternate-screen / cursor escapes onto a
/// pipe; a full-screen app must.
pub struct RawTermGuard;
impl Drop for RawTermGuard {
    fn drop(&mut self) {
        crate::builtins::restore_raw();
    }
}

// ── the run pre-flight's verdict cache ────────────────────────────────────────────────
//
// `brood file.blsp` type-checks the program before it runs it, and the checker's own walk
// scales with the file: 34M instructions on the `pipeline` benchmark row (18.7% of the
// run), 20M on `reduce` (13.8%), 60M on `errors-deep`, 48M on `nqueens` — paid on EVERY
// run of an unchanged program, and growing with every checker feature (KI-150: the 0.30.1
// column refresh read the short rows "carrying today's checker cost"). A verdict is a
// function of the program's text and the binary that checks it, so the second run of the
// same text needs no walk: the warnings are replayed from
// `~/.cache/brood/run-check/<key>`, keyed on a hash of the source, `system/build-id` (the
// executable's own mtime — any rebuild), `system/stdlib-id` (every baked-in `.blsp`), the
// checking mode, and whether this process installed the stdlib image: a walk without it
// scans module sources and loads more along the way, and a verdict recorded then replayed
// those extra loads on every later run — after `nest` had built the image, a trivial
// program booted in 148M instructions instead of 81M. The same shape as `nest run`'s own pre-flight manifest
// (`project-check.blsp`, `"checks-run"`), which reuses on the file's mtime.
//
// A hit replays the check's LOADS as well as its warnings. The pre-flight loads the file's
// own references eagerly (ADR-370's drain), and those loads survive into the run (ADR-339:
// a module load is a runtime-wide fact); a run that skipped the walk would instead load
// them lazily at first use, which is correct (KI-167), but the body compiled before the
// load is marked stale and recompiled (ADR-366), and that churn costs peak RSS on even a
// trivial program. So the entry records the embedded modules `*features*` gained during
// the walk, and a hit `require`s them first — from the image, a millisecond — leaving the
// runtime in the state a walk would have left it.
//
// What it will NOT cache: a check that loaded a module from OUTSIDE the binary — a
// `(:use foo)` found on the load-path — because that file's content is not in the key
// and an edit to it must re-derive the verdict. `*features*` after the check names every
// module loaded; one that is not an embedded std module declines the write, and the
// program pays the walk it always did. `BROOD_NO_CHECK_CACHE=1` bypasses the cache both
// ways, as it does `nest check`'s.

/// A cached run pre-flight: the modules the walk loaded (to replay) and its warnings.
pub struct RunCheckEntry {
    pub loads: Vec<String>,
    pub warnings: Vec<(Option<crate::error::Pos>, String)>,
}

/// The cached verdict of the run pre-flight for `src` under this binary and mode, or
/// `None` on a miss (no cache dir, the flag, an unreadable or malformed entry).
pub fn run_check_cache_read(heap: &crate::core::heap::Heap, src: &str) -> Option<RunCheckEntry> {
    let path = run_check_cache_path(heap, src)?;
    let text = std::fs::read_to_string(&path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != RUN_CHECK_CACHE_MAGIC {
        return None;
    }
    let loads: Vec<String> = lines
        .next()?
        .strip_prefix("loads")?
        .split('\t')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect();
    let mut out = Vec::new();
    for line in lines {
        let mut parts = line.splitn(3, '\t');
        let l = parts.next()?;
        let c = parts.next()?;
        let msg = run_check_unescape(parts.next()?);
        let pos = if l == "-" {
            None
        } else {
            Some(crate::error::Pos {
                line: l.parse().ok()?,
                col: c.parse().ok()?,
            })
        };
        out.push((pos, msg));
    }
    Some(RunCheckEntry {
        loads,
        warnings: out,
    })
}

/// The modules loaded BEFORE the walk — `run_check_cache_write` subtracts these from the
/// modules loaded after it to learn what the walk itself loaded.
pub fn run_check_features_before(heap: &crate::core::heap::Heap) -> Vec<String> {
    crate::builtins::provided_features(heap)
}

/// Replay a hit's loads: `require` each module the walk had loaded, so the run starts from
/// the state a walk leaves. Best-effort — a module that will not load now fails the same
/// way at the program's own first use of it, with the program's own error.
pub fn run_check_cache_replay_loads(interp: &mut Interp, loads: &[String]) {
    for m in loads {
        if crate::builtins::is_embedded_module(m) {
            let _ = interp.eval_str(&format!("(require-one '{m})"));
        }
    }
}

/// Record the pre-flight's verdict for `src` — unless the check loaded a module from
/// outside the binary (see the module comment), or there is nowhere to write. Best-effort:
/// a cache that cannot be written is a check that runs next time, never an error.
pub fn run_check_cache_write(
    heap: &crate::core::heap::Heap,
    src: &str,
    features_before: &[String],
    warnings: &[(Option<crate::error::Pos>, String)],
) {
    let Some(path) = run_check_cache_path(heap, src) else {
        return;
    };
    if !crate::builtins::every_provided_feature_is_embedded(heap) {
        return;
    }
    let loads: Vec<String> = crate::builtins::provided_features(heap)
        .into_iter()
        .filter(|m| features_before.binary_search(m).is_err())
        .collect();
    let mut text = String::from(RUN_CHECK_CACHE_MAGIC);
    text.push('\n');
    text.push_str("loads");
    for m in &loads {
        text.push('\t');
        text.push_str(m);
    }
    text.push('\n');
    for (pos, msg) in warnings {
        match pos {
            Some(p) => text.push_str(&format!("{}\t{}\t", p.line, p.col)),
            None => text.push_str("-\t-\t"),
        }
        text.push_str(&run_check_escape(msg));
        text.push('\n');
    }
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Write-then-rename, so a reader never sees a torn entry (two runs of the same
    // program can race on the first write).
    let tmp = dir.join(format!(".{}.tmp", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    run_check_cache_prune(dir, &path);
}

const RUN_CHECK_CACHE_MAGIC: &str = "brood-run-check-v2";

/// The `BROOD_*` flags that change what a checker walk DOES — its verdict or its loads —
/// so a cached verdict is reusable only under the same values. An explicit list, not every
/// `BROOD_*` set: the first version hashed them all, and then no hit could be observed under
/// any trace flag (`BROOD_IMAGE_TRACE=1` was its own key), which is how a +4 MB RSS question
/// about the hit path went unanswerable for an hour. A new checker or loader flag belongs
/// here; a trace flag does not. Read by `brood file`'s pre-flight verdict cache (ADR-371)
/// and, through `(%check-walk-flags)`, by `nest check`'s manifests (ADR-382), so the two
/// cannot disagree about what a verdict depends on.
pub const WALK_FLAGS: &[&str] = &[
    "BROOD_NO_IMAGE_SIGS",
    "BROOD_NO_DERIVE_CACHE",
    "BROOD_NO_LAZY_LOAD",
    "BROOD_NO_STDIMAGE",
    "BROOD_NO_PRELUDE_IMAGE",
    "BROOD_CONTRACTS",
    "BROOD_COVERAGE",
    "BROOD_TIER",
];

/// `~/.cache/brood/run-check/<hash>` for `src` under this binary and checking mode; `None`
/// under `BROOD_NO_CHECK_CACHE`, or with no cache base to write under.
fn run_check_cache_path(heap: &crate::core::heap::Heap, src: &str) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    if std::env::var_os("BROOD_NO_CHECK_CACHE").is_some() {
        return None;
    }
    let dir = crate::boot::cache_dir()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    RUN_CHECK_CACHE_MAGIC.hash(&mut h);
    crate::builtins::build_id_string().hash(&mut h);
    crate::builtins::stdlib_id_string().hash(&mut h);
    crate::types::strict_checking().hash(&mut h);
    let image = heap
        .env_get(
            crate::core::value::EnvId::GLOBAL,
            crate::core::value::intern("*std-image-installed*"),
        )
        .is_some_and(crate::eval::truthy);
    image.hash(&mut h);
    // The flags that change what a walk DOES — `WALK_FLAGS`, name and value.
    // `var_os`, never `var`: that one errors on a non-UTF-8 value and a hostile environment
    // is a test (`builtin_robustness_test`) — a lossy spelling keys just as well.
    for name in WALK_FLAGS {
        let v = std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
        (name, v).hash(&mut h);
    }
    src.hash(&mut h);
    Some(dir.join("run-check").join(format!("{:016x}", h.finish())))
}

/// Best-effort prune of stale entries — every distinct program text a box ever ran leaves
/// one small file, and an old binary's entries can never hit again. Runs only on a write
/// (a miss), never on the hit path, and never touches `keep`.
///
/// **At most once an hour per cache, and to a bounded size** (KI-271). It used to sweep the
/// whole directory on every miss, deleting only entries older than a week, and a test suite
/// misses on every fresh temp script: this box's cache reached 108 173 entries (427 MB), so
/// each miss cost 108 197 `statx` calls, 0.15-0.6 s idle. Hundreds of concurrent test boots
/// doing that held the directory's lock against every other boot's `open` of its entry,
/// which is the `D:open_last_lookups` stall that left a server not listening after 20 s. A
/// stamp file now gates the sweep (one `stat` on a miss), and the sweep also trims the
/// oldest entries past [`RUN_CHECK_CACHE_MAX_ENTRIES`].
fn run_check_cache_prune(dir: &Path, keep: &Path) {
    run_check_cache_prune_every(dir, keep, RUN_CHECK_CACHE_PRUNE_EVERY);
}

const RUN_CHECK_CACHE_PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);
const RUN_CHECK_CACHE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
const RUN_CHECK_CACHE_MAX_ENTRIES: usize = 10_000;
const RUN_CHECK_CACHE_STAMP: &str = ".pruned";

/// Claim this interval's sweep, for exactly one of the processes that miss at once. A
/// check-then-write of the stamp let all thirty of thirty concurrent misses through, and
/// thirty processes deleting the same 98 000 files held every boot for 20-40 s; a
/// check-then-rename still let two through when one renamed a stamp another had just
/// recreated. So the stamp is a LOCK: an exclusive, non-blocking `try_lock` on it, and,
/// under the lock, its contents — the last sweep's time in seconds, empty for never — are
/// read and, on a claim, rewritten. A process that crashes mid-claim releases the lock with
/// its file descriptor.
fn run_check_cache_claim_sweep(
    stamp: &Path,
    now: std::time::SystemTime,
    every: std::time::Duration,
) -> bool {
    use std::io::{Read, Seek, Write};
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(stamp)
    else {
        return false;
    };
    if file.try_lock().is_err() {
        return false;
    }
    let now_secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut text = String::new();
    let last = file
        .read_to_string(&mut text)
        .ok()
        .and_then(|_| text.trim().parse::<u64>().ok());
    // A stamp in the future (the clock moved back) counts as expired, or it would stop
    // every sweep until the clock caught up.
    if let Some(last) = last
        && last <= now_secs
        && now_secs - last < every.as_secs()
    {
        return false;
    }
    file.set_len(0).is_ok()
        && file.rewind().is_ok()
        && file.write_all(now_secs.to_string().as_bytes()).is_ok()
}

fn run_check_cache_prune_every(dir: &Path, keep: &Path, every: std::time::Duration) {
    let now = std::time::SystemTime::now();
    let stamp = dir.join(RUN_CHECK_CACHE_STAMP);
    if !run_check_cache_claim_sweep(&stamp, now, every) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(std::time::SystemTime, std::path::PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p == keep || p == stamp {
            continue;
        }
        let Some(modified) = e.metadata().and_then(|m| m.modified()).ok() else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > RUN_CHECK_CACHE_MAX_AGE)
        {
            let _ = std::fs::remove_file(&p);
        } else {
            kept.push((modified, p));
        }
    }
    if kept.len() > RUN_CHECK_CACHE_MAX_ENTRIES {
        kept.sort_unstable_by_key(|(modified, _)| *modified);
        let excess = kept.len() - RUN_CHECK_CACHE_MAX_ENTRIES;
        for (_, p) in kept.into_iter().take(excess) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// One warning per line: a message may span lines (a hint under the finding), so its
/// newlines and backslashes are escaped on the way out and restored on the way in.
fn run_check_escape(msg: &str) -> String {
    msg.replace('\\', "\\\\").replace('\n', "\\n")
}

fn run_check_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod run_check_cache_tests {
    use super::*;
    use crate::core::heap::Heap;
    use crate::core::value::{EnvId, Value, intern};

    fn scratch_cache(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("brood-run-check-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn age(path: &std::path::Path, by: std::time::Duration) {
        let when = std::time::SystemTime::now() - by;
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    /// Record a sweep `secs` ago in `stamp`, the way a claim writes it.
    fn stamp_age(stamp: &std::path::Path, secs: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        std::fs::write(stamp, (now - secs).to_string()).unwrap();
    }

    /// KI-271: the sweep ran on every miss. Within the interval a second miss must not sweep
    /// (a week-old entry it would delete survives); once the stamp is older, it does.
    #[test]
    fn the_prune_sweeps_at_most_once_per_interval() {
        let dir = scratch_cache("interval");
        let keep = dir.join("keep");
        std::fs::write(&keep, "k").unwrap();
        run_check_cache_prune_every(&dir, &keep, std::time::Duration::from_secs(3600));
        let stale = dir.join("stale");
        std::fs::write(&stale, "s").unwrap();
        age(&stale, std::time::Duration::from_secs(8 * 24 * 3600));
        run_check_cache_prune_every(&dir, &keep, std::time::Duration::from_secs(3600));
        assert!(
            stale.exists(),
            "a second miss inside the interval swept again"
        );
        stamp_age(&dir.join(RUN_CHECK_CACHE_STAMP), 7200);
        run_check_cache_prune_every(&dir, &keep, std::time::Duration::from_secs(3600));
        assert!(
            !stale.exists(),
            "a sweep past the interval left a week-old entry"
        );
        assert!(keep.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Thirty concurrent misses once ran thirty sweeps over the same directory. Of sixteen
    /// threads racing for an expired stamp, and then for a missing one, exactly one wins.
    #[test]
    fn exactly_one_concurrent_miss_claims_the_sweep() {
        let dir = scratch_cache("claim");
        let stamp = dir.join(RUN_CHECK_CACHE_STAMP);
        let race = |stamp: &std::path::Path| {
            let barrier = std::sync::Barrier::new(16);
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..16)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            run_check_cache_claim_sweep(
                                stamp,
                                std::time::SystemTime::now(),
                                std::time::Duration::from_secs(3600),
                            )
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .filter(|&won| won)
                    .count()
            })
        };
        assert_eq!(
            race(&stamp),
            1,
            "a missing stamp was claimed more than once"
        );
        stamp_age(&stamp, 7200);
        assert_eq!(
            race(&stamp),
            1,
            "an expired stamp was claimed more than once"
        );
        assert_eq!(race(&stamp), 0, "a fresh stamp was claimed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A busy box fills the cache faster than entries age out; the sweep trims the oldest.
    #[test]
    fn the_prune_bounds_the_entry_count_keeping_the_newest() {
        let dir = scratch_cache("bound");
        let keep = dir.join("keep");
        std::fs::write(&keep, "k").unwrap();
        let extra = 5;
        for i in 0..RUN_CHECK_CACHE_MAX_ENTRIES + extra {
            let p = dir.join(format!("e{i:06}"));
            std::fs::write(&p, "x").unwrap();
            // e000000 is the oldest; ages fall as i rises.
            let secs = (RUN_CHECK_CACHE_MAX_ENTRIES + extra - i) as u64;
            age(&p, std::time::Duration::from_secs(secs));
        }
        run_check_cache_prune_every(&dir, &keep, std::time::Duration::ZERO);
        let left = std::fs::read_dir(&dir).unwrap().count();
        // the bounded entries, plus `keep` and the stamp
        assert_eq!(left, RUN_CHECK_CACHE_MAX_ENTRIES + 2);
        assert!(!dir.join("e000000").exists() && !dir.join(format!("e{:06}", extra - 1)).exists());
        assert!(dir.join(format!("e{extra:06}")).exists());
        assert!(keep.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// KI-238: a verdict recorded by a process WITHOUT the stdlib image must not be served
    /// to one with it. The un-imaged walk scans module sources and loads more along the way,
    /// and a hit replays those loads — every later run of a trivial program paid for seven
    /// modules it never used.
    #[test]
    fn the_run_check_key_says_whether_the_stdlib_image_is_installed() {
        let src = "(io/puts 1)";
        let mut heap = Heap::new();
        let flag = intern("*std-image-installed*");
        heap.env_define(EnvId::GLOBAL, flag, Value::nil());
        let without = run_check_cache_path(&heap, src);
        heap.env_define(EnvId::GLOBAL, flag, Value::int(128));
        let with = run_check_cache_path(&heap, src);
        assert!(without.is_some(), "no cache base to key under");
        assert_ne!(
            without, with,
            "an imaged run would replay an un-imaged run's loads"
        );
    }
}
