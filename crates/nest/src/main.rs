//! The `nest` command — Brood project tooling.
//!
//! `nest` is the project/workspace tool sitting above the `brood` language
//! binary — the `cargo`/`rustc`, `mix`/`elixir` split (ADR-028). For everyday
//! work this is the daily driver: `nest` covers scaffolding, running, testing,
//! type-checking, formatting, REPL, docs, and the MCP server. `brood` is the
//! low-level "just run the language" tool.
//!
//! `nest` is a thin Rust shell. The actual policy — name checks, templates,
//! discovery — is written in Brood (`std/project.blsp`) and driven through
//! `Interp`, keeping behaviour in the language (ADR-006).
//!
//! Subcommands:
//!
//!   nest new <name>        scaffold a new project
//!   nest run [<file>]      run :main, or `<file>` if given (project context
//!                          preloaded when inside a project)
//!   nest test [<file>...]  run the project's tests, or the listed files
//!   nest check [<file>...] type-check the project, or the listed files
//!   nest repl              project-aware REPL (sources preloaded)
//!   nest format            in-place reformat (`--check` for CI dry-run)
//!   nest doc [module]      Markdown docs (whole project or one module)
//!   nest mcp               Model Context Protocol server over stdio
//!
//! `-j N` / `--max-parallel N` caps concurrent spawned processes. Hot reload
//! lives in `nest run --watch <path>` (file or directory, repeatable).

use brood::cli_support::report_error;
use brood::Interp;
use clap::{Parser, Subcommand};

mod mcp;

#[derive(Parser, Debug)]
#[command(
    name = "nest",
    version,
    about = "Brood project tooling — the daily driver above the `brood` language binary (ADR-028).",
    propagate_version = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Cap concurrent spawned processes (0 = unlimited). Bounds a concurrent
    /// test run; see `std/test.blsp`.
    #[arg(
        short = 'j',
        long = "max-parallel",
        visible_alias = "jobs",
        value_name = "N",
        global = true
    )]
    max_parallel: Option<usize>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Scaffold a new project (project.blsp + src/ + tests/ + starter files).
    New {
        /// The project's name. Becomes the directory + `:name` in project.blsp.
        name: String,
    },

    /// Run the project's entry point, or a specific .blsp file.
    ///
    /// Inside a project: with no FILE, runs `:main` (defaults to `main/main`);
    /// with a FILE, runs that file with the project's sources pre-loaded so
    /// it can reach project modules.
    /// Outside a project: FILE is required and runs like `brood <file>`.
    Run {
        /// .blsp file to run instead of the project's `:main`.
        #[arg(value_name = "FILE")]
        file: Option<String>,

        /// Watch a file or directory; on every save re-`load`s the affected
        /// file. Repeatable. Directories are walked recursively for `.blsp`
        /// files; new files added later are picked up automatically.
        #[arg(long = "watch", value_name = "PATH")]
        watch: Vec<String>,

        /// Trailing arguments passed to the entry function as strings.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Run the project's tests, or specific test files.
    ///
    /// With no FILES: discover and run every `tests/**/*_test.blsp`.
    /// With FILES: load each (registering its cases) and run the suite once —
    /// inside a project, project sources are pre-loaded so cross-module names
    /// resolve.
    Test {
        /// Specific test files to run. Omit for project-wide discovery.
        #[arg(value_name = "FILE")]
        files: Vec<String>,
    },

    /// Advisory type-check the project, or specific files.
    ///
    /// With no FILES: walk every `.blsp` under `src/` + `tests/` and exit
    /// non-zero on any warning (CI-friendly).
    /// With FILES: check only those files.
    Check {
        /// Specific files to check. Omit for project-wide checking.
        #[arg(value_name = "FILE")]
        files: Vec<String>,
    },

    /// Start a REPL. Inside a project, every source file is pre-loaded so the
    /// project's modules are immediately callable.
    Repl,

    /// Reformat every `.blsp` under `src/` and `tests/` in place.
    Format {
        /// Don't write; exit non-zero if any file would change (CI mode).
        #[arg(long, short = 'c')]
        check: bool,
    },

    /// Emit Markdown documentation — the whole project, or one named module.
    Doc {
        /// Module name to document (a baked-in std module or one on the
        /// load-path). Omit to document the whole project.
        module: Option<String>,
    },

    /// Serve the project over Model Context Protocol on stdio so an agent
    /// (Claude Code etc.) can eval / lookup / format / expand / run tests /
    /// read docs against this project's live image (ADR-036, docs/mcp.md).
    /// Errors if cwd is not inside a Brood project.
    Mcp,

    /// Open a live process observer — a full-screen TUI listing every process on
    /// this runtime and its mailbox backlog (an Erlang-observer-style view). The
    /// first app on the M3 display/input seam (ADR-046); proves the render
    /// protocol + key loop end-to-end. Seeds a few demo processes so there is
    /// something to watch. Press `q` / Esc / Ctrl-C to quit.
    Observe,
}

fn main() {
    let cli = Cli::parse();
    // Run on an explicitly-sized large stack so the stack-budget guard (ADR-043)
    // is uniform across the root thread and spawned coroutines — see the matching
    // comment in `crates/cli/src/main.rs`. The OS default main stack (~8 MiB) is
    // too small for the heavy debug eval frames.
    let handle = std::thread::Builder::new()
        .name("nest-main".into())
        .stack_size(brood::process::CORO_STACK_BYTES)
        .spawn(move || run_main(cli))
        .expect("spawn nest-main thread");
    handle.join().expect("nest-main thread panicked");
}

fn run_main(cli: Cli) {
    if let Some(n) = cli.max_parallel {
        brood::process::set_max_parallel(n);
    }
    // Honour BROOD_MEM_LIMIT for every command; `nest test` defaults a ceiling
    // on (in cmd_test) so a runaway test can't OOM the host. `nest run`/`mcp`
    // stay unlimited unless the user opts in — the live image edits all day
    // (ADR-043).
    brood::core::alloc::init_limits_from_env();

    let mut interp = Interp::new();

    match cli.cmd {
        Cmd::Test { files } => cmd_test(&mut interp, &files),
        Cmd::Check { files } => cmd_check(&mut interp, &files),
        Cmd::New { name } => cmd_new(&mut interp, &name),
        Cmd::Format { check } => cmd_format(&mut interp, check),
        Cmd::Run { file, watch, args } => cmd_run(&mut interp, file.as_deref(), &watch, &args),
        Cmd::Doc { module } => cmd_doc(&mut interp, module.as_deref()),
        Cmd::Repl => cmd_repl(&mut interp),
        Cmd::Mcp => cmd_mcp(&mut interp),
        Cmd::Observe => cmd_observe(&mut interp),
    }
}

/// Restores the terminal on drop — the abnormal-path backstop for `nest observe`.
/// The Brood `term-leave` is the normal teardown; this guard fires on a panic
/// unwind too, so a crash never leaves the terminal in raw mode / the alternate
/// screen. (`std::process::exit` skips Drop, so `cmd_observe` scopes the guard so
/// it drops *before* it reports an error and exits.)
struct TermGuard;
impl Drop for TermGuard {
    fn drop(&mut self) {
        brood::builtins::restore_terminal();
    }
}

// ---------- subcommand handlers ----------

/// `nest test [FILES...]` — project-wide if no files, otherwise just those.
/// Single-file mode mirrors the old `brood --test` shape but with project
/// sources pre-loaded if we're inside a project, so cross-module names work.
fn cmd_test(interp: &mut Interp, files: &[String]) {
    // Default a memory ceiling on for test runs (ADR-043); an explicit
    // BROOD_MEM_LIMIT still wins (init ran first in main()).
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
    if files.is_empty() {
        // Whole-project discovery via std/project.blsp. Raises on failure,
        // so a non-zero exit falls out of the eval error.
        run(
            interp,
            "(require 'project) (load-config) (run-project-tests)",
        );
        return;
    }
    // Single-file path: mirror brood --test, but pre-load project image when
    // we're inside a project so cross-module names resolve.
    let bootstrap = if in_project() {
        "(require 'project) (load-config) (let (root (project--find-root (cwd))) \
            (when root (project-setup root) (project-load-sources root))) \
            (require 'test)"
    } else {
        "(require 'test)"
    };
    run(interp, bootstrap);
    for path in files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("nest test: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        };
        if let Err(e) = eval_file(interp, path, &src) {
            report_error(&e.or_file(path.clone()));
            std::process::exit(1);
        }
    }
    run(interp, "(run-tests)");
}

/// `nest check [FILES...]` — project-wide if no files, otherwise file-by-file.
fn cmd_check(interp: &mut Interp, files: &[String]) {
    if files.is_empty() {
        let v = run_for_value(
            interp,
            "(require 'project) (load-config) (require 'test) (check-project)",
        );
        match v {
            brood::core::value::Value::Int(0) => {}
            brood::core::value::Value::Int(_) => std::process::exit(1),
            other => {
                eprintln!(
                    "nest check: check-project returned a non-integer ({})",
                    interp.print(other)
                );
                std::process::exit(1);
            }
        }
        return;
    }
    // Single-file path: print warnings to stdout (CI-friendly), exit non-zero
    // if any file warned.
    let mut warned = false;
    for path in files {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("nest check: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        };
        let forms = match brood::syntax::reader::read_all_positioned(&mut interp.heap, &src) {
            Ok(forms) => forms,
            Err(e) => {
                report_error(&e.clone().or_file(path.to_string()));
                warned = true;
                continue;
            }
        };
        let just_forms: Vec<_> = forms.into_iter().map(|(f, _)| f).collect();
        let warnings = brood::types::check::check_file(&mut interp.heap, &just_forms);
        if !warnings.is_empty() {
            warned = true;
        }
        for (pos, msg) in warnings {
            match pos {
                Some(p) => println!("{}:{}:{}: warning: {}", path, p.line, p.col, msg),
                None => println!("{}: warning: {}", path, msg),
            }
        }
    }
    if warned {
        std::process::exit(1);
    }
}

/// `nest new <name>` — delegates to `(new-project name)` in std/project.blsp.
fn cmd_new(interp: &mut Interp, name: &str) {
    let escaped = brood::introspect::escape_brood_string(name);
    let code = format!(
        "(require 'project) (load-config) (new-project \"{}\")",
        escaped
    );
    run(interp, &code);
}

/// `nest format [--check]` — reformat in place, or dry-run on `--check`.
fn cmd_format(interp: &mut Interp, check: bool) {
    let entry = if check {
        "(format-project-check)"
    } else {
        "(format-project)"
    };
    let code = format!(
        "(require 'project) (load-config) (require 'format) {}",
        entry
    );
    run(interp, &code);
}

/// `nest run [FILE] [--watch PATH]... [args...]` — the entry point.
///
/// If no FILE is given but exactly one `--watch` path is a regular file,
/// promote it to the entry — so `nest run --watch src/foo.blsp` reads as
/// "run foo.blsp and hot-reload it on save", matching the most natural
/// reading. With a directory or multiple watch paths there's no unambiguous
/// promotion, so we fall through to running `:main` and watching alongside.
fn cmd_run(interp: &mut Interp, file: Option<&str>, watch: &[String], args: &[String]) {
    let promoted: Option<String> = if file.is_none() && watch.len() == 1 {
        let p = &watch[0];
        match std::fs::metadata(p) {
            Ok(meta) if !meta.is_dir() => Some(p.clone()),
            _ => None,
        }
    } else {
        None
    };
    let file: Option<&str> = file.or(promoted.as_deref());

    let escaped_args = args
        .iter()
        .map(|a| format!("\"{}\"", brood::introspect::escape_brood_string(a)))
        .collect::<Vec<_>>()
        .join(" ");

    let watch_setup = if watch.is_empty() {
        String::new()
    } else {
        let calls = watch
            .iter()
            .map(|p| {
                format!(
                    "(reload-on-change \"{}\")",
                    brood::introspect::escape_brood_string(p)
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("(require 'reload) {}", calls)
    };

    // With `--watch`, wrap the user's program in a supervised process and
    // park the root thread on its monitor. The supervisor catches throws so
    // a save with a typo doesn't kill the session; the root parks on
    // `(receive [:down …])` so it's there to print the final exit reason
    // when the supervised process really gives up (Erlang intensity
    // exceeded). Without `--watch`, run inline — plain script, let-it-crash.
    //
    // `__nest-supervised` is the supervised pid we expose so a `--watch`
    // session can be introspected (`(list-processes)` shows it). The
    // wrapping is invisible to the user's code: their file still sees the
    // global env, their `(spawn …)` calls are unsupervised by default.
    let wrap = !watch.is_empty();
    let run_form: String = match file {
        // No FILE: run the project's :main via std/project.blsp.
        None => format!("(run-project (list {}))", escaped_args),
        // FILE: run that file. Inside a project, set up the project so its
        // `src/` is on `*load-path*` (the file can `(require 'foo)` other
        // project modules), but *don't* eager-load every source — otherwise a
        // file under `src/` would run twice (once via the walker, once via the
        // explicit `load`). Outside a project, plain `brood <file>`.
        Some(path) => {
            let escaped_path = brood::introspect::escape_brood_string(path);
            format!("(load \"{}\")", escaped_path)
        }
    };
    let project_setup = if file.is_none() {
        "(require 'project) (load-config) ".to_string()
    } else if in_project() {
        "(require 'project) (load-config) \
         (let (root (project--find-root (cwd))) \
           (when root (project-setup root))) "
            .to_string()
    } else {
        String::new()
    };
    let body = if wrap {
        // Park the root on a monitor of the spawned process so the script
        // doesn't return before the user's program does — and the root sees
        // `[:down …]` if it dies. Erlang let-it-crash: a throw kills the
        // process and the `--watch` session exits with the reason. (Auto-
        // retry-with-state was removed alongside the supervisor scaffolding;
        // edit the file again to spawn a fresh attempt.)
        format!(
            "(let (p (%spawn (fn () {}))) \
                  (monitor p) \
                  (receive ([:down _ ~p reason] (println \"[exit]\" reason))))",
            run_form
        )
    } else {
        run_form
    };
    let code = format!("{}{} {}", project_setup, watch_setup, body);
    run(interp, &code);
}

/// `nest doc [module]` — Markdown docs to stdout.
fn cmd_doc(interp: &mut Interp, module: Option<&str>) {
    let code = match module {
        Some(name) => {
            let escaped = brood::introspect::escape_brood_string(name);
            format!("(require 'docs) (generate-docs \"{}\")", escaped)
        }
        None => "(require 'docs) (generate-docs)".to_string(),
    };
    run(interp, &code);
}

/// `nest repl` — project-aware REPL. Inside a project, pre-load every source
/// file so the project's modules are immediately callable from the prompt.
/// Outside a project, fall through to the plain language REPL (same UX as
/// `brood`). The REPL itself is `brood_repl` — one implementation shared
/// across both binaries.
fn cmd_repl(interp: &mut Interp) {
    if in_project() {
        run(
            interp,
            "(require 'project) (load-config) \
             (let (root (project--find-root (cwd))) \
               (when root (project-setup root) (project-load-sources root)))",
        );
        eprintln!("nest repl — project sources loaded; Ctrl-D to exit");
    } else {
        eprintln!("nest repl — no project.blsp here; plain REPL (`brood` would do the same)");
    }
    // The REPL is Brood now (`std/repl.blsp`), same as `brood` with no args.
    run(interp, "(require 'repl) (repl-run)");
}

/// `nest mcp` — see docs/mcp.md (ADR-036). Strictly per-project.
fn cmd_mcp(interp: &mut Interp) {
    let bootstrap = r#"
        (require 'project)
        (load-config)
        (let (root (project--find-root (cwd)))
          (when (nil? root)
            (error "nest mcp: not in a Brood project (no project.blsp found from " (cwd) ")"))
          (project-setup root)
          (project-load-sources root)
          (require 'test)
          (require 'format))
    "#;
    run(interp, bootstrap);
    if let Err(e) = mcp::run(interp) {
        eprintln!("nest mcp: {e}");
        std::process::exit(1);
    }
}

/// `nest observe` — the process observer TUI (ADR-046, the M3 display seam). Runs
/// the Brood observer loop in the root process (so its blocking key-poll blocks
/// only this thread, never a scheduler worker running the observed processes).
fn cmd_observe(interp: &mut Interp) {
    // The guard restores the terminal on a panic unwind; the inner scope drops it
    // (restoring) before any error is reported and we exit — `process::exit`
    // skips Drop. On the normal `q` path the Brood `term-leave` already restored;
    // the guard's second restore is idempotent.
    let result = {
        let _guard = TermGuard;
        interp.eval_str("(require 'observe) (observe-run)")
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

// ---------- helpers ----------

/// Evaluate a bootstrap snippet, reporting any error in GNU form and exiting
/// non-zero on failure.
fn run(interp: &mut Interp, code: &str) {
    if let Err(e) = interp.eval_str(code) {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Like [`run`], but returns the last value so the caller can decide whether
/// to exit non-zero based on it. Used by `nest check` to convert a non-zero
/// warning count into a non-zero exit without throwing a synthetic error.
fn run_for_value(interp: &mut Interp, code: &str) -> brood::core::value::Value {
    match interp.eval_str(code) {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    }
}

/// Evaluate a file's source with `(current-file)` set so runtime-error /
/// test locations carry the file. Mirrors the helper in `cli/main.rs`.
fn eval_file(interp: &mut Interp, path: &str, src: &str) -> Result<(), brood::error::LispError> {
    let prev = interp.heap.set_current_file(Some(path.to_string()));
    let result = interp.eval_source(src);
    interp.heap.set_current_file(prev);
    result.map(|_| ())
}

/// Walk up from cwd looking for a `project.blsp` marker. Used by the
/// single-file `nest run/test/check` paths to decide whether to bootstrap
/// the project image.
fn in_project() -> bool {
    let mut here = std::env::current_dir().ok();
    while let Some(dir) = here {
        if dir.join("project.blsp").exists() {
            return true;
        }
        here = dir.parent().map(|p| p.to_path_buf());
    }
    false
}
