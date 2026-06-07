//! The `brood` command-line tool — the language binary.
//!
//! `brood` only runs the language; project tooling (scaffolding, test
//! discovery, hot-reload, MCP, etc.) lives in the separate `nest` binary —
//! the `rustc`/`cargo`, `elixir`/`mix` split (ADR-028). For everyday work
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
//!   test run; see `std/test.blsp`.
//!
//! Hot-reload lives in `nest run --watch` — the language binary has no
//! `--watch` flag. The two-file pattern (entry + helpers) is the cleanest
//! shape for hot reload (docs/shared-code.md), and `nest` is where the
//! daily-driver workflow lives.

use brood::cli_support::{report_error, run_on_main_stack, RawTermGuard};
use brood::Interp;
use clap::Parser;
use std::path::Path;

/// `brood` — the Brood language binary. Parsed by clap; the help text users
/// see is generated from these doc-comments + the `#[command]`/`#[arg]` attrs.
#[derive(Parser, Debug)]
#[command(
    name = "brood",
    version,
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

    /// Run the file(s) as a single in-language test suite (registers each
    /// `deftest`, then calls `(test/run-tests)` once). For project-wide discovery
    /// use `nest test`.
    #[arg(long, conflicts_with = "check")]
    test: bool,

    /// Type-check the file(s) without running (advisory; never gates a run).
    /// For project-wide checking use `nest check`.
    #[arg(long)]
    check: bool,

    /// Cap concurrent spawned processes (0 = unlimited). Useful for bounding
    /// a concurrent test run; see `std/test.blsp`.
    #[arg(
        short = 'j',
        long = "max-parallel",
        visible_alias = "jobs",
        value_name = "N"
    )]
    max_parallel: Option<usize>,
}

fn main() {
    // Default to a backtrace on panic — these are the dev/edit binaries, and a
    // bare panic (e.g. a heap index, a use-after-GC) is far easier to triage with
    // a trace than a one-line message. Set before any thread spawns (single-
    // threaded here, so it's sound); an explicit RUST_BACKTRACE=0 still wins.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    // Capture any panic (use-after-GC tripwire, heap index, …) to .brood_crash_dump.
    brood::cli_support::install_crash_dump();
    // A release bundle (`nest release`, ADR-038): this binary carries an app's
    // source appended to it. Boot the app and pass argv straight through to its
    // entry fn — *before* `Cli::parse`, so the app's own arguments/flags aren't
    // intercepted by `brood`'s own clap. A plain `brood` has no bundle and falls
    // through to normal CLI dispatch.
    if brood::bundle::is_bundled() {
        let args: Vec<String> = std::env::args().skip(1).collect();
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
    if let Some(n) = cli.max_parallel {
        brood::process::set_max_parallel(n);
    }
    // Honour BROOD_MEM_LIMIT / BROOD_MEM_SOFT_LIMIT for every mode; plain runs
    // and the REPL stay unlimited unless the user opts in (ADR-043). `--test`
    // additionally defaults a ceiling on (see run_test_files).
    brood::core::alloc::init_limits_from_env();
    // Flag a stressed/retuned heap so a benchmark can't silently measure one.
    brood::cli_support::warn_nondefault_gc_env();

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
        run_check_files(&mut interp, &cli.files);
        return;
    }
    if cli.test {
        run_test_files(&mut interp, &cli.files);
        brood::perf::dump_if_requested(); // BROOD_PERF_STATS=1 — VM work attribution
        return;
    }
    if !cli.files.is_empty() {
        run_files(&mut interp, &cli.files);
        brood::perf::dump_if_requested(); // BROOD_PERF_STATS=1 — VM work attribution
        return;
    }

    // The REPL is now Brood (`std/repl.blsp`): bootstrap into `(repl/repl-run)`. On a
    // TTY it raw-mode edits (std/lineedit.blsp); piped input keeps `read-line`.
    // The guard restores the terminal on a panic unwind (the Brood `term-raw-leave`
    // is the normal teardown); scope it so it drops before any error report + exit
    // (`process::exit` skips Drop). Restore is idempotent.
    let result = {
        let _guard = RawTermGuard;
        interp.eval_str("(require 'repl) (repl/repl-run)")
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Boot an embedded release bundle (ADR-038) — the path taken when this binary
/// was produced by `nest release`. Hands `args` (the app's argv) to the embedded
/// app's `:main` via `std/project.blsp`. No CLI flags, no project dir, no source
/// on disk: every module is read from the appended archive.
fn run_bundle(args: Vec<String>) {
    // Honour BROOD_MEM_LIMIT etc.; a shipped app stays unlimited unless opted in.
    brood::core::alloc::init_limits_from_env();
    let mut interp = Interp::new();
    let list = args
        .iter()
        .map(|a| format!("\"{}\"", brood::introspect::escape_brood_string(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let code = format!("(require 'project) (project/run-bundle (list {list}))");
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
    // Default a memory ceiling on for test runs so an adversarial / runaway test
    // can't OOM the host (ADR-043). An explicit BROOD_MEM_LIMIT still wins —
    // init_limits_from_env ran first in main(), so re-applying with defaults only
    // fills in what the env didn't set.
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
    // Ensure `run-tests` exists even if a file forgot to `(require 'test)`.
    if let Err(e) = interp.eval_str("(require 'test)") {
        report_error(&e);
        std::process::exit(1);
    }
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        if !no_check_env() {
            check_one_file(interp, path, &src, CheckSink::Stderr);
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

/// Where the advisory checker's warnings land — stdout for the explicit
/// `brood --check` command (the user wants the warnings), stderr for the
/// implicit pre-run check (sidebar info that shouldn't muddle stdout).
enum CheckSink {
    Stdout,
    Stderr,
}

/// Run the advisory checker over a single file's source, writing each warning
/// as a GNU `FILE:LINE:COL: warning: msg` line to the chosen sink. Returns
/// `true` when at least one warning was emitted. **Never** raises — a parse
/// error here is reported but signalled by the bool, so the caller can choose
/// whether to continue (regular run) or fail fast (`--check`).
fn check_one_file(interp: &mut Interp, path: &str, src: &str, sink: CheckSink) -> bool {
    let forms = match brood::syntax::reader::read_all_positioned(&mut interp.heap, src) {
        Ok(forms) => forms,
        Err(e) => {
            report_error(&e.clone().or_file(path.to_string()));
            return true;
        }
    };
    let just_forms: Vec<_> = forms.into_iter().map(|(f, _)| f).collect();
    let warnings = brood::types::check::check_file(&mut interp.heap, &just_forms);
    let warned = !warnings.is_empty();
    for (pos, msg) in warnings {
        let line = match pos {
            Some(p) => format!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
            None => format!("{}: warning: {}", path, msg),
        };
        match sink {
            CheckSink::Stdout => println!("{}", line),
            CheckSink::Stderr => eprintln!("{}", line),
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
    let mut warned = false;
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        if check_one_file(interp, path, &src, CheckSink::Stdout) {
            warned = true;
        }
    }
    if warned {
        std::process::exit(1);
    }
}

fn run_files(interp: &mut Interp, files: &[String]) {
    for path in files {
        let src = brood::cli_support::read_source_or_exit("brood", Path::new(path));
        // Auto-run the advisory checker before eval (stderr so it doesn't muddle
        // program stdout). `BROOD_NO_CHECK=1` opts out.
        if !no_check_env() {
            check_one_file(interp, path, &src, CheckSink::Stderr);
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
