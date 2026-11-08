//! The `nest` command — Brood project tooling.
//!
//! `nest` is the project/workspace tool sitting above the `brood` language
//! binary — the `cargo`/`rustc` split (ADR-028). For everyday
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

use brood::Interp;
use brood::cli_support::{
    FullTermGuard, RawTermGuard, init_runtime, report_error, run_on_main_stack,
};
use brood::introspect::string_literal;
use clap::{Parser, Subcommand};

mod mcp;
mod release;

#[derive(Parser, Debug)]
#[command(
    name = "nest",
    // The build sha, not just the semver — see `cli_support::VERSION_LINE`.
    version = brood::cli_support::VERSION_LINE,
    about = "Brood project tooling — the daily driver above the `brood` language binary (ADR-028).",
    after_help = "Also (implemented in Brood, std/tool/nest.blsp): new, run, test, check, format, doc, docs, doctest, grammar, rename, update-tooling, update-brood, fetch, update, tree, add, remove, publish, search, key, ws, repl, observe, attach, mcp, completions — `nest <command> --help`.",
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

#[derive(Subcommand, Debug)]
enum Cmd {
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
///
/// Skipped when the image is switched OFF (`BROOD_NO_STDIMAGE=1`): there is no point
/// spending ~1 s writing a file nothing will read, and doing it unconditionally made every
/// `nest` command in a parallel test run pay for it.
/// `nest stdimage` builds from SOURCE: unless the image is already off, run this same
/// command again with `BROOD_NO_STDIMAGE=1` and exit with its status.
///
/// The build `require`s every module and snapshots what it binds, and the boot installs any
/// current image first, so with the image on every module came OUT of the image being
/// replaced: a rebuild re-wrote the old values and could never repair one. That is how a
/// value baked from another build's prelude (`doc-catalog`'s section headers, read while a
/// shared prelude copy held an older text) survived every `nest stdimage` (2026-11-03).
fn stdimage_from_source() {
    if std::env::var_os("BROOD_NO_STDIMAGE").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let status = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("BROOD_NO_STDIMAGE", "1")
        .status();
    std::process::exit(status.ok().and_then(|st| st.code()).unwrap_or(1));
}

fn ensure_stdimage(interp: &mut Interp) {
    if std::env::var_os("BROOD_NO_STDIMAGE").is_some() {
        return;
    }
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
    // Unarmed, whatever this process is: an image is keyed on the stdlib's content alone and
    // read by every run, and a build with contracts armed would bind every sig'd function
    // to its checking shim and image THAT (ADR-381; `stdimage/build` refuses outright).
    let ok = std::process::Command::new(exe)
        .arg("stdimage")
        .env_remove("BROOD_CONTRACTS")
        // From source, never from the image being replaced (`stdimage_from_source`).
        .env("BROOD_NO_STDIMAGE", "1")
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
    brood::cli_support::grammar_vetting_entry();
    brood::cli_support::install_panic_reporting();
    // A subcommand implemented in Brood (`std/tool/nest.blsp`, ADR-322) is routed there
    // BEFORE clap sees argv: clap would reject its flags, which it no longer knows. The list
    // is the routing table; a name is in exactly one of it and the `Cmd` enum.
    let argv = brood::cli_support::args_lossy();
    if let Some((max_parallel, rest)) = blsp_routed(&argv) {
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
    "update-brood",
    "rename",
    "move",
    "fetch",
    "update",
    "tree",
    "add",
    "remove",
    "publish",
    "search",
    "key",
    "ws",
    "repl",
    "observe",
    "attach",
    "mcp",
    "dap",
    "profile",
    "completions",
    "complete",
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
        // Past a `--` every word is the subcommand's (`nest complete -- test -j 4`), so the
        // global option is no longer ours to take.
        if word == "--" {
            rest.extend(args[i..].iter().cloned());
            break;
        }
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
fn arm_test_env(argv: &[String], armed: bool) {
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
            // Pin the ceiling at the VM, whatever was asked: the recording is a bytecode
            // instruction, so native code runs without it and the tree-walker (which
            // runs every body at `BROOD_TIER=0`, ADR-416) never sees it — either would
            // report a measured 0%.
            std::env::set_var("BROOD_TIER", "1");
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
    // A test run says so, and gets a private scratch home for program state unless asked
    // to keep the real one (ADR-602): no project's tests write the user's config by accident.
    // Unless a parent already asked for one (`BROOD_TEST_ARM`, ADR-608) and it is armed.
    if !armed {
        brood::cli_support::arm_test_run(has(&["--real-home"]));
    }
    // See `brood --test`: a green suite from a binary older than the `std/` under test is a
    // gate that lies, and this is where the reading gets believed.
    brood::cli_support::warn_if_stdlib_is_stale();
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
}

/// Runtime contracts are ON by default in dev mode (ADR-381): `nest run` and `nest test`
/// arm contracts unless `BROOD_CONTRACTS` says otherwise, so every `(sig …)`
/// in the project and in std is enforced while the code is being developed and tested, and
/// never in a released bundle (a bundle runs the `brood` runtime, which reads the variable
/// and defaults it off). `BROOD_CONTRACTS=0` — or any value but `1` — opts out explicitly.
///
/// Set here, in Rust, for the same timing reason as the coverage flags above: the kernel
/// reads the variable once, and the prelude's own contracts are installed by `Interp::new`,
/// which runs before `std/tool/nest.blsp` could say anything. The policy — which commands
/// are dev mode — is this list and nothing else.
fn arm_contracts_default(argv: &[String]) {
    if !matches!(argv.first().map(String::as_str), Some("run" | "test")) {
        return;
    }
    // A process flag, not `set_var`: the default is THIS process's, and an exported
    // variable armed every program it started too (a benchmark's measured subjects, a
    // `nest stdimage` child). An explicit `BROOD_CONTRACTS` still wins, and is inherited.
    brood::cli_support::arm_contracts_by_default();
}

/// Run a Brood-implemented subcommand: `(nest/main argv)` returns the exit code.
fn run_blsp(max_parallel: Option<usize>, argv: Vec<String>) {
    init_runtime(max_parallel);
    if argv.first().map(String::as_str) == Some("check") {
        // `nest check` resolves every `:use`d std module from THIS binary's baked-in copy,
        // so from a binary older than the tree its verdict is against the old declarations
        // — green over an edit that should red, red over the edit that fixed it — and reads
        // exactly like a right one. A test run gets a warning (`arm_test_env`; its result
        // is usually still right); a checker run is refused (B7).
        brood::cli_support::refuse_if_stdlib_is_stale("nest check");
    }
    // Any subcommand started with `BROOD_TEST_ARM=1` is a test run (ADR-608): the editor's
    // generated runner under `nest run`, a warm test child — armed before the interpreter.
    let armed = brood::cli_support::arm_requested_test_run();
    arm_test_env(&argv, armed);
    arm_contracts_default(&argv);
    let mut interp = Interp::new();
    // `complete` runs on a keypress: read an image if there is one, never spend the
    // keypress building it.
    let completing = argv.first().is_some_and(|c| c == "complete");
    if !completing {
        ensure_stdimage(&mut interp);
    }
    let mut code = format!("(nest/main {})", blsp_string_list(&argv));
    if argv.first().is_some_and(|c| c == "mcp") {
        code = mcp::with_embedded_documents(&code);
    }
    // Terminal guards for a panic unwind, scoped so they drop before the exit
    // (`process::exit` skips Drop) and no-ops for a subcommand that never touches the
    // terminal. `observe`/`attach` draw full-screen and need the full teardown
    // (`FullTermGuard`: alternate screen + cursor); `repl`'s line editor only enters raw
    // mode and must NOT emit those escapes onto a pipe (`RawTermGuard`). The Brood side's
    // own `term-leave`/`term-raw-leave` is the normal teardown; restore is idempotent.
    let full_screen = matches!(argv.first().map(String::as_str), Some("observe" | "attach"));
    let result = {
        // `then`, not `then_some`: the latter builds its value eagerly and DROPS the
        // unwanted one on the spot — and a dropped `FullTermGuard` emits the teardown escapes
        // onto every command's stdout (seen as `[?25h[?1049l` after `nest check`).
        let _full = full_screen.then(|| FullTermGuard);
        let _raw = (!full_screen).then(|| RawTermGuard);
        run_for_value(&mut interp, &code)
    };
    // A test run's scratch home goes with it (a no-op for every other command).
    brood::cli_support::remove_test_scratch_home();
    if let brood::core::value::Value::Int(code) = result {
        std::process::exit(code as i32);
    }
}

fn run_main(cli: Cli) {
    if matches!(cli.cmd, Cmd::Stdimage) {
        stdimage_from_source();
    }
    init_runtime(cli.max_parallel);
    // `BROOD_TEST_ARM=1` makes every entry point a test run (ADR-608), not only the routed ones.
    brood::cli_support::arm_requested_test_run();

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
    if !matches!(cli.cmd, Cmd::Stdimage) {
        ensure_stdimage(&mut interp);
    }

    match cli.cmd {
        // Nothing but the prelude is loaded when this runs — see the variant's doc for why
        // that is the whole point, and why `stdimage/build` refuses otherwise (KI-112).
        Cmd::Stdimage => {
            run_for_value(
                &mut interp,
                concat!(
                    "(require-one 'stdimage) ",
                    "(let (r (stdimage/build)) ",
                    "  (if (nil? r) ",
                    "    (io/puts \"no cache directory (set XDG_CACHE_HOME or HOME)\") ",
                    "    (io/puts (second r) \" bindings -> \" (first r) ",
                    "      \" (shared by brood, nest and brood-lsp from this tree)\")))",
                ),
            );
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

/// Read a Brood list of strings into owned `String`s, or exit naming `what` returned
/// the bad value. Owned deliberately: the list is not rooted, so a later `eval` could
/// collect it out from under a borrow.
fn string_list(interp: &mut Interp, what: &str, list: brood::core::value::Value) -> Vec<String> {
    use brood::core::value::Value;
    let items = match interp.heap.seq_items(list) {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    };
    items
        .iter()
        .map(|v| match v {
            Value::Str(id) => Ok(interp.heap.string(*id).to_string()),
            other => Err(interp.print(*other)),
        })
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|bad| {
            eprintln!("nest release: {what} returned a non-string ({bad})");
            std::process::exit(1);
        })
}

/// `(list "a" "b")`, or `nil` when empty — the shape `test-make-filter` expects.
fn blsp_string_list(items: &[String]) -> String {
    if items.is_empty() {
        "nil".to_string()
    } else {
        let quoted: Vec<String> = items.iter().map(|s| string_literal(s)).collect();
        format!("(list {})", quoted.join(" "))
    }
}

/// `nest release [-o PATH] [--runtime PATH] [--target TRIPLE]…` — bundle the
/// project into one self-contained executable per target (ADR-038). Collection
/// is policy (Brood: `project-release/bundle-collect`); byte assembly + I/O is mechanism
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
         (project-release/bundle-collect root))",
    );
    let strings = string_list(interp, "bundle-collect", collected);
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
        "(if project/*name* (->string project/*name*) \"app\")",
    ) {
        Value::Str(id) => interp.heap.string(id).to_string(),
        _ => "app".to_string(),
    };

    // 3. Serialize the archive once — it's target-independent. The startup image is not:
    //    each runtime writes its own, after the first write below (`embed_startup_image`).
    let archive = brood::bundle::serialize(manifest, &modules, None);

    // 4. What are the binaries CALLED, and is any name refused? Policy, so Brood
    //    answers it (`project-release/release-plan`): a flat `("ok" triple out …)` list with
    //    an empty triple meaning the host, or `("error" message exit-code)`.
    let planned = run_for_value(
        interp,
        &format!(
            "(project-release/release-plan {} {} {} {})",
            string_literal(&name),
            output
                .map(string_literal)
                .unwrap_or_else(|| "nil".to_string()),
            blsp_string_list(targets),
            runtime.is_some(),
        ),
    );
    let plan_words = string_list(interp, "release-plan", planned);
    let plans: Vec<(Option<String>, std::path::PathBuf)> = match plan_words.split_first() {
        Some((tag, rest)) if tag == "ok" => rest
            .chunks(2)
            .map(|c| {
                let triple = (!c[0].is_empty()).then(|| c[0].clone());
                (triple, std::path::PathBuf::from(&c[1]))
            })
            .collect(),
        Some((tag, rest)) if tag == "error" && rest.len() == 2 => {
            eprintln!("nest release: {}", rest[0]);
            std::process::exit(rest[1].parse().unwrap_or(2));
        }
        _ => {
            eprintln!("nest release: release-plan returned an unrecognised answer");
            std::process::exit(1);
        }
    };
    for (triple, out) in plans {
        let triple = triple.as_deref();
        let base = release::resolve_runtime(runtime, triple);
        if let Err(e) = brood::bundle::write_release(&base, &archive, &out) {
            eprintln!("nest release: cannot write {}: {e}", out.display());
            std::process::exit(1);
        }
        let image_note = embed_startup_image(&out, triple, &base, manifest, &modules);
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        // What the command SAYS it wrote is policy too (`project-release/release-report`).
        let line = run_for_value(
            interp,
            &format!(
                "(project-release/release-report {} {} {} {})",
                string_literal(&out.display().to_string()),
                modules.len(),
                size,
                triple
                    .map(string_literal)
                    .unwrap_or_else(|| "nil".to_string()),
            ),
        );
        match line {
            Value::Str(id) => println!("{}", interp.heap.string(id)),
            other => println!("{}", interp.print(other)),
        }
        println!("  image: {image_note}");
        if smoke {
            smoke_test(&out, triple);
        }
    }
}

/// `out` as something `Command` can run: `./out`, not `out`, because a bare relative name
/// is not a command on any shell's PATH, and `Command` inherits that rule.
fn runnable_path(out: &std::path::Path) -> std::path::PathBuf {
    if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::path::Path::new(".").join(out)
    }
}

/// Give the binary just written its startup image (ADR-484), and say what happened.
///
/// The image is written BY THE BINARY (`--brood-build-image`): it loads its own embedded
/// sources on its own runtime and std, so the image is valid for exactly that runtime by
/// construction — `nest`'s runtime is a different build, and an image it wrote would never
/// match the shipped one's stamp. The binary is then rewritten with the image after its
/// modules. Best-effort: a project that cannot be imaged (a `table` global, say), or a
/// target this host cannot run, ships without one and loads its modules from source, as
/// every bundle did before; the release is no less correct, only slower to start.
fn embed_startup_image(
    out: &std::path::Path,
    triple: Option<&str>,
    base: &[u8],
    manifest: &str,
    modules: &[(String, String)],
) -> String {
    if let Some(t) = triple
        && t != release::HOST_TRIPLE
    {
        return format!("none for {t} (only the target can build it) — modules load from source");
    }
    let stem = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "release".to_string());
    let image_path = out.with_file_name(format!(".{stem}.nest-image-{}", std::process::id()));
    let built = std::process::Command::new(runnable_path(out))
        .arg(brood::bundle::BUNDLE_BUILD_IMAGE_ARG)
        .arg(&image_path)
        .output();
    let image = std::fs::read(&image_path);
    let _ = std::fs::remove_file(&image_path);
    let failed = |why: String| format!("none ({why}) — modules load from source");
    let image = match (built, image) {
        (Ok(run), Ok(image)) if run.status.success() => image,
        (Ok(run), _) => {
            let stderr = String::from_utf8_lossy(&run.stderr);
            let why = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("the build failed");
            return failed(why.trim().to_string());
        }
        (Err(e), _) => return failed(format!("cannot run the binary: {e}")),
    };
    let archive = brood::bundle::serialize(manifest, modules, Some(&image));
    if let Err(e) = brood::bundle::write_release(base, &archive, out) {
        eprintln!("nest release: cannot write {}: {e}", out.display());
        std::process::exit(1);
    }
    format!("{} KB, written by the bundled runtime", image.len() / 1024)
}

/// The release's boot check — run the binary just written with the reserved
/// `--brood-boot-check` argument, so the ARTIFACT is proven to load its modules and
/// resolve `:main` before the release is called a success (KI-66).
///
/// **On by default**, and `--no-smoke` is the only way out: it is the one question a
/// release has to answer — "does the thing I just wrote start?" — and nothing upstream
/// answers it (`nest check` resolves names and `nest test` runs the suite, and NEITHER
/// loads `main`). Running it costs one process and a module load.
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
    if let Some(t) = triple
        && t != release::HOST_TRIPLE
    {
        println!("  smoke: skipped for {t} (not this host — run it on the target)");
        return;
    }
    let path = runnable_path(out);
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

// ---------- helpers ----------

/// Evaluate `code` and return its last value, or report the error and exit 1.
fn run_for_value(interp: &mut Interp, code: &str) -> brood::core::value::Value {
    let result = interp.eval_str(code);
    // Restore the terminal on the way out — whether the program returned cleanly or
    // threw. A full-screen TUI app that entered raw mode / the alternate screen and never
    // reached its Brood `term-raw-leave` would otherwise leave the shell wedged.
    // `process::exit` skips Drop, so a guard wouldn't fire — restore explicitly. The call
    // is a no-op unless the terminal was left raw.
    brood::builtins::restore_terminal_on_exit();
    match result {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    }
}
