//! The `nest` command — Brood project tooling.
//!
//! `nest` is the project/workspace tool sitting above the `brood` language
//! binary — the `cargo`/`rustc`, `mix`/`elixir` split (ADR-028). For everyday
//! work this is the daily driver: `nest` covers scaffolding, running, testing,
//! type-checking, formatting, REPL, docs, and the MCP server. `brood` is the
//! low-level "just run the language" tool.
//!
//! `nest` is a thin Rust shell. The actual policy — name checks, templates,
//! discovery — is written in Brood (`std/tool/project.blsp`) and driven through
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
//!   nest format            in-place reformat (`--check` for CI dry-run,
//!                          `--changed` for only git-changed files)
//!   nest doc [module]      Markdown docs (whole project or one module);
//!                          `--all` is the complete builtin + prelude reference
//!   nest mcp               Model Context Protocol server over stdio
//!
//! `-j N` / `--max-parallel N` caps concurrent spawned processes. Hot reload
//! lives in `nest run --watch <path>` (file or directory, repeatable).

use brood::cli_support::{report_error, run_on_main_stack, FullTermGuard, RawTermGuard};
use brood::Interp;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

mod mcp;
mod release;

#[derive(Parser, Debug)]
#[command(
    name = "nest",
    // The build sha, not just the semver — see `cli_support::VERSION_LINE`.
    version = brood::cli_support::VERSION_LINE,
    about = "Brood project tooling — the daily driver above the `brood` language binary (ADR-028).",
    after_help = "Also (implemented in Brood, std/tool/nest.blsp): new, run, test, check, format, doc, docs, doctest, grammar, rename, update-tooling, fetch, update, tree, add, remove, publish, search, key, ws — `nest <command> --help`.",
    propagate_version = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Cap concurrent spawned processes (0 = unlimited). Bounds a concurrent
    /// test run; see `std/tool/test.blsp`.
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

/// Which shell `nest completions` emits an integration script for. A `ValueEnum`,
/// so the choices are listed in `--help`, an unknown one is rejected with a
/// formatted error, and `nest completions <TAB>` completes them from this
/// definition rather than a restated list.
#[derive(ValueEnum, Clone, Copy, Debug)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print a shell integration script enabling TAB completion for `nest`.
    ///
    /// Completes subcommands, flags, and project-aware values — test files, tags
    /// for `--only`/`--exclude`/`--include`, dependency names, module names.
    ///
    /// Install by sourcing it from your shell's startup file:
    ///   bash:  eval "$(nest completions bash)"
    ///   zsh:   eval "$(nest completions zsh)"
    ///   fish:  nest completions fish | source
    Completions {
        /// Which shell to emit for.
        #[arg(value_name = "SHELL")]
        shell: CompletionShell,
    },

    /// Print completion candidates for a partial command line (used by the shell
    /// scripts from `nest completions`; not usually run by hand).
    ///
    /// Takes the words after `nest`, the word being typed last, and prints one
    /// candidate per line. Always exits 0 — a completion must never fail.
    #[command(hide = true)]
    Complete {
        /// The words after `nest`, with the partial word last.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        words: Vec<String>,
    },

    /// Build this binary's standard-library startup image (ADR-218), once.
    ///
    /// Keyed on `system/stdlib-id` — a content hash of every baked-in `.blsp` — so `brood`,
    /// `nest` and `brood-lsp` from one tree all read the SAME file. Every ordinary `nest`
    /// command writes the image when one is missing (`ensure_stdimage`); this is the explicit
    /// form, for a machine where the first run should not be the one that pays.
    ///
    /// Deliberately NOT one of the Brood-routed subcommands (ADR-322, KI-112): the build
    /// attributes a module's ROOT globals by loading it and diffing, so it is sound only in
    /// a process where nothing but the prelude is loaded — and `std/tool/nest.blsp`, the
    /// dispatcher, is a std module whose own load pulls the toolchain in. Routed, it wrote
    /// an image with none of `project`'s root globals. This arm builds before any std module
    /// loads, and the child process `ensure_stdimage` spawns is exactly this arm.
    Stdimage,

    /// Start a REPL. Inside a project, every source file is pre-loaded so the
    /// project's modules are immediately callable.
    Repl,

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
    ///
    /// Every binary written is then RUN with the reserved `--brood-boot-check`
    /// argument to prove it starts; one that does not is deleted and the release
    /// fails. `--no-smoke` skips that.
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

        /// Skip the boot check. By default every binary written is run with the
        /// reserved `--brood-boot-check` argument to prove it boots — load every
        /// embedded module, resolve `:main`, run nothing — and a binary that does
        /// not boot fails the release and is deleted. Skipped for a non-host
        /// `--target` (that binary cannot run here); the skip is reported, never
        /// silent. Use this only when you cannot run the artifact at all.
        #[arg(long = "no-smoke")]
        no_smoke: bool,
    },
}

/// Write this binary's stdlib image if there is no current one, so the NEXT process boots with
/// it. Best-effort and silent on success: a `:present` answer is the common case and costs one
/// index read, and a failure to build is not a reason to fail the user's command — `require`
/// simply keeps reading source, exactly as it did before images existed.
fn ensure_stdimage(interp: &mut Interp, cmd: &Cmd) {
    // `Stdimage` builds explicitly and reports; the rest are the commands that must stay
    // instant, where a first-run build would BE the command's whole runtime.
    if matches!(
        cmd,
        Cmd::Stdimage | Cmd::Completions { .. } | Cmd::Complete { .. }
    ) {
        return;
    }
    ensure_stdimage_now(interp);
}

/// The unconditional half of [`ensure_stdimage`], for the Brood-routed subcommands (which
/// have no `Cmd`).
fn ensure_stdimage_now(interp: &mut Interp) {
    // Is there already a current image? A pure-PRELUDE probe on purpose — `%std-image-path`
    // and `%image-index` load no modules at all, where asking `stdimage` would pull that
    // module and its dependency tree into the process before the command has started.
    let current = interp.eval_str(
        "(let (p (%std-image-path)) (if p (not (nil? (%image-index p (system/stdlib-id)))) true))",
    );
    if let Ok(brood::core::value::Value::Bool(true)) = current {
        return;
    }

    // Build it in a CHILD process, not here.
    //
    // `stdimage/build` works by loading every module and snapshotting what each one binds, so
    // building in-process leaves the caller holding all ~107 std modules before its own work
    // begins. For `nest test` that silently rewrites the world the suite then measures: any
    // test whose premise is "this module is not loaded yet" is false on the first run after a
    // `std/` edit and true on every run after, which is a test that fails for a reason with no
    // relation to the change under test. `tests/stdimage_test.blsp`'s attribution case failed
    // exactly that way, 6 runs out of 6, and read as a concurrency bug for a day.
    //
    // A child pays the same ~1 s and hands back a file. Best-effort throughout: a failure to
    // build is not a reason to fail the user's command — `require` keeps reading source,
    // exactly as it did before images existed.
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let ok = std::process::Command::new(exe)
        .arg("stdimage")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|st| st.success())
        .unwrap_or(false);
    // Announce the rebuild, and only a rebuild. The image is written for the NEXT process, so
    // the command that triggers the write is itself running from source — and on this suite
    // that is the difference between ~91 s and ~110 s, and it is what makes the source path
    // (a documented KI-89 amplifier) the one this run took. Nothing said so, which is how a
    // slow run gets attributed to the change you just made instead of to a one-off cache miss.
    if ok {
        eprintln!(
            "nest: rebuilt the stdlib image (std/ or the commit changed) — THIS command \
             loaded std/ from source; later ones will not."
        );
    }
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
    // A subcommand implemented in Brood (`std/tool/nest.blsp`, ADR-322) is routed there
    // BEFORE clap sees argv: clap would reject its flags, which it no longer knows. The list
    // is the routing table AND the completion table's Rust half, so it cannot go stale
    // against the `Cmd` enum — a name is in exactly one of the two.
    let argv: Vec<String> = std::env::args().collect();
    if let Some((max_parallel, rest)) = blsp_routed(&argv[1..]) {
        run_on_main_stack("nest-main", move || run_blsp(max_parallel, rest));
        return;
    }
    let cli = Cli::parse();
    // Run on an explicitly-sized large stack so the stack-budget guard (ADR-043)
    // is uniform across the root thread and spawned coroutines (see
    // `cli_support::run_on_main_stack`).
    run_on_main_stack("nest-main", move || run_main(cli));
}

/// Subcommands implemented in `std/tool/nest.blsp` (ADR-322). Routed there from `main`
/// before clap runs; listed by `nest complete` beside clap's own; absent from `Cmd`.
const BLSP_SUBCOMMANDS: &[&str] = &[
    "doc",
    "docs",
    "doctest",
    "grammar",
    "format",
    "check",
    "test",
    "run",
    "new",
    "update-tooling",
    "rename",
    "fetch",
    "update",
    "tree",
    "add",
    "remove",
    "publish",
    "search",
    "key",
    "ws",
];

/// Is this argv (after the binary name) a Brood-implemented subcommand? Returns the value
/// of the one GLOBAL option clap owns — `-j`/`--max-parallel`/`--jobs N`, accepted before
/// or after the subcommand — and the remaining words from the subcommand on, with that
/// option removed. It is honoured here rather than in Brood because it sizes the
/// scheduler pool, which is built once, before any Brood runs.
fn blsp_routed(args: &[String]) -> Option<(Option<usize>, Vec<String>)> {
    let mut max_parallel = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(word) = args.get(i) {
        let value = match word.as_str() {
            "-j" | "--max-parallel" | "--jobs" => {
                i += 1;
                args.get(i)?.clone()
            }
            w if w.starts_with("--max-parallel=") || w.starts_with("--jobs=") => {
                w.split_once('=')?.1.to_string()
            }
            w if w.starts_with("-j") && w.len() > 2 && !w.starts_with("-jo") => w[2..].to_string(),
            c if rest.is_empty() && !BLSP_SUBCOMMANDS.contains(&c) => return None,
            _ => {
                rest.push(word.clone());
                i += 1;
                continue;
            }
        };
        max_parallel = Some(value.parse().ok()?);
        i += 1;
    }
    (!rest.is_empty()).then_some((max_parallel, rest))
}

/// The process-wide knobs `nest test` needs set BEFORE the interpreter exists — the Rust
/// half of the moved `test` arm, keyed on the subcommand because the timing is
/// load-bearing and fails silently when wrong:
///
///   * `BROOD_COVERAGE` decides whether the compiler emits `RecordLine`. Chunks are
///     compiled while `Interp::new()` builds the prelude, and the kernel caches the
///     flag on first read — set it later and the instrumentation is simply absent.
///   * `BROOD_NO_RELOAD_DIAG` silences the hot-reload chatter that function-tier
///     instrumentation legitimately provokes (it rebinds every project function).
///   * `--cover-lines`/`--cover-branches` also disable the JIT: an instrumented arm bails
///     lowering anyway, but turning it off outright keeps the measurement honest.
///   * The default memory ceiling (ADR-043), so a runaway test can't OOM the host; an
///     explicit `BROOD_MEM_LIMIT` still wins.
fn arm_test_env(argv: &[String]) {
    if argv.first().map(String::as_str) != Some("test") {
        return;
    }
    let has = |names: &[&str]| {
        argv.iter().any(|w| {
            names
                .iter()
                .any(|n| w == n || w.starts_with(&format!("{n}=")))
        })
    };
    if has(&["--cover-lines", "--cover-branches"]) {
        // SAFETY: called before any thread or interpreter is created.
        unsafe {
            std::env::set_var("BROOD_COVERAGE", "1");
            std::env::set_var("BROOD_NO_JIT", "1");
        }
    }
    if has(&[
        "--cover",
        "--cover-lines",
        "--cover-branches",
        "--cover-min",
    ]) {
        // SAFETY: as above.
        unsafe { std::env::set_var("BROOD_NO_RELOAD_DIAG", "1") };
    }
    // See `brood --test`: a green suite from a binary older than the `std/` under test is a
    // gate that lies, and this is where the reading gets believed.
    brood::cli_support::warn_if_stdlib_is_stale();
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
}

/// Run a Brood-implemented subcommand: `(nest/main argv)` returns the exit code.
fn run_blsp(max_parallel: Option<usize>, argv: Vec<String>) {
    if let Some(n) = max_parallel {
        brood::process::set_max_parallel(n);
    }
    brood::core::alloc::init_limits_from_env();
    brood::cli_support::warn_nondefault_gc_env();
    arm_test_env(&argv);
    let mut interp = Interp::new();
    if std::env::var_os("BROOD_NO_STDIMAGE").is_none() {
        ensure_stdimage_now(&mut interp);
    }
    let code = format!("(nest/main {})", blsp_string_list(&argv));
    if let brood::core::value::Value::Int(code) = run_for_value(&mut interp, &code) {
        std::process::exit(code as i32);
    }
}

fn run_main(cli: Cli) {
    if let Some(n) = cli.max_parallel {
        brood::process::set_max_parallel(n);
    }
    // Honour BROOD_MEM_LIMIT for every command; `nest test` (Brood-routed, see
    // `arm_test_env`) defaults a ceiling on so a runaway test can't OOM the host. `nest run`/`mcp`
    // stay unlimited unless the user opts in — the live image edits all day
    // (ADR-043).
    brood::core::alloc::init_limits_from_env();
    // Flag a stressed/retuned heap so a benchmark can't silently measure one.
    brood::cli_support::warn_nondefault_gc_env();

    // Completion runs on a KEYPRESS, so it must not pay interpreter boot for an
    // answer clap already knows. Both arms were below the unconditional
    // `Interp::new()` and so paid it anyway — 31 ms against a 9 ms floor for
    // `nest complete -- te`, whose answer ("test") is a static subcommand name; the
    // project-dependent path paid it TWICE, once here and once in
    // `print_dynamic_values`. Handle them before any interpreter exists, which is
    // what the module comment already claimed happened.
    match &cli.cmd {
        Cmd::Completions { shell } => return cmd_completions(*shell),
        Cmd::Complete { words } => return cmd_complete(words),
        _ => {}
    }

    let mut interp = Interp::new();

    // Make the stdlib startup image standard for a project, without asking. The image is what
    // makes `require` cheap (`json` 6.5 -> 1.7 ms, `http` 12.0 -> 3.6 ms; ADR-256), the prelude
    // installs one at boot whenever a current one exists, and the only thing missing on a fresh
    // machine — or the first run after a `brood` upgrade — is that nobody has WRITTEN it.
    //
    // `nest` is the right place to pay for that and `brood` is not: a project tool can afford
    // ~1 s once per stdlib change, while `brood app.blsp` is exactly the short-lived run the
    // cost would land on. The image is keyed on `system/stdlib-id` — the same for every binary
    // built from one tree — so `nest` building it also speeds up `brood`, `brood-lsp` and every
    // later `nest`. Skipped for the commands where a second of silence would be the whole
    // command (`--version`, `complete`), and never for `stdimage` itself, which does its own.
    // Skipped when the image is switched OFF (`BROOD_NO_STDIMAGE=1`): there is no point
    // spending ~1 s writing a file nothing will read, and doing it unconditionally made
    // every `nest` command in a parallel test run pay for it.
    if std::env::var_os("BROOD_NO_STDIMAGE").is_none() {
        ensure_stdimage(&mut interp, &cli.cmd);
    }

    match cli.cmd {
        // Handled above, before the interpreter is built.
        Cmd::Completions { .. } | Cmd::Complete { .. } => unreachable!(),
        // Nothing but the prelude is loaded when this runs — see the variant's doc for why
        // that is the whole point, and why `stdimage/build` refuses otherwise (KI-112).
        Cmd::Stdimage => run(
            &mut interp,
            concat!(
                "(require-one 'stdimage) ",
                "(let (r (stdimage/build)) ",
                "  (if (nil? r) ",
                "    (io/puts \"no cache directory (set XDG_CACHE_HOME or HOME)\") ",
                "    (io/puts (second r) \" bindings -> \" (first r) ",
                "      \" (shared by brood, nest and brood-lsp from this tree)\")))",
            ),
        ),
        Cmd::Repl => cmd_repl(&mut interp),
        Cmd::Mcp => {
            require_project("mcp", None);
            cmd_mcp(&mut interp)
        }
        Cmd::Observe { connect, cookie } => {
            require_terminal("observe");
            cmd_observe(&mut interp, connect, cookie)
        }
        Cmd::Attach { spec, cookie } => {
            require_terminal("attach");
            cmd_attach(&mut interp, spec, cookie)
        }
        Cmd::Release {
            output,
            runtime,
            targets,
            no_smoke,
        } => cmd_release(
            &mut interp,
            output.as_deref(),
            runtime.as_deref(),
            &targets,
            !no_smoke,
        ),
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
/// The `nest test` selection / execution flags, lowered to the Brood option
/// plist that `run-tests` and friends already accept. Selector *parsing* stays in
/// Brood (`test-make-filter`) so the grammar has one definition; this only
/// forwards argv.
fn blsp_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `(list "a" "b")`, or `nil` when empty — the shape `test-make-filter` expects.
fn blsp_string_list(items: &[String]) -> String {
    if items.is_empty() {
        "nil".to_string()
    } else {
        let quoted: Vec<String> = items.iter().map(|s| blsp_string(s)).collect();
        format!("(list {})", quoted.join(" "))
    }
}

/// `nest repl` — project-aware REPL. Inside a project, pre-load every source
/// file so the project's modules are immediately callable from the prompt.
/// Outside a project, fall through to the plain language REPL (same UX as
/// `brood`). The REPL itself is Brood (`std/tool/repl.blsp`, ADR-048) — one
/// implementation both binaries bootstrap into via `(repl/run)`.
fn cmd_repl(interp: &mut Interp) {
    if in_project() {
        // After loading the project's sources, tell the REPL to start in the project's
        // `:main` module namespace so a BARE project fn (`go`) resolves at the prompt
        // without qualifying it `myproj/main/go` — the interactive half of package
        // rooting (ADR-070). `*repl-start-ns*` is a plain `def` (not a `binding`) so it
        // reaches the spawned loop process, which roots + enters it through the ambient
        // package context `project-setup` just established. Other project modules still
        // need their `mod/fn` (or a `(defmodule …)`/`%in-ns` switch), exactly as in a file.
        run(
            interp,
            "(project/load-config) \
             (let (root (project/find-root (file/cwd))) \
               (when root \
                 (project/setup root) \
                 (project/load-sources root) \
                 (def repl/*repl-start-ns* (first *project-main*))))",
        );
        eprintln!(
            "nest repl — project sources loaded, in the project's main namespace; Ctrl-D to exit"
        );
    } else {
        eprintln!("nest repl — no project.blsp here; plain REPL (`brood` would do the same)");
    }
    // The REPL is Brood now (`std/tool/repl.blsp`), same as `brood` with no args. The
    // interactive editor enters raw mode (std/editor/lineedit.blsp), so guard the
    // terminal: the Brood `term-raw-leave` is the normal teardown, but this
    // restores it on a panic unwind too. Scope it like `cmd_observe` so it drops
    // (restoring) before any error report + exit (`process::exit` skips Drop).
    let result = {
        let _guard = RawTermGuard;
        interp.eval_str("(repl/run)")
    };
    if let Err(e) = result {
        report_error(&e);
        std::process::exit(1);
    }
}

/// `nest mcp` — see docs/mcp.md (ADR-036). Strictly per-project.
fn cmd_mcp(interp: &mut Interp) {
    // `setup-tooling-image` (std/tool/project.blsp) is the shared tooling bootstrap
    // the LSP also uses (via `introspect::load_tooling_image`) — sources + the
    // test/format frameworks — so the two servers can't drift on its contents.
    let bootstrap = r#"
        (project/load-config)
        (let (root (project/find-root (file/cwd)))
          (when (nil? root)
            (error "nest mcp: not in a Brood project (no project.blsp found from " (file/cwd) ")"))
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
                "(require-one 'observer) {}",
                brood::introspect::call_form("observer/observe-connect", &args)
            )
        }
        None => {
            // `--cookie` only authenticates a link, and the local demo makes none.
            // Say so rather than accepting a flag that does nothing — the same
            // "warn rather than ignore silently" rule `nest run --main` follows.
            if cookie.is_some() {
                eprintln!("nest observe: --cookie is ignored without --connect (the local demo opens no link)");
            }
            "(observer/observe-run)".to_string()
        }
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
        "(require-one 'editor/serve) {}",
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
    // Prove each binary boots before calling the release a success — the default;
    // `--no-smoke` clears it. See `smoke_test`.
    smoke: bool,
) {
    use brood::core::value::Value;

    // 1. Collect the manifest + module sources as a flat list of strings
    //    `(manifest stem0 src0 stem1 src1 …)`. Errors (e.g. not in a project) are
    //    reported + exit by `run_for_value`.
    let collected = run_for_value(
        interp,
        "(let (root (project/find-root (file/cwd))) \
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
    let name = match run_for_value(
        interp,
        "(if *project-name* (->string *project-name*) \"app\")",
    ) {
        Value::Str(id) => interp.heap.string(id).to_string(),
        _ => "app".to_string(),
    };

    // 3. Serialize the archive once — it's target-independent.
    let archive = brood::bundle::serialize(manifest, &modules);

    // 4. One release binary per target (no --target = one, for the host).
    //    --runtime names a single specific base, so it can't serve a matrix.
    if runtime.is_some() && targets.len() > 1 {
        eprintln!(
            "nest release: --runtime names one base binary; use it with at most one --target"
        );
        std::process::exit(2);
    }
    // A DEFAULTED output name is manifest data, not an argument: `:name` is read out
    // of a project.blsp that may not be ours (a cloned repo, a `nest release` run
    // before anyone read it). `(project :name |../../escaped-app|)` wrote a 30 MB
    // **executable** two directories above the project root, with no `-o` and no
    // warning — verified. A defaulted artifact must land in the project directory, so
    // require a plain filename; an explicit `-o PATH` is the user's own choice and
    // stays unrestricted (that is what it is for).
    if output.is_none() && !is_plain_filename(&name) {
        eprintln!(
            "nest release: the manifest's :name ({name:?}) is not a plain filename, so it \
             cannot name the output binary."
        );
        eprintln!("  Give the path explicitly: nest release -o <path>");
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
                    let exe = if release::is_windows_triple(t) {
                        ".exe"
                    } else {
                        ""
                    };
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
        if smoke {
            smoke_test(&out, triple);
        }
    }
}

/// The release's boot check — run the binary just written with the reserved
/// `--brood-boot-check` argument, so the ARTIFACT is proven to load its modules and
/// resolve `:main` before the release is called a success (KI-66).
///
/// **On by default**, and `--no-smoke` is the only way out. It was opt-in until
/// 2026-08-29, which is the wrong default for the one question a release has to
/// answer: "does the thing I just wrote start?" Nothing upstream answers it —
/// `nest check` resolves names and `nest test` runs the suite, and NEITHER loads
/// `main` — so an unbootable binary was reported as `Wrote app (41 modules, 31.5 MB)`
/// and discovered by whoever ran it. Running it costs one process and a module load.
///
/// Checking the source tree is not the same act either: the bundle carries a *snapshot*
/// of every dependency, so a dependency updated on disk since the last `nest fetch`, a
/// module outside `:source-paths`, or a `:main` naming a module that was never
/// collected are invisible upstream and fatal here. This is the only gate that runs
/// the thing that ships.
///
/// A binary that fails the check is **deleted**. A release that failed must not leave
/// an executable behind for a later `scp`/`docker COPY`/`gh release upload` to pick up:
/// the exit code is seen once, by whoever ran the command, while the file outlives it.
fn smoke_test(out: &std::path::Path, triple: Option<&str>) {
    // A cross-target binary cannot execute here. Say so — a smoke test that
    // quietly does nothing is the gate-that-cannot-fail shape (KI-68/70).
    if let Some(t) = triple {
        if t != release::host_triple() {
            println!("  smoke: skipped for {t} (not this host — run it on the target)");
            return;
        }
    }
    // `./out`, not `out`: a bare relative name is not a command on any shell's PATH,
    // and `Command` inherits that rule.
    let path = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::path::Path::new(".").join(out)
    };
    match std::process::Command::new(&path)
        .arg(brood::bundle::BUNDLE_BOOT_CHECK_ARG)
        .status()
    {
        Ok(s) if s.success() => println!("  smoke: boots"),
        Ok(s) => {
            eprintln!(
                "nest release: {} does NOT boot (exit {}) — the failure is above, from the \
                 binary itself",
                out.display(),
                s.code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string())
            );
            discard_unbootable(&path, out);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!(
                "nest release: cannot run {} for the boot check: {e}",
                out.display()
            );
            eprintln!("  Pass --no-smoke to write the binary without proving it boots.");
            std::process::exit(1);
        }
    }
}

/// Remove a binary that failed its boot check, reporting either way. Best-effort:
/// a removal that fails is *said*, not swallowed — the point of deleting it is that
/// nobody ships it by accident, so "I could not" has to be as loud as "I did".
fn discard_unbootable(path: &std::path::Path, out: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => eprintln!(
            "  removed {} (a release that fails is not an artifact)",
            out.display()
        ),
        Err(e) => eprintln!(
            "  WARNING: {} does not boot and could not be removed ({e}) — delete it by hand",
            out.display()
        ),
    }
}

/// Is `s` a single path component that names a file in the current directory —
/// no separator, no `.`/`..`, not empty? The test a *defaulted* output name has to
/// pass before it may become a filesystem path (see `cmd_release`). Deliberately
/// rejects `\` too: a name is data, and a name written for one platform must not
/// traverse on another.
fn is_plain_filename(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

// ---------- helpers ----------

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
    let result = interp.eval_str(code);
    // Restore on BOTH paths, exactly as `run` does — `nest run --for` routes a
    // full-screen TUI app through here, and an app that returned without its own
    // `term-raw-leave` would otherwise leave the shell wedged. No-op unless raw.
    brood::builtins::restore_terminal_on_exit();
    match result {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    }
}

// ── shell completion ────────────────────────────────────────────────────────
//
// Two halves, split by what owns the truth:
//
//   * Subcommand and flag names are read out of clap's OWN model
//     (`Cli::command()`), never a hand-kept list. That is the whole point: a flag
//     added to the `Cmd` enum is completable the same day, and a flag renamed
//     can't leave a stale completion behind.
//   * A Brood-routed subcommand (`BLSP_SUBCOMMANDS`) is handed to `nest/complete`, which
//     reads the same table the parser does — flags, fixed positionals, and the
//     project-dependent VALUES (tags, dep names, modules, test files) via
//     `std/tool/complete.blsp` — and only then pays interpreter boot.
//
// Everything here must be silent and total: completion runs on a keypress, so it
// prints candidates or nothing, exits 0, and never reports an error.

/// Every subcommand name clap knows about, hidden ones excluded.
fn subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|s| !s.is_hide_set())
        .map(|s| s.get_name().to_string())
        .chain(BLSP_SUBCOMMANDS.iter().map(|s| s.to_string()))
        .collect()
}

/// Completion for a Brood-implemented subcommand: its flags and positional values live in
/// `std/tool/nest.blsp`'s table, the one source of truth for what it accepts, so ask it.
/// Costs an interpreter boot, as the project-dependent values already did.
fn print_blsp_completion(subcommand: &str, prior: &[String], current: &str) {
    let mut interp = Interp::new();
    let code = format!(
        "(nest/complete {} {} {})",
        blsp_string(subcommand),
        blsp_string_list(prior),
        blsp_string(current)
    );
    let _ = interp.eval_str(&code);
}

/// The `--long` flags of one subcommand, plus the global ones.
fn flag_names(subcommand: &str) -> Vec<String> {
    let root = Cli::command();
    let Some(sub) = root.get_subcommands().find(|s| s.get_name() == subcommand) else {
        return Vec::new();
    };
    sub.get_arguments()
        .chain(root.get_arguments())
        .filter(|a| !a.is_hide_set())
        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
        .collect()
}

/// Does this argument take a value (so the word after it is a value, not a flag)?
fn takes_value(subcommand: &str, long: &str) -> bool {
    let root = Cli::command();
    let sub_takes = root
        .get_subcommands()
        .find(|s| s.get_name() == subcommand)
        .and_then(|sub| {
            sub.get_arguments()
                .find(|a| a.get_long() == Some(long))
                .map(|a| a.get_num_args().is_none_or(|n| n.takes_values()))
        });
    sub_takes.unwrap_or_else(|| {
        root.get_arguments()
            .find(|a| a.get_long() == Some(long))
            .is_some_and(|a| a.get_num_args().is_none_or(|n| n.takes_values()))
    })
}

/// The `--long` value-taking arg immediately before the cursor, if any.
fn pending_value_flag(subcommand: &str, words: &[String]) -> Option<String> {
    let previous = words.last()?;
    let long = previous.strip_prefix("--")?;
    // `--flag=value` is already complete; only a bare `--flag` leaves a value pending.
    if long.contains('=') {
        return None;
    }
    takes_value(subcommand, long).then(|| long.to_string())
}

/// `nest completions <shell>` — emit a shell integration script.
///
/// The scripts are deliberately thin: each one forwards the current words to
/// `nest complete` and offers whatever comes back, so there is exactly ONE
/// implementation of completion logic and the shells can't disagree with it (or go
/// stale when a flag is added). Each also falls back to the shell's own filename
/// completion when `nest complete` returns nothing, so a path is always typeable.
fn cmd_completions(shell: CompletionShell) {
    match shell {
        // `-o default` is the fallback: with no candidates, bash resumes normal
        // filename completion instead of offering nothing.
        CompletionShell::Bash => print!(
            r#"# nest completion for bash — eval "$(nest completions bash)"
_nest_complete() {{
    local IFS=$'\n'
    local words=("${{COMP_WORDS[@]:1:COMP_CWORD}}")
    # An empty trailing word means "completing a fresh word": keep it, so
    # `nest test <TAB>` differs from `nest tes<TAB>`.
    [[ ${{#words[@]}} -eq 0 ]] && words=("")
    COMPREPLY=($(nest complete -- "${{words[@]}}" 2>/dev/null))
    return 0
}}
complete -o default -o bashdefault -F _nest_complete nest
"#
        ),
        // NB the locals are `parts`/`candidates`, NOT `words`: zsh's completion
        // context provides `$words`, so declaring `local -a words` would blank it
        // before it could be read and every completion would see an empty command
        // line.
        CompletionShell::Zsh => print!(
            r#"# nest completion for zsh — eval "$(nest completions zsh)"
_nest_complete() {{
    local -a candidates parts
    parts=("${{(@)words[2,$CURRENT]}}")
    (( ${{#parts}} == 0 )) && parts=("")
    candidates=("${{(@f)$(nest complete -- "${{parts[@]}}" 2>/dev/null)}}")
    # `_files` is the fallback when nest has no opinion, so paths stay completable.
    if (( ${{#candidates}} == 0 )) || [[ -z "${{candidates[1]}}" ]]; then
        _files
    else
        compadd -- "${{candidates[@]}}"
    fi
}}
compdef _nest_complete nest
"#
        ),
        // fish has no "fall back to files" switch, so ask for both: nest's
        // candidates plus the usual file list.
        CompletionShell::Fish => print!(
            r#"# nest completion for fish — nest completions fish | source
function __nest_complete
    set -l tokens (commandline -opc) (commandline -ct)
    nest complete -- $tokens[2..-1] 2>/dev/null
end
complete -c nest -f -a '(__nest_complete)'
complete -c nest -a '(__fish_complete_path)'
"#
        ),
    }
}

/// `nest complete -- <words…>` — print one candidate per line for the word being
/// typed. `words` is everything after `nest`, with the (possibly empty) partial
/// word last. Always exits 0.
fn cmd_complete(words: &[String]) {
    // The word under the cursor, and the settled words before it.
    let (current, prior) = match words.split_last() {
        Some((last, rest)) => (last.clone(), rest.to_vec()),
        None => (String::new(), Vec::new()),
    };
    let subcommand = prior
        .iter()
        .find(|w| !w.starts_with('-'))
        .cloned()
        .filter(|w| subcommand_names().contains(w));

    // Static candidates are filtered and printed here; dynamic ones are printed by
    // Brood (which also filters), so `kind` is resolved and then handed over.
    let statics: Vec<String> = match &subcommand {
        // Still choosing a subcommand.
        None => subcommand_names(),
        Some(sub) if BLSP_SUBCOMMANDS.contains(&sub.as_str()) => {
            let after: Vec<String> = prior
                .iter()
                .skip_while(|w| w != &sub)
                .skip(1)
                .cloned()
                .collect();
            return print_blsp_completion(sub, &after, &current);
        }
        Some(sub) => {
            if current.starts_with('-') {
                flag_names(sub)
            } else if pending_value_flag(sub, &prior).is_some() {
                // A value position of a clap-side subcommand. None of these has a
                // project-dependent kind any more — every subcommand with one is
                // Brood-routed and completes through `nest/complete` — so print nothing and
                // let the shell fall back to filenames, which beats a confidently wrong list.
                return;
            } else if let Some(values) = positional_possible_values(sub) {
                // A `ValueEnum` positional (`nest completions <SHELL>`) — choices come from
                // the enum definition, not a restated list.
                values
            } else {
                return;
            }
        }
    };

    for c in statics {
        if !c.is_empty() && c.starts_with(&current) {
            println!("{c}");
        }
    }
}

/// A positional's `ValueEnum` choices (e.g. `nest grammar <TARGET>`), so those
/// come from the enum definition rather than being restated.
fn positional_possible_values(subcommand: &str) -> Option<Vec<String>> {
    let values: Vec<String> = Cli::command()
        .get_subcommands()
        .find(|s| s.get_name() == subcommand)?
        .get_positionals()
        .next()?
        .get_possible_values()
        .iter()
        .map(|v| v.get_name().to_string())
        .collect();
    (!values.is_empty()).then_some(values)
}

/// Reject a full-screen TUI subcommand when stdout isn't a terminal.
///
/// `nest observe` / `nest attach` drive an alternate-screen TUI. Piped or
/// redirected, the terminal primitives fail deep inside the render loop and the
/// user got `runtime error: terminal: No such device or address (os error 6)` with
/// an `at editor/ui/ui-run` frame — technically true, and useless. Say the actual
/// problem before anything is started.
fn require_terminal(command: &str) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        return;
    }
    eprintln!("nest {command}: needs an interactive terminal — stdout is not a tty.");
    eprintln!("  It draws a full-screen view, so it can't be piped or redirected.");
    eprintln!("  To capture output for a test, run it under a pty: script -qec 'nest {command}' /dev/null");
    std::process::exit(2);
}

/// Guard a project-scoped subcommand at the `nest` boundary.
///
/// Without this, running one outside a project surfaced a raw Brood `error`: a
/// bogus source position pointing into the bootstrap string (`1:58`), an internal
/// function name (`project/run-tests`), and an internal line number — for
/// what is only a wrong-directory mistake. Compare `cargo`: "could not find
/// `Cargo.toml` in /x or any parent directory". `hint` names the file-scoped
/// alternative when the command has one, so the error also teaches the way out.
fn require_project(command: &str, hint: Option<&str>) {
    if in_project() {
        return;
    }
    let cwd = std::env::current_dir().map_or_else(|_| ".".to_string(), |p| p.display().to_string());
    eprintln!("nest {command}: no project.blsp in {cwd} or any parent directory.");
    eprintln!("  Create one with `nest new <name>`, or cd into an existing project.");
    if let Some(hint) = hint {
        eprintln!("  {hint}");
    }
    std::process::exit(2);
}

/// Walk up from cwd looking for a `project.blsp` marker. Used by the
/// single-file `nest run/test/check` paths to decide whether to bootstrap
/// the project image, and by `require_project` to reject a project-scoped
/// command run outside one.
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
