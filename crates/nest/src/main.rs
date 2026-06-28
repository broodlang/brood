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
//!   nest fetch             resolve dependencies, write project.lock.blsp (ADR-037)
//!   nest update [<name>…]  re-resolve dependency refs and re-lock (advance moving refs)
//!   nest tree              print the resolved dependency tree
//!   nest add <name> …      add a dependency (`:path PATH` or `:git URL :ref REF`) and re-lock
//!   nest remove <name>     remove a dependency and re-lock
//!   nest repl              project-aware REPL (sources preloaded)
//!   nest format            in-place reformat (`--check` for CI dry-run)
//!   nest doc [module]      Markdown docs (whole project or one module);
//!                          `--all` is the complete builtin + prelude reference
//!   nest mcp               Model Context Protocol server over stdio
//!
//! `-j N` / `--max-parallel N` caps concurrent spawned processes. Hot reload
//! lives in `nest run --watch <path>` (file or directory, repeatable).

use brood::cli_support::{report_error, run_on_main_stack, FullTermGuard, RawTermGuard};
use brood::Interp;
use clap::{Parser, Subcommand, ValueEnum};

mod mcp;
mod release;

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

/// Which editor grammar `nest grammar` emits (ADR-092). A `ValueEnum` so clap
/// lists the choices in `--help`, rejects an unknown one with a formatted error,
/// and offers shell completion — instead of a hand-rolled match + `exit(2)`.
#[derive(ValueEnum, Clone, Copy, Debug)]
enum GrammarTarget {
    /// A VS Code TextMate grammar (JSON).
    #[value(alias = "vscode", alias = "textmate")]
    Tmlanguage,
    /// The `brood-special-forms` defconst for Emacs.
    Emacs,
    /// The `tree-sitter-brood` `queries/highlights.scm`.
    #[value(alias = "treesitter", alias = "highlights")]
    TreeSitter,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Scaffold a new project (project.blsp + src/ + tests/ + starter files).
    New {
        /// The project's name. Becomes the directory + `:name` in project.blsp.
        name: String,

        /// Starter template: `default` (a main+hello pair), `tui-loop` (a
        /// tail-recursive animation loop, pairs with `nest run --for`), `gen`
        /// (a stateful gen_server-style process), or `http-server` (a basic web
        /// app over std/net/http). An unknown name lists the full set.
        #[arg(long = "template", short = 't', value_name = "NAME")]
        template: Option<String>,
    },

    /// Run the project's entry point, or a specific .blsp file.
    ///
    /// Inside a project: with no FILE, runs `:main` (defaults to `main/main`);
    /// with a `.blsp` FILE, runs that file with the project's sources pre-loaded
    /// so it can reach project modules; with a *non-*`.blsp` FILE, runs `:main`
    /// passing FILE as its argument — so `nest run notes.txt` opens notes.txt in
    /// the editor (vim/emacs style) rather than parsing it as Brood.
    /// Outside a project: FILE is required and runs like `brood <file>`.
    Run {
        /// A `.blsp` file to run instead of `:main`, or a document to hand `:main`.
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

        /// Start this runtime as a node named NAME before running — a local
        /// Unix-socket node (no port), the Emacs `--daemon` model. Peers reach
        /// it with `(connect "NAME")`; the shared `~/.config/brood/cookie`
        /// authenticates. The program need not call `node-start` itself.
        #[arg(long = "name", value_name = "NAME")]
        name: Option<String>,

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

    /// Re-resolve dependency refs and re-lock, advancing moving refs (ADR-037).
    ///
    /// With no NAMES: re-resolves every dependency (ignoring the locked commits,
    /// so a branch or floating tag moves forward). With NAMES: only those deps
    /// re-resolve; the rest keep their locked pins.
    Update {
        /// The require-names of the dependencies to update. Omit to update all.
        #[arg(value_name = "NAME")]
        names: Vec<String>,
    },

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

    /// Generate an editor syntax grammar from the language's own `(special-forms)`
    /// — one source of truth, no hand-maintained keyword lists (ADR-092). Prints to
    /// stdout; redirect to the editor's grammar file.
    ///
    /// TARGET is `tmlanguage` (default — a VS Code TextMate grammar, JSON), `emacs`
    /// (the `brood-special-forms` defconst), or `tree-sitter` (the `tree-sitter-brood`
    /// `queries/highlights.scm`). E.g.
    /// `nest grammar > brood-vscode/syntaxes/brood.tmLanguage.json`.
    Grammar {
        /// What to emit (default `tmlanguage`).
        #[arg(value_enum, default_value_t = GrammarTarget::Tmlanguage)]
        target: GrammarTarget,
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

    /// Attach this terminal to a `ui-run` app served by a running daemon — the
    /// `emacsclient` to its `--daemon` (ADR-090). The daemon's app renders here and
    /// this terminal's keys drive it; the app's model lives on the daemon, so several
    /// terminals can attach at once.
    ///
    /// SPEC is the served node: a bare `name` over the local Unix socket (e.g. a
    /// `nest run --name ed app.blsp` that called `(serve …)`), or `name@host:port`
    /// over TCP. The cookie comes from `--cookie` or `$BROOD_COOKIE`, else the shared
    /// `~/.config/brood/cookie`. Press the app's own quit key to detach.
    Attach {
        /// The served node to attach to: `name` (local Unix socket) or `name@host:port`.
        #[arg(value_name = "SPEC")]
        spec: String,

        /// Shared cookie authenticating the link (must match the daemon's). Falls
        /// back to `$BROOD_COOKIE`, then the shared cookie file.
        #[arg(long = "cookie", value_name = "COOKIE")]
        cookie: Option<String>,
    },

    /// Bundle the project into a single self-contained executable (ADR-038).
    ///
    /// Appends the project's manifest + every `src/**/*.blsp` (and resolved
    /// dependency sources) to a copy of the prebuilt `brood` runtime. The result
    /// runs `:main` on any compatible machine with no interpreter, project dir,
    /// or source files alongside — just the one binary. `tests/` is excluded.
    Release {
        /// Output path for the binary. Defaults to the project's `:name`; with
        /// `--target` the name gets a per-target suffix (e.g. `app-macos-arm64`).
        #[arg(long = "output", short = 'o', value_name = "PATH")]
        output: Option<String>,

        /// The base `brood` runtime to append to. Defaults to the `brood`
        /// embedded in this `nest`. Only valid alongside at most one `--target`.
        #[arg(long = "runtime", value_name = "PATH")]
        runtime: Option<String>,

        /// Target triple(s) to release for — repeatable. Each resolves a
        /// prebuilt lean runtime from the local cache
        /// (`~/.cache/brood/runtimes/<triple>/brood`); the host's own triple
        /// falls back to the embedded runtime. Cross-compiling is out of scope
        /// (ADR-038) — build the runtime on/for the target and drop it in the
        /// cache (or pass `--runtime`).
        #[arg(long = "target", value_name = "TRIPLE")]
        targets: Vec<String>,
    },
}

fn main() {
    // Default to a backtrace on panic (see the matching note in
    // `crates/cli/src/main.rs`) — set before any thread spawns; RUST_BACKTRACE=0
    // still opts out.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }
    // Capture any panic (use-after-GC tripwire, heap index, …) to .brood_crash_dump.
    brood::cli_support::install_crash_dump();
    let cli = Cli::parse();
    // Run on an explicitly-sized large stack so the stack-budget guard (ADR-043)
    // is uniform across the root thread and spawned coroutines (see
    // `cli_support::run_on_main_stack`).
    run_on_main_stack("nest-main", move || run_main(cli));
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
    // Flag a stressed/retuned heap so a benchmark can't silently measure one.
    brood::cli_support::warn_nondefault_gc_env();

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
            name,
            args,
        } => cmd_run(
            &mut interp,
            file.as_deref(),
            &watch,
            for_duration.as_deref(),
            main.as_deref(),
            name.as_deref(),
            &args,
        ),
        Cmd::Doc { module, all } => cmd_doc(&mut interp, module.as_deref(), all),
        Cmd::Grammar { target } => cmd_grammar(&mut interp, target),
        Cmd::Fetch => run(&mut interp, "(require 'package) (package/fetch)"),
        Cmd::Update { names } => cmd_update(&mut interp, &names),
        Cmd::Tree => run(&mut interp, "(require 'package) (package/tree)"),
        Cmd::Add { name, spec } => cmd_add(&mut interp, &name, &spec),
        Cmd::Remove { name } => {
            let call = brood::introspect::call_form("package/remove-dep", &[&name]);
            run(&mut interp, &format!("(require 'package) {call}"));
        }
        Cmd::Repl => cmd_repl(&mut interp),
        Cmd::Mcp => cmd_mcp(&mut interp),
        Cmd::Observe { connect, cookie } => cmd_observe(&mut interp, connect, cookie),
        Cmd::Attach { spec, cookie } => cmd_attach(&mut interp, spec, cookie),
        Cmd::Release {
            output,
            runtime,
            targets,
        } => cmd_release(&mut interp, output.as_deref(), runtime.as_deref(), &targets),
    }
}

// Terminal-restore guards (`FullTermGuard` for the full-screen `nest observe` /
// `nest attach` path; `RawTermGuard` for the inline `nest repl` editor) live in
// `brood::cli_support`, shared with the `brood` binary — see there for the
// deliberate `restore_terminal` vs `restore_raw` divergence.

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
            "(require 'project) (project/load-config) (project/run-project-tests)",
        );
        return;
    }
    // Single-file path: mirror brood --test, but pre-load project image when
    // we're inside a project so cross-module names resolve.
    let bootstrap = if in_project() {
        "(require 'project) (project/load-config) (let (root (project/project--find-root (cwd))) \
            (when root (project/project-setup root) (project/project-load-sources root))) \
            (require 'test)"
    } else {
        "(require 'test)"
    };
    run(interp, bootstrap);
    for path in files {
        let src = brood::cli_support::read_source_or_exit("nest test", std::path::Path::new(path));
        if let Err(e) = brood::cli_support::eval_file(interp, path, &src) {
            report_error(&e.or_file(path.clone()));
            std::process::exit(1);
        }
    }
    // `:trace` prints each test's name as it starts (live progress) — wanted for the
    // interactive `nest test`; the `brood --test` path stays quiet for clean,
    // machine-parseable output.
    run(interp, "(test/run-tests :trace)");
}

/// `nest check [FILES...]` — project-wide if no files, otherwise file-by-file.
fn cmd_check(interp: &mut Interp, files: &[String]) {
    // One checker, one path. Whole-project and file-list checks both go through
    // `std/project.blsp`, which loads the project image *first* so cross-module /
    // namespace imports resolve through the heap's globals. The single-file path
    // used to be a separate Rust loop that skipped that setup — so every `:use`d
    // or qualified name in a namespaced file false-flagged as unbound (the
    // breakage the `.brood-skip-blsp-check` migration hatch was added for). Both
    // forms now return a warning count; non-zero → exit 1.
    let code = if files.is_empty() {
        "(require 'project) (project/load-config) (require 'test) (project/check-project)".to_string()
    } else {
        let list = files
            .iter()
            .map(|f| format!("\"{}\"", brood::introspect::escape_brood_string(f)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("(require 'project) (require 'test) (project/check-files (list {list}))")
    };
    match run_for_value(interp, &code) {
        brood::core::value::Value::Int(0) => {}
        brood::core::value::Value::Int(_) => std::process::exit(1),
        other => {
            eprintln!(
                "nest check: checker returned a non-integer ({})",
                interp.print(other)
            );
            std::process::exit(1);
        }
    }
}

/// `nest new <name> [--template NAME]` — delegates to `(project/new-project name
/// template)` in std/project.blsp.
fn cmd_new(interp: &mut Interp, name: &str, template: Option<&str>) {
    let mut args: Vec<&str> = vec![name];
    args.extend(template);
    let call = brood::introspect::call_form("project/new-project", &args);
    run(
        interp,
        &format!("(require 'project) (project/load-config) {call}"),
    );
}

/// `nest format [--check]` — reformat in place, or dry-run on `--check`.
fn cmd_format(interp: &mut Interp, check: bool) {
    let entry = if check {
        "(format/format-project-check)"
    } else {
        "(format/format-project)"
    };
    let code = format!(
        "(require 'project) (project/load-config) (require 'format) {}",
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
    name: Option<&str>,
    args: &[String],
) {
    // A non-`.blsp` positional FILE inside a project is a *document* for the entry
    // point (the editor opens it), not a Brood script to run: route it to `:main` as
    // an argument, so `nest run notes.txt` edits notes.txt (vim/emacs style) instead
    // of trying to parse it as Brood. A `.blsp` FILE still runs as a script; outside a
    // project FILE always runs (there's no `:main` to hand it to).
    let doc_arg: Option<String> = match file {
        Some(p) if in_project() && !p.ends_with(".blsp") => Some(p.to_string()),
        _ => None,
    };
    let file: Option<&str> = if doc_arg.is_some() { None } else { file };

    let promoted: Option<String> = if file.is_none() && doc_arg.is_none() && watch.len() == 1 {
        let p = &watch[0];
        match std::fs::metadata(p) {
            Ok(meta) if !meta.is_dir() => Some(p.clone()),
            _ => None,
        }
    } else {
        None
    };
    // With no explicit FILE but `--watch` paths that *can't* promote to the entry
    // we run `:main` and watch alongside. That's the intended, unremarkable case for
    // watching a directory (`nest run --watch src` — the standard hot-reload dev
    // loop), so stay silent there. Only speak up for the genuinely surprising case:
    // the user watched *files* (one of which they may have expected to *run*), but
    // gave more than one, so none was promoted — say so once.
    let watched_a_file = watch.iter().any(|p| std::path::Path::new(p).is_file());
    if file.is_none() && doc_arg.is_none() && promoted.is_none() && watched_a_file {
        eprintln!(
            "nest run: watching {} files and running :main — none was run directly. \
             (A single watched *file* is promoted to the entry to run; multiple files can't \
             be, so :main runs.)",
            watch.len()
        );
    }
    let file: Option<&str> = file.or(promoted.as_deref());

    // The document arg (if any) leads the trailing args passed to `:main`.
    let escaped_args = doc_arg
        .into_iter()
        .chain(args.iter().cloned())
        .map(|a| format!("\"{}\"", brood::introspect::escape_brood_string(&a)))
        .collect::<Vec<_>>()
        .join(" ");

    let watch_setup = if watch.is_empty() {
        String::new()
    } else {
        let calls = watch
            .iter()
            .map(|p| brood::introspect::call_form("reload/reload-on-change", &[p]))
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
        None => format!("(project/run-project (list {}))", escaped_args),
        // FILE: run that file. Inside a project, set up the project so its
        // `src/` is on `*load-path*` (the file can `(require 'foo)` other
        // project modules), but *don't* eager-load every source — otherwise a
        // file under `src/` would run twice (once via the walker, once via the
        // explicit `load`). Outside a project, plain `brood <file>`.
        Some(path) => brood::introspect::call_form("load", &[path]),
    };
    // `--main module/fn` overrides the manifest's `:main` for this run only.
    // It applies to the project-entry path (no FILE); with a FILE we run that
    // file directly, so the override is meaningless — warn rather than ignore
    // silently (the silent-wrong-result lesson from the Game-of-Life retro).
    let main_override = match (main, file.is_none()) {
        (Some(spec), true) => format!(
            "{} ",
            brood::introspect::call_form("project/set-project-main", &[spec])
        ),
        (Some(_), false) => {
            eprintln!("nest run: --main is ignored when a FILE is given");
            String::new()
        }
        (None, _) => String::new(),
    };
    let project_setup = if file.is_none() {
        format!("(require 'project) (project/load-config) {}", main_override)
    } else if in_project() {
        "(require 'project) (project/load-config) \
         (let (root (project/project--find-root (cwd))) \
           (when root (project/project-setup root))) "
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
    // `--name`: bring up a local Unix-socket node before the program runs, so
    // the file is pure app logic (the Emacs `--daemon` model). Pass the name as
    // a keyword built from the escaped string so an odd NAME can't break out.
    let node_setup = match name {
        Some(n) => format!(
            "(node-start (keyword \"{}\")) ",
            brood::introspect::escape_brood_string(n)
        ),
        None => String::new(),
    };
    // Advisory pre-flight for an explicit FILE run, so *every* `nest run` path
    // checks first: `nest run` (:main) already checks via `check-project-sources`
    // (in `run-project`), and `brood <file>` pre-checks too — this closes the gap
    // for `nest run FILE.blsp`, which loads the file directly. `check-file` returns
    // GNU `path:line:col: warning:` strings; print to stderr and run regardless
    // (advisory, never gates). `BROOD_NO_CHECK=1` opts out — the flag the rest of
    // the toolchain honors. Runs after `project_setup` (so the file's load-path is
    // set) and before the body. Like `brood <file>`, this is a *single-file* check:
    // a qualified reference to an unloaded sibling module may warn — use `nest check`
    // (whole-project) or `BROOD_NO_CHECK=1` for that case.
    let check_setup = match file {
        Some(path) => format!(
            "(unless (= (getenv \"BROOD_NO_CHECK\") \"1\") \
               (doseq (w (check-file \"{}\")) (eprintln w))) ",
            brood::introspect::escape_brood_string(path)
        ),
        None => String::new(),
    };
    let code = format!(
        "{}{}{}{} {}",
        project_setup, check_setup, node_setup, watch_setup, body
    );
    run(interp, &code);
}

/// `nest update [NAME...]` — re-resolve refs and re-lock (ADR-037). No NAMES
/// updates every dep; NAMES updates only those.
fn cmd_update(interp: &mut Interp, names: &[String]) {
    let args: Vec<&str> = names.iter().map(String::as_str).collect();
    let call = format!(
        "(require 'package) {}",
        brood::introspect::call_form("package/update", &args)
    );
    run(interp, &call);
}

/// `nest add NAME :path PATH` — dispatch into the package module's `add` verb,
/// passing NAME and each spec token as escaped string arguments.
fn cmd_add(interp: &mut Interp, name: &str, spec: &[String]) {
    let mut args: Vec<&str> = vec![name];
    args.extend(spec.iter().map(String::as_str));
    let call = format!(
        "(require 'package) {}",
        brood::introspect::call_form("package/add", &args)
    );
    run(interp, &call);
}

/// `nest doc [module] [--all]` — Markdown docs to stdout. `--all` documents
/// every public global in a fresh image (the complete builtin + prelude
/// reference) and ignores MODULE.
fn cmd_doc(interp: &mut Interp, module: Option<&str>, all: bool) {
    let code = if all {
        "(require 'docs) (println (docs/document-all))".to_string()
    } else {
        match module {
            Some(name) => format!(
                "(require 'docs) {}",
                brood::introspect::call_form("docs/generate-docs", &[name])
            ),
            None => "(require 'docs) (docs/generate-docs)".to_string(),
        }
    };
    run(interp, &code);
}

/// `nest grammar [TARGET]` — emit an editor syntax grammar generated from the
/// language's own `(special-forms)` (ADR-092), to stdout. `tmlanguage` (default) is
/// a VS Code TextMate grammar (JSON); `emacs` is the `brood-special-forms` defconst.
/// Pure Brood — `std/tool/grammar.blsp` — so adding a special form updates every
/// editor's highlighting from one place.
fn cmd_grammar(interp: &mut Interp, target: GrammarTarget) {
    // Exhaustive — clap already rejected any unknown value (with a listed-choices
    // error) before we get here, so there's no fallback/exit(2) arm.
    let call = match target {
        GrammarTarget::Tmlanguage => "(grammar/tmlanguage)",
        GrammarTarget::Emacs => "(grammar/emacs-special-forms)",
        GrammarTarget::TreeSitter => "(grammar/tree-sitter-highlights)",
    };
    run(interp, &format!("(require 'grammar) (println {call})"));
}

/// `nest repl` — project-aware REPL. Inside a project, pre-load every source
/// file so the project's modules are immediately callable from the prompt.
/// Outside a project, fall through to the plain language REPL (same UX as
/// `brood`). The REPL itself is Brood (`std/repl.blsp`, ADR-048) — one
/// implementation both binaries bootstrap into via `(repl/repl-run)`.
fn cmd_repl(interp: &mut Interp) {
    if in_project() {
        run(
            interp,
            "(require 'project) (project/load-config) \
             (let (root (project/project--find-root (cwd))) \
               (when root (project/project-setup root) (project/project-load-sources root)))",
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
        let _guard = RawTermGuard;
        interp.eval_str("(require 'repl) (repl/repl-run)")
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// `nest mcp` — see docs/mcp.md (ADR-036). Strictly per-project.
fn cmd_mcp(interp: &mut Interp) {
    // `setup-tooling-image` (std/project.blsp) is the shared tooling bootstrap
    // the LSP also uses (via `introspect::load_tooling_image`) — sources + the
    // test/format frameworks — so the two servers can't drift on its contents.
    let bootstrap = r#"
        (require 'project)
        (project/load-config)
        (let (root (project/project--find-root (cwd)))
          (when (nil? root)
            (error "nest mcp: not in a Brood project (no project.blsp found from " (cwd) ")"))
          (project/setup-tooling-image root))
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
            // Cookie precedence: --cookie → $BROOD_COOKIE → (node-cookie). The
            // first two are resolved here; when neither is set we omit the arg
            // and `observe-connect` falls back to the shared cookie file itself
            // (ADR-068), so a matching local setup needs no flag.
            let cookie = cookie
                .or_else(|| std::env::var("BROOD_COOKIE").ok())
                .filter(|c| !c.is_empty());
            // `spec`/`cookie` are user input — `call_form` embeds them as escaped
            // string literals so they can't break out of the call.
            let args: Vec<&str> = match &cookie {
                Some(c) => vec![&spec, c],
                None => vec![&spec],
            };
            format!(
                "(require 'observer) {}",
                brood::introspect::call_form("observer/observe-connect", &args)
            )
        }
        None => "(require 'observer) (observer/observe-run)".to_string(),
    };
    // The guard restores the terminal on a panic unwind; the inner scope drops it
    // (restoring) before any error is reported and we exit — `process::exit`
    // skips Drop. On the normal `q` path the Brood `term-leave` already restored;
    // the guard's second restore is idempotent.
    let result = {
        let _guard = FullTermGuard;
        interp.eval_str(&boot)
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// `nest attach SPEC` — the thin `emacsclient`-style frontend (ADR-090). Connects to
/// the daemon serving a `ui-run` app and runs `editor/serve/attach`, which paints the
/// pushed frames + ships back keys. Same shape as `cmd_observe`: resolve the cookie
/// (`--cookie` → `$BROOD_COOKIE` → the shared cookie file), connect *before* taking
/// the terminal (so a bad spec / wrong cookie is a clean error, screen untouched),
/// and run under a `FullTermGuard` that restores the terminal on a panic unwind.
fn cmd_attach(interp: &mut Interp, spec: String, cookie: Option<String>) {
    let cookie = cookie
        .or_else(|| std::env::var("BROOD_COOKIE").ok())
        .filter(|c| !c.is_empty());
    // `spec`/`cookie` are user input — `call_form` embeds them as escaped string
    // literals so they can't break out of the call.
    let args: Vec<&str> = match &cookie {
        Some(c) => vec![&spec, c],
        None => vec![&spec],
    };
    let boot = format!(
        "(require 'editor/serve) {}",
        brood::introspect::call_form("editor/serve/attach", &args)
    );
    let result = {
        let _guard = FullTermGuard;
        interp.eval_str(&boot)
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// `nest release [-o PATH] [--runtime PATH] [--target TRIPLE]…` — bundle the
/// project into one self-contained executable per target (ADR-038). Collection
/// is policy (Brood: `project/bundle-collect`); byte assembly + I/O is mechanism
/// (Rust: `brood::bundle`). See `crates/lisp/src/bundle.rs` for the wire format.
fn cmd_release(
    interp: &mut Interp,
    output: Option<&str>,
    runtime: Option<&str>,
    targets: &[String],
) {
    use brood::core::value::Value;

    // 1. Collect the manifest + module sources as a flat list of strings
    //    `(manifest stem0 src0 stem1 src1 …)`. Errors (e.g. not in a project) are
    //    reported + exit by `run_for_value`.
    let collected = run_for_value(
        interp,
        "(require 'project) (let (root (project/project--find-root (cwd))) \
         (project/bundle-collect root))",
    );
    let items = match interp.heap.seq_items(collected) {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    };
    // Extract to owned Strings *before* any further eval — the list isn't rooted,
    // so a later collection could reclaim it.
    let strings: Vec<String> = items
        .iter()
        .map(|v| match v {
            Value::Str(id) => Ok(interp.heap.string(*id).to_string()),
            other => Err(interp.print(*other)),
        })
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|bad| {
            eprintln!("nest release: bundle-collect returned a non-string ({bad})");
            std::process::exit(1);
        });
    let (manifest, rest) = match strings.split_first() {
        Some(pair) => pair,
        None => {
            eprintln!("nest release: nothing to bundle");
            std::process::exit(1);
        }
    };
    // `bundle-collect` returns the modules as a flat `stem0 src0 stem1 src1 …`
    // list, so `rest` must have an even length. An odd tail means a stem with no
    // source — a contract violation; fail loudly (like the non-string check
    // above) rather than silently bundling the last module with empty source.
    if rest.len() % 2 != 0 {
        eprintln!(
            "nest release: bundle-collect returned an odd number of module items ({}); \
             expected stem/source pairs",
            rest.len()
        );
        std::process::exit(1);
    }
    let modules: Vec<(String, String)> = rest
        .chunks(2)
        .map(|c| (c[0].clone(), c[1].clone()))
        .collect();

    // 2. Default the output name from the manifest's `:name` (set in the interp
    //    by `bundle-collect`'s `project--apply`).
    let name = match run_for_value(interp, "(if *project-name* (name *project-name*) \"app\")") {
        Value::Str(id) => interp.heap.string(id).to_string(),
        _ => "app".to_string(),
    };

    // 3. Serialize the archive once — it's target-independent.
    let archive = brood::bundle::serialize(manifest, &modules);

    // 4. One release binary per target (no --target = one, for the host).
    //    --runtime names a single specific base, so it can't serve a matrix.
    if runtime.is_some() && targets.len() > 1 {
        eprintln!("nest release: --runtime names one base binary; use it with at most one --target");
        std::process::exit(2);
    }
    let stem = output.unwrap_or(&name);
    let plans: Vec<(Option<&str>, std::path::PathBuf)> = if targets.is_empty() {
        vec![(None, std::path::PathBuf::from(stem))]
    } else {
        targets
            .iter()
            .map(|t| {
                // `-o` with a single target is the exact output path; otherwise
                // each binary gets a per-target suffix (`app-macos-arm64`, …).
                let out = if output.is_some() && targets.len() == 1 {
                    stem.to_string()
                } else {
                    let exe = if release::is_windows_triple(t) { ".exe" } else { "" };
                    format!("{stem}-{}{exe}", release::target_suffix(t))
                };
                (Some(t.as_str()), std::path::PathBuf::from(out))
            })
            .collect()
    };
    for (triple, out) in plans {
        let base = release::resolve_runtime(runtime, triple);
        if let Err(e) = brood::bundle::write_release(&base, &archive, &out) {
            eprintln!("nest release: cannot write {}: {e}", out.display());
            std::process::exit(1);
        }
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        println!(
            "Wrote {} ({} module{}, {}{})",
            out.display(),
            modules.len(),
            if modules.len() == 1 { "" } else { "s" },
            release::human_size(size),
            triple.map(|t| format!(", {t}")).unwrap_or_default(),
        );
    }
}

// ---------- helpers ----------

/// Evaluate a bootstrap snippet, reporting any error in GNU form and exiting
/// non-zero on failure.
fn run(interp: &mut Interp, code: &str) {
    let result = interp.eval_str(code);
    // Restore the terminal on the way out — whether the program returned
    // cleanly or threw. A `nest run` of a TUI demo that entered raw mode / the
    // alternate screen and never reached its Brood `term-raw-leave` (because it
    // threw, *or* because it returned without one) would otherwise leave the
    // shell wedged. `process::exit` skips Drop, so a guard wouldn't fire —
    // restore explicitly. The call is a no-op unless the terminal was left raw.
    brood::builtins::restore_terminal_on_exit();
    if let Err(e) = result {
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
            brood::builtins::restore_terminal_on_exit();
            report_error(&e);
            std::process::exit(1);
        }
    }
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

    // The release-mechanism tests (target_suffix / is_windows_triple /
    // runtime_cache_path) moved alongside their helpers into `release.rs`.
}
