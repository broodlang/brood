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

use brood::cli_support::report_error;
use brood::error::LispError;
use brood::Interp;
use clap::Parser;

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
    /// `deftest`, then calls `(run-tests)` once). For project-wide discovery
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
    let cli = Cli::parse();
    if let Some(n) = cli.max_parallel {
        brood::process::set_max_parallel(n);
    }

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
        return;
    }
    if !cli.files.is_empty() {
        run_files(&mut interp, &cli.files);
        return;
    }

    brood_repl::repl(&mut interp);
}

/// Evaluate a file's source with `(current-file)` set to its path (restored
/// after), so runtime-error / test locations carry the file and load-time def
/// sites are recorded for `(source-location …)` (ADR-031) — the same as the
/// `load` builtin does for `require`d modules.
fn eval_file(interp: &mut Interp, path: &str, src: &str) -> Result<(), LispError> {
    let prev = interp.heap.set_current_file(Some(path.to_string()));
    let result = interp.eval_source(src);
    interp.heap.set_current_file(prev);
    result.map(|_| ())
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
    // Ensure `run-tests` exists even if a file forgot to `(require 'test)`.
    if let Err(e) = interp.eval_str("(require 'test)") {
        report_error(&e);
        std::process::exit(1);
    }
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(src) => {
                if !no_check_env() {
                    check_one_file(interp, path, &src, CheckSink::Stderr);
                }
                if let Err(e) = eval_file(interp, path, &src) {
                    report_error(&e.or_file(path.clone()));
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("brood: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }
    if let Err(e) = interp.eval_str("(run-tests)") {
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
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("brood: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        };
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
        match std::fs::read_to_string(path) {
            Ok(src) => {
                // Auto-run the advisory checker before eval (stderr so it
                // doesn't muddle program stdout). `BROOD_NO_CHECK=1` opts out.
                if !no_check_env() {
                    check_one_file(interp, path, &src, CheckSink::Stderr);
                }
                if let Err(e) = eval_file(interp, path, &src) {
                    report_error(&e.or_file(path.clone()));
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("brood: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }
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

