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
//!                          preloaded when inside a project); `--main MOD[/FN]`
//!                          overrides the entry for one run
//!   nest test [<file>...]  run the project's tests, or the listed files
//!   nest check [<file>...] type-check the project, or the listed files
//!   nest repl              project-aware REPL (sources preloaded)
//!   nest format            in-place reformat (`--check` for CI dry-run)
//!   nest doc [module]      Markdown docs (whole project or one module);
//!                          `--all` is the complete builtin + prelude reference
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

        /// Starter template: `default` (a main+hello pair), `tui-loop` (a
        /// tail-recursive animation loop, pairs with `nest run --for`), `hatch`
        /// (a stateful gen_server-style process), or `http-server` (a basic web
        /// app over std/http). An unknown name lists the full set.
        #[arg(long = "template", short = 't', value_name = "NAME")]
        template: Option<String>,
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

        /// Run for at most this long, then exit cleanly — e.g. `2s`, `500ms`,
        /// or a bare `1500` (milliseconds). Lets a long-running loop / TUI app
        /// be exercised end-to-end and in CI without a manual `timeout`.
        #[arg(long = "for", value_name = "DURATION")]
        for_duration: Option<String>,

        /// Override the entry point for this run — `module` or `module/fn` —
        /// without editing the manifest's `:main`. Ignored when a FILE is given.
        #[arg(long = "main", value_name = "MODULE[/FN]")]
        main: Option<String>,

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

    /// Resolve the project's dependencies and write project.lock.blsp (ADR-037).
    ///
    /// For `:path` deps this verifies each sibling project exists and records its
    /// content hash; `:git` deps land in a later slice. Errors if cwd is not
    /// inside a Brood project.
    Fetch,

    /// Print the project's resolved dependency tree (root → direct → transitive).
    Tree,

    /// Add a dependency to project.blsp and re-lock (ADR-037).
    ///
    /// `nest add NAME :path PATH` (`:git` lands in a later slice). NAME is the
    /// local require-name. The manifest is rewritten preserving its comments.
    Add {
        /// The local require-name for the dependency.
        name: String,

        /// The source spec: `:path PATH` (or, later, `:git URL :ref REF`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        spec: Vec<String>,
    },

    /// Remove a dependency from project.blsp and re-lock.
    Remove {
        /// The require-name of the dependency to remove.
        name: String,
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

        /// Document every public global in a fresh image (the builtins +
        /// prelude) — the complete primitive reference. Read this instead of
        /// probing names one at a time. Ignores MODULE.
        #[arg(long = "all")]
        all: bool,
    },

    /// Serve the project over Model Context Protocol on stdio so an agent
    /// (Claude Code etc.) can eval / lookup / format / expand / run tests /
    /// read docs against this project's live image (ADR-036, docs/mcp.md).
    /// Errors if cwd is not inside a Brood project.
    Mcp,

    /// Open a live process observer — a full-screen TUI listing processes and
    /// their status / mailbox / memory (an Erlang-observer-style view, ADR-046).
    ///
    /// With no `--connect`: a standalone demo over a fresh runtime's own (seeded)
    /// processes. With `--connect name@host:port`: **remote attach** — observe a
    /// *running* program over the node link (it must have called `node-start` +
    /// `observe-serve`); the cookie comes from `--cookie` or `$BROOD_COOKIE`
    /// (ADR-053). Press `q` / Esc / Ctrl-C to quit.
    Observe {
        /// Attach to a running peer node `name@host:port` instead of the local
        /// demo (the target must have called `observe-serve`).
        #[arg(long = "connect", value_name = "NODE")]
        connect: Option<String>,

        /// Shared cookie authenticating the link (must match the target's). Falls
        /// back to `$BROOD_COOKIE`; required when `--connect` is given.
        #[arg(long = "cookie", value_name = "COOKIE")]
        cookie: Option<String>,
    },
}

fn main() {
    // Default to a backtrace on panic (see the matching note in
    // `crates/cli/src/main.rs`) — set before any thread spawns; RUST_BACKTRACE=0
    // still opts out.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
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
        Cmd::New { name, template } => cmd_new(&mut interp, &name, template.as_deref()),
        Cmd::Format { check } => cmd_format(&mut interp, check),
        Cmd::Run {
            file,
            watch,
            for_duration,
            main,
            args,
        } => cmd_run(
            &mut interp,
            file.as_deref(),
            &watch,
            for_duration.as_deref(),
            main.as_deref(),
            &args,
        ),
        Cmd::Doc { module, all } => cmd_doc(&mut interp, module.as_deref(), all),
        Cmd::Fetch => run(&mut interp, "(require 'package) (fetch)"),
        Cmd::Tree => run(&mut interp, "(require 'package) (tree)"),
        Cmd::Add { name, spec } => cmd_add(&mut interp, &name, &spec),
        Cmd::Remove { name } => {
            let escaped = brood::introspect::escape_brood_string(&name);
            run(
                &mut interp,
                &format!("(require 'package) (remove-dep \"{}\")", escaped),
            );
        }
        Cmd::Repl => cmd_repl(&mut interp),
        Cmd::Mcp => cmd_mcp(&mut interp),
        Cmd::Observe { connect, cookie } => cmd_observe(&mut interp, connect, cookie),
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

/// Like [`TermGuard`] but for the *inline* REPL editor (`term-raw-enter`): only
/// leaves raw mode, writing no escape sequences, so a piped (non-TTY) `nest repl`
/// stdout stays clean on exit. The Brood `term-raw-leave` is the normal teardown.
struct ReplTermGuard;
impl Drop for ReplTermGuard {
    fn drop(&mut self) {
        brood::builtins::restore_raw();
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
    // `:trace` prints each test's name as it starts (live progress) — wanted for the
    // interactive `nest test`; the `brood --test` path stays quiet for clean,
    // machine-parseable output.
    run(interp, "(run-tests :trace)");
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

/// `nest new <name> [--template NAME]` — delegates to `(new-project name
/// template)` in std/project.blsp.
fn cmd_new(interp: &mut Interp, name: &str, template: Option<&str>) {
    let escaped = brood::introspect::escape_brood_string(name);
    let tmpl_arg = match template {
        Some(t) => format!(" \"{}\"", brood::introspect::escape_brood_string(t)),
        None => String::new(),
    };
    let code = format!(
        "(require 'project) (load-config) (new-project \"{}\"{})",
        escaped, tmpl_arg
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
/// Parse a duration like `2s`, `500ms`, or a bare `1500` (milliseconds) into
/// milliseconds. `None` if unparseable or negative (the caller turns that into
/// an exit-2 with a usage hint).
fn parse_duration_ms(s: &str) -> Option<u64> {
    let t = s.trim();
    let ms = if let Some(n) = t.strip_suffix("ms") {
        n.trim().parse::<f64>().ok()?
    } else if let Some(n) = t.strip_suffix('s') {
        n.trim().parse::<f64>().ok()? * 1000.0
    } else {
        t.parse::<f64>().ok()? // bare number = milliseconds
    };
    (ms.is_finite() && ms >= 0.0).then_some(ms as u64)
}

fn cmd_run(
    interp: &mut Interp,
    file: Option<&str>,
    watch: &[String],
    for_duration: Option<&str>,
    main: Option<&str>,
    args: &[String],
) {
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
    let timed: Option<(u64, String)> = for_duration.map(|s| match parse_duration_ms(s) {
        Some(ms) => (ms, s.trim().to_string()),
        None => {
            eprintln!("nest run: invalid --for duration '{s}' (use e.g. 2s, 500ms, or 1500)");
            std::process::exit(2);
        }
    });
    let wrap = !watch.is_empty() || timed.is_some();
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
    // `--main module/fn` overrides the manifest's `:main` for this run only.
    // It applies to the project-entry path (no FILE); with a FILE we run that
    // file directly, so the override is meaningless — warn rather than ignore
    // silently (the silent-wrong-result lesson from the Game-of-Life retro).
    let main_override = match (main, file.is_none()) {
        (Some(spec), true) => format!(
            "(set-project-main \"{}\") ",
            brood::introspect::escape_brood_string(spec)
        ),
        (Some(_), false) => {
            eprintln!("nest run: --main is ignored when a FILE is given");
            String::new()
        }
        (None, _) => String::new(),
    };
    let project_setup = if file.is_none() {
        format!("(require 'project) (load-config) {}", main_override)
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
        //
        // With `--for DURATION`, add a `(after ms …)` timeout clause: when the
        // cap elapses the receive returns, the root falls through, and the
        // binary exits cleanly (the spawned program is dropped on exit). This
        // is the first-class form of `timeout Ns nest run` — it lets a loop /
        // TUI app be exercised end-to-end (not just its pure fns) and makes
        // time-based behaviour reproducible in CI.
        let after_clause = match &timed {
            Some((ms, label)) => format!(
                "(after {} (println \"[stopped after {}]\"))",
                ms,
                brood::introspect::escape_brood_string(label)
            ),
            None => String::new(),
        };
        format!(
            "(let (p (%spawn (fn () {}))) \
                  (monitor p) \
                  (receive ([:down _ ~p reason] (println \"[exit]\" reason)) {}))",
            run_form, after_clause
        )
    } else {
        run_form
    };
    let code = format!("{}{} {}", project_setup, watch_setup, body);
    run(interp, &code);
}

/// `nest doc [module] [--all]` — Markdown docs to stdout. `--all` documents
/// every public global in a fresh image (the complete builtin + prelude
/// reference) and ignores MODULE.
/// `nest add NAME :path PATH` — dispatch into the package module's `add` verb,
/// passing NAME and each spec token as escaped string arguments.
fn cmd_add(interp: &mut Interp, name: &str, spec: &[String]) {
    use brood::introspect::escape_brood_string;
    let mut call = format!("(require 'package) (add \"{}\"", escape_brood_string(name));
    for tok in spec {
        call.push_str(&format!(" \"{}\"", escape_brood_string(tok)));
    }
    call.push(')');
    run(interp, &call);
}

fn cmd_doc(interp: &mut Interp, module: Option<&str>, all: bool) {
    let code = if all {
        "(require 'docs) (println (document-all))".to_string()
    } else {
        match module {
            Some(name) => {
                let escaped = brood::introspect::escape_brood_string(name);
                format!("(require 'docs) (generate-docs \"{}\")", escaped)
            }
            None => "(require 'docs) (generate-docs)".to_string(),
        }
    };
    run(interp, &code);
}

/// `nest repl` — project-aware REPL. Inside a project, pre-load every source
/// file so the project's modules are immediately callable from the prompt.
/// Outside a project, fall through to the plain language REPL (same UX as
/// `brood`). The REPL itself is Brood (`std/repl.blsp`, ADR-048) — one
/// implementation both binaries bootstrap into via `(repl-run)`.
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
    // The REPL is Brood now (`std/repl.blsp`), same as `brood` with no args. The
    // interactive editor enters raw mode (std/lineedit.blsp), so guard the
    // terminal: the Brood `term-raw-leave` is the normal teardown, but this
    // restores it on a panic unwind too. Scope it like `cmd_observe` so it drops
    // (restoring) before any error report + exit (`process::exit` skips Drop).
    let result = {
        let _guard = ReplTermGuard;
        interp.eval_str("(require 'repl) (repl-run)")
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
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
fn cmd_observe(interp: &mut Interp, connect: Option<String>, cookie: Option<String>) {
    // Pick the bootstrap: a remote attach (`--connect`) or the standalone demo.
    // For remote, resolve the cookie (--cookie → $BROOD_COOKIE → error) and connect
    // — `observe-connect` dials the peer *before* taking the terminal, so a bad
    // host / wrong cookie surfaces as a clean error with the screen never entered.
    let boot = match connect {
        Some(spec) => {
            let cookie = cookie
                .or_else(|| std::env::var("BROOD_COOKIE").ok())
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| {
                    eprintln!(
                        "nest observe --connect: provide --cookie <c> or set $BROOD_COOKIE"
                    );
                    std::process::exit(2);
                });
            // `spec`/`cookie` are user input embedded in a Brood string literal —
            // escape backslash and quote so they can't break out of the literal.
            format!(
                "(require 'observer) (observe-connect \"{}\" \"{}\")",
                brood_str_escape(&spec),
                brood_str_escape(&cookie),
            )
        }
        None => "(require 'observer) (observe-run)".to_string(),
    };
    // The guard restores the terminal on a panic unwind; the inner scope drops it
    // (restoring) before any error is reported and we exit — `process::exit`
    // skips Drop. On the normal `q` path the Brood `term-leave` already restored;
    // the guard's second restore is idempotent.
    let result = {
        let _guard = TermGuard;
        interp.eval_str(&boot)
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Escape a host string for safe embedding in a Brood double-quoted string literal
/// (backslash and double-quote only — Brood string syntax is C-like).
fn brood_str_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
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

#[cfg(test)]
mod tests {
    use super::parse_duration_ms;

    #[test]
    fn parse_duration_ms_handles_units_and_bare_millis() {
        assert_eq!(parse_duration_ms("1500"), Some(1500)); // bare = ms
        assert_eq!(parse_duration_ms("500ms"), Some(500));
        assert_eq!(parse_duration_ms("2s"), Some(2000));
        assert_eq!(parse_duration_ms("1.5s"), Some(1500)); // fractional seconds
        assert_eq!(parse_duration_ms("  250ms  "), Some(250)); // trimmed
        assert_eq!(parse_duration_ms("0"), Some(0));
    }

    #[test]
    fn parse_duration_ms_rejects_garbage_and_negatives() {
        assert_eq!(parse_duration_ms("2x"), None);
        assert_eq!(parse_duration_ms("abc"), None);
        assert_eq!(parse_duration_ms(""), None);
        assert_eq!(parse_duration_ms("-5s"), None);
    }
}
