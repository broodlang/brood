//! The `brood` command-line tool — the language binary.
//!
//! `brood` only runs the language; project tooling (scaffolding, test
//! discovery, hot-reload, MCP, etc.) lives in the separate `nest` binary —
//! the `rustc`/`cargo` split (ADR-028). For everyday work
//! reach for `nest`; `brood` is for the language-only path (REPL, run a
//! single file, one-shot test/check) outside of any project.
//!
//! Shape (all parsed by `clap`):
//!
//! - With no arguments it starts a REPL (line-edited in a terminal, plain
//!   on a pipe).
//! - With file arguments it loads and runs each file in order.
//! - `--test <file>...` loads each file (registers its cases via the test
//!   framework) and runs the in-language suite once. Project-wide test
//!   discovery is `nest test`.
//! - `--check <file>...` advisory type-check (no eval). Project-wide
//!   checking is `nest check`.
//! - `-j N` / `--max-parallel N` caps how many spawned processes run on OS
//!   threads at once (0 = unlimited, the default). Bounds a concurrent
//!   test run; see `std/tool/test.blsp`.
//!
//! Hot-reload lives in `nest run --watch` — the language binary has no
//! `--watch` flag. The two-file pattern (entry + helpers) is the cleanest
//! shape for hot reload (docs/shared-code.md), and `nest` is where the
//! daily-driver workflow lives.

use brood::Interp;
use brood::bundle::{BUNDLE_BOOT_CHECK_ARG, BUNDLE_BUILD_IMAGE_ARG, BUNDLE_BUILD_INFO_ARG};
use brood::cli_support::{RawTermGuard, report_error, run_on_main_stack};
use clap::Parser;
use std::path::Path;

/// `brood` — the Brood language binary. Parsed by clap; the help text users
/// see is generated from these doc-comments + the `#[command]`/`#[arg]` attrs.
#[derive(Parser, Debug)]
#[command(
    name = "brood",
    // The build sha, not just the semver — see `cli_support::VERSION_LINE`.
    version = brood::cli_support::VERSION_LINE,
    about = "The Brood language — language half of the brood/nest split (ADR-028).",
    long_about = "Run Brood code as a single file or a REPL, plus one-shot \
type-check and test runs. For projects (scaffolding, project-wide tests, \
hot-reload, MCP) use `nest` instead — `brood` is the rustc-style low-level \
runner; `nest` is the cargo-style daily driver."
)]
struct Cli {
    /// .blsp file(s) to run. With no files, starts a REPL.
    #[arg(value_name = "FILE")]
    files: Vec<String>,

    /// Arguments for the program, after `--`. Read with `(system/script-args)`.
    ///
    /// Without this every trailing argument parsed as another FILE, so
    /// `brood run.blsp -- --verbose` tried to open a file named `--verbose` and
    /// died. A script therefore could not take options at all, and the workaround
    /// was to configure it through the environment.
    #[arg(last = true, value_name = "ARG", allow_hyphen_values = true)]
    script_args: Vec<String>,

    /// Run the file(s) as a single in-language test suite (registers each
    /// `deftest`, then calls `(test/run-tests)` once). For project-wide discovery
    /// use `nest test`.
    #[arg(long, conflicts_with = "check")]
    test: bool,

    /// Type-check the file(s) without running (advisory; never gates a run).
    /// For project-wide checking use `nest check`.
    #[arg(long)]
    check: bool,

    /// With `--check`: strict mode — a dynamic value flowing into a precisely typed
    /// position is reported too. The flag spelling of `BROOD_CHECK_STRICT=1`, matching
    /// `nest check --strict`.
    #[arg(long, requires = "check")]
    strict: bool,

    /// List the `BROOD_*` diagnostic environment flags used for performance
    /// triage and A/B, grouped, with one line each.
    #[arg(long)]
    debug_flags: bool,

    /// Cap concurrent spawned processes (0 = unlimited). Useful for bounding
    /// a concurrent test run; see `std/tool/test.blsp`.
    #[arg(
        short = 'j',
        long = "max-parallel",
        visible_alias = "jobs",
        value_name = "N"
    )]
    max_parallel: Option<usize>,
}

fn main() {
    brood::cli_support::install_panic_reporting();
    // A release bundle (`nest release`, ADR-038): this binary carries an app's
    // source appended to it. Boot the app and pass argv straight through to its
    // entry fn — *before* `Cli::parse`, so the app's own arguments/flags aren't
    // intercepted by `brood`'s own clap. A plain `brood` has no bundle and falls
    // through to normal CLI dispatch. A bundle whose payload is corrupt is refused
    // outright: falling through would run plain `brood` in place of the app.
    if let Some(error) = brood::bundle::mount_error() {
        eprintln!("error: {error}");
        std::process::exit(70);
    }
    if brood::bundle::is_bundled() {
        // Lossy, not `std::env::args()`: this runs *before* clap, and a bundled app must
        // not die on a filename its own OS allows.
        let args = brood::cli_support::args_lossy();
        // The two runtime-reserved arguments, honoured only as the FIRST one. They are
        // the single exception to "argv goes straight to the app", and they carry the
        // `--brood-` prefix precisely so the exception costs the app nothing: the prefix
        // is reserved by the runtime, so no app has to know it exists in order to keep
        // its own `--build-info` or `--boot-check`.
        //
        // Both answer questions about the ARTIFACT, which is why they live here rather
        // than in a library the app calls: the moment they are the app's responsibility,
        // a broken app can no longer tell you what it is — and "what is this?" is asked
        // exactly when something is broken.
        match brood::bundle::reserved_command(&args) {
            Some(BUNDLE_BUILD_INFO_ARG) => {
                run_on_main_stack("brood-main", || {
                    run_bundle_meta("(project-release/build-info-report)")
                });
                return;
            }
            Some(BUNDLE_BOOT_CHECK_ARG) => {
                run_on_main_stack("brood-main", || {
                    run_bundle_meta("(project-release/check-bundle-boot)")
                });
                return;
            }
            Some(BUNDLE_BUILD_IMAGE_ARG) => {
                let Some(path) = args.get(1) else {
                    eprintln!(
                        "error: {BUNDLE_BUILD_IMAGE_ARG} needs the path to write the image to"
                    );
                    std::process::exit(2);
                };
                let code = format!(
                    "(project-release/write-bundle-image {})",
                    brood::introspect::string_literal(path)
                );
                run_on_main_stack("brood-main", move || run_bundle_meta(&code));
                return;
            }
            _ => {}
        }
        run_on_main_stack("brood-main", move || run_bundle(args));
        return;
    }
    let cli = Cli::parse();
    // Run the actual work on an explicitly-sized large stack (see
    // `cli_support::run_on_main_stack` for why). The child inherits the process
    // exit via the `std::process::exit` calls inside `run`.
    run_on_main_stack("brood-main", move || run(cli));
}

fn run(cli: Cli) {
    // Answered before anything else: it is a question about the binary, not a run, so it
    // must not depend on limits, an Interp, or a file argument being present.
    if cli.debug_flags {
        brood::diagnostics::debug_flags::print_catalogue();
        return;
    }
    // Hand the post-`--` arguments to the runtime before anything runs: only this
    // parser knows where brood's own arguments stop, so a script asking `%argv`
    // would otherwise have to re-derive it.
    brood::builtins::set_script_args(cli.script_args.clone());
    // Plain runs and the REPL stay unlimited unless the user opts in (ADR-043); `--test`
    // additionally defaults a ceiling on (see run_test_files).
    brood::cli_support::init_runtime(cli.max_parallel);
    // Started with `BROOD_TEST_ARM=1`: a test runner asked for this process to be a test run
    // with a scratch home of its own (ADR-608) — armed before the interpreter exists.
    brood::cli_support::arm_requested_test_run();

    // A bare `brood new foo` (or run/test/... ) parses `new` as a FILE and dies
    // with a cryptic "cannot read new". Project tooling lives in `nest`, not
    // `brood` (ADR-028) — so when the first arg is plainly a `nest` subcommand
    // and not a readable file, point the user there instead.
    if let Some(hint) = nest_subcommand_misuse(&cli.files) {
        eprintln!("{hint}");
        std::process::exit(2);
    }

    let mut interp = Interp::new();

    if cli.check {
        if cli.strict {
            brood::types::set_strict_checking(true);
        }
        run_check_files(&mut interp, &cli.files);
        return;
    }
    if cli.test {
        run_test_files(&mut interp, &cli.files);
        brood::diagnostics::perf::dump_if_requested(); // BROOD_PERF_STATS=1 — VM work attribution
        brood::process::l1_stats::dump_if_requested(); // BROOD_L1_STATS=1
        return;
    }
    if !cli.files.is_empty() {
        run_files(&mut interp, &cli.files);
        brood::cli_support::remove_test_scratch_home();
        brood::diagnostics::perf::dump_if_requested(); // BROOD_PERF_STATS=1 — VM work attribution
        brood::process::l1_stats::dump_if_requested(); // BROOD_L1_STATS=1
        return;
    }

    // The REPL is now Brood (`std/tool/repl.blsp`): bootstrap into `(repl/run)`. On a
    // TTY it raw-mode edits (std/editor/lineedit.blsp); piped input keeps `read-line`.
    // The guard restores the terminal on a panic unwind (the Brood `term-raw-leave`
    // is the normal teardown); scope it so it drops before any error report + exit
    // (`process::exit` skips Drop). Restore is idempotent.
    let result = {
        let _guard = RawTermGuard;
        interp.eval_str("(repl/run)")
    };
    brood::cli_support::remove_test_scratch_home();
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Run one of the reserved bundle meta-commands: evaluate `code` in a fresh interp
/// and exit non-zero if it raises. Deliberately does **not** go through
/// `run_bundle` — neither command may invoke the app's `:main`, and the build-info
/// one must answer even when the app's modules cannot load at all.
fn run_bundle_meta(code: &str) {
    brood::core::alloc::init_limits_from_env();
    let mut interp = Interp::new();
    if let Err(e) = interp.eval_str(code) {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Boot an embedded release bundle (ADR-038) — the path taken when this binary
/// was produced by `nest release`. Hands `args` (the app's argv) to the embedded
/// app's `:main` via `std/tool/project.blsp`. No CLI flags, no project dir, no source
/// on disk: every module is read from the appended archive.
fn run_bundle(args: Vec<String>) {
    // Honour BROOD_MEM_LIMIT etc.; a shipped app stays unlimited unless opted in.
    brood::core::alloc::init_limits_from_env();
    let mut interp = Interp::new();
    let list = args
        .iter()
        .map(|a| brood::introspect::string_literal(a))
        .collect::<Vec<_>>()
        .join(" ");
    let code = format!("(project-release/run-bundle (list {list}))");
    let result = {
        let _guard = RawTermGuard;
        interp.eval_str(&code)
    };
    // Restore the terminal whether the app returned or threw (a full-screen app
    // that entered raw mode / the alternate screen and then threw never reached
    // its own teardown; `process::exit` skips Drop guards).
    brood::builtins::restore_terminal_on_exit();
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// `brood --test <file>...`: load each file (registering its cases via the
/// `test` framework), then run the whole in-language suite once. Output is the
/// same structured GNU `FILE:LINE:COL:` block per failure as `nest test`; see
/// `docs/tooling.md`. Single-file run — project-wide discovery (walk to
/// `project.blsp`, load `tests/**/*_test.blsp`) is `nest test`.
fn run_test_files(interp: &mut Interp, files: &[String]) {
    if files.is_empty() {
        eprintln!("brood --test: expected a file, e.g. `brood --test foo_test.blsp`");
        std::process::exit(2);
    }
    // A test result read from a binary older than the `std/` it is testing is worse than no
    // result: it looks like an answer. This is the entry point where that has actually
    // happened, so it is the entry point that checks.
    brood::cli_support::warn_if_stdlib_is_stale();
    // Default a memory ceiling on for test runs so an adversarial / runaway test
    // can't OOM the host (ADR-043). An explicit BROOD_MEM_LIMIT still wins —
    // init_limits_from_env ran first in main(), so re-applying with defaults only
    // fills in what the env didn't set.
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
    // Ensure `run-tests` exists even if a file forgot to `(require-one 'test)`.
    if let Err(e) = interp.eval_str("(require-one 'test)") {
        report_error(&e);
        std::process::exit(1);
    }
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        if !no_check_env() {
            check_one_file(interp, path, &src, true);
        }
        if let Err(e) = brood::cli_support::eval_file(interp, path, &src) {
            // Restore the terminal first: a TUI program that entered raw mode and
            // then threw never reached its `term-raw-leave`, and `process::exit`
            // skips Drop guards. Without this the shell is left wedged in raw mode
            // after an erroring full-screen run.
            brood::builtins::restore_terminal_on_exit();
            report_error(&e.or_file(path.clone()));
            std::process::exit(1);
        }
    }
    if let Err(e) = interp.eval_str("(test/run-tests)") {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Run the advisory checker over a single file's source, writing each warning
/// as a GNU `FILE:LINE:COL: warning: msg` line. Returns `true` when at least one
/// warning was emitted. **Never** raises — a parse error here is reported but
/// signalled by the bool, so the caller can choose whether to continue (regular
/// run) or fail fast (`--check`).
///
/// `for_run` is the RUN pre-flight: warnings go to stderr (sidebar info that
/// shouldn't muddle the program's stdout), and the verdict is served from the
/// run-check cache when this binary has checked this exact text before, and
/// recorded otherwise (`cli_support::run_check_cache_read`/`_write` — what it keys
/// on, what it declines). `brood --check` writes to stdout (the user wants the
/// warnings) and never reads the cache: it is the authoritative entry point, and a
/// verdict it prints must be the walk it just did.
fn check_one_file(interp: &mut Interp, path: &str, src: &str, for_run: bool) -> bool {
    let warnings = match for_run
        .then(|| brood::cli_support::run_check_cache_read(&interp.heap, src))
        .flatten()
    {
        Some(entry) => {
            brood::cli_support::run_check_cache_replay_loads(interp, &entry.loads);
            entry.warnings
        }
        None => {
            let forms = match brood::syntax::reader::read_all_positioned(&mut interp.heap, src) {
                Ok(forms) => forms,
                Err(e) => {
                    report_error(&e.clone().or_file(path.to_string()));
                    return true;
                }
            };
            let just_forms: Vec<_> = forms.into_iter().map(|(f, _)| f).collect();
            let before = if for_run {
                brood::cli_support::run_check_features_before(&interp.heap)
            } else {
                Vec::new()
            };
            let w = brood::types::check::check_file(&mut interp.heap, &just_forms);
            if for_run {
                brood::cli_support::run_check_cache_write(&interp.heap, src, &before, &w);
            }
            w
        }
    };
    let warned = warnings
        .iter()
        .any(|(_, msg)| !msg.starts_with("checker gave up:"));
    for (pos, msg) in warnings {
        // A budget the checker ran out of is a NOTE about the check, not a finding about
        // the code (B6): printed so a run is never mistaken for a full check, worded so
        // it is not mistaken for a warning.
        let kind = if msg.starts_with("checker gave up:") {
            "note"
        } else {
            "warning"
        };
        let line = match pos {
            Some(p) => format!("{}:{}:{}: {kind}: {}", path, p.line, p.col, msg),
            None => format!("{}: {kind}: {}", path, msg),
        };
        if for_run {
            brood::cli_support::write_stderr(&format!("{line}\n"));
        } else {
            brood::cli_support::write_stdout(&format!("{line}\n"));
        }
    }
    warned
}

/// `brood --check <file>...`: advisory type-check each file. Never runs the
/// code, never gates a run; exits non-zero when warnings are emitted so CI /
/// `nest check` can use it. See `docs/types.md`.
fn run_check_files(interp: &mut Interp, files: &[String]) {
    if files.is_empty() {
        eprintln!("brood --check: expected a file, e.g. `brood --check foo.blsp`");
        std::process::exit(2);
    }
    // A checker verdict from a binary older than the `std/` it resolves against is wrong in
    // both directions and looks right (B7) — so this entry point declines rather than warns.
    brood::cli_support::refuse_if_stdlib_is_stale("brood --check");
    let mut warned = false;
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        warned |= check_one_file(interp, path, &src, false);
    }
    if warned {
        std::process::exit(1);
    }
}

fn run_files(interp: &mut Interp, files: &[String]) {
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        // Auto-run the advisory checker before eval (stderr so it doesn't muddle
        // program stdout). `BROOD_NO_CHECK=1` opts out; an unchanged program replays its
        // last verdict from the run-check cache instead of walking again.
        if !no_check_env() {
            check_one_file(interp, path, &src, true);
        }
        // Run the whole program as one green process (ADR-135): a top-level driver
        // talking to a spawned worker then uses the userspace direct-handoff path (no
        // per-message cross-thread futex) and its top-level `receive`s park-and-capture,
        // instead of the root thread blocking on its mailbox condvar.
        // The default crash reporter (ADR-305) is armed as the program's first form, in
        // its own process; `BROOD_NO_CRASH_REPORT=1` is honoured inside
        // `%crash-report-arm-default`. That arm is a PRELUDE function, not
        // `crash-report/arm-default`, and deliberately: calling into the module to decide
        // whether to arm loaded its nine-module require closure on every run, crash or not
        // (9.0 ms of ~24 ms — ADR-313). The prelude version subscribes to the kernel and
        // loads the reporter only when something actually crashes.
        if let Err(e) = interp.run_program_with_preamble(
            "(%crash-report-arm-default)",
            &src,
            Some(path.clone()),
        ) {
            // Restore the terminal first: a TUI program that entered raw mode and
            // then threw never reached its `term-raw-leave`, and `process::exit`
            // skips Drop guards. Without this the shell is left wedged in raw mode
            // after an erroring full-screen run.
            brood::builtins::restore_terminal_on_exit();
            report_error(&e);
            std::process::exit(1);
        }
    }
    // Success path too: a program may have entered raw mode and returned
    // without a matching `term-raw-leave`. No-op unless the terminal is raw.
    brood::builtins::restore_terminal_on_exit();
}

/// If the first FILE is actually a `nest` subcommand the user typed at `brood`
/// by mistake (and isn't a real file on disk), return a friendly hint pointing
/// them at `nest`. Keeps the brood/nest split clean (ADR-028) — `brood` runs
/// the language, `nest` runs the project — while turning the opaque "cannot
/// read new" into an actionable message. Returns `None` for normal file runs.
fn nest_subcommand_misuse(files: &[String]) -> Option<String> {
    const NEST_CMDS: &[&str] = &[
        "new", "run", "test", "check", "repl", "format", "doc", "mcp",
    ];
    let first = files.first()?;
    if !NEST_CMDS.contains(&first.as_str()) || std::path::Path::new(first).exists() {
        return None;
    }
    // Reconstruct the command they probably meant, e.g. `nest new foobar`.
    let suggestion = format!("nest {}", files.join(" "));
    Some(format!(
        "brood: `{first}` is a `nest` command, not a `brood` one.\n       \
         try:  {suggestion}\n\n(brood runs the language; nest runs the project — ADR-028)"
    ))
}

/// `BROOD_NO_CHECK=1` disables the implicit pre-run advisory check for the
/// rare case a caller wants raw eval (e.g. timing a hot path). Default: on.
fn no_check_env() -> bool {
    std::env::var_os("BROOD_NO_CHECK").is_some()
}
