//! The `nest` command — Brood project tooling.
//!
//! `nest` is the project/workspace tool sitting above the `brood` language
//! binary — the `cargo`/`rustc`, `mix`/`elixir` split (ADR-028). `brood` only
//! runs the language; everything project-shaped (scaffolding, test discovery,
//! user config) lives here. `nest` is a *thin Rust shell*: the actual policy —
//! name checks, templates, discovery — is written in Brood (`std/project.blsp`)
//! and driven through `Interp`, keeping behaviour in the language (ADR-006).
//!
//!   nest new <name>   scaffold a new project (project.blsp + src/ + tests/)
//!   nest run [args…]  run the project's entry point (configured via :main)
//!   nest test         discover tests/**/*_test.blsp and run the suite once
//!   nest check        advisory type-check every .blsp under src/ + tests/
//!   nest format       format every .blsp under src/ and tests/ in place
//!   nest format --check  exit non-zero if any file would change (CI mode)
//!
//! `-j N` / `--max-parallel N` caps how many spawned processes run on OS
//! threads at once (0 = unlimited, the default) — useful for bounding a
//! concurrent test run; see `std/test.blsp`.

use brood::error::LispError;
use brood::Interp;

/// Print an error as a GNU `FILE:LINE:COL: message` line (editor-parseable),
/// followed — when the file and position are known — by the offending source
/// line and a caret under the column. See `docs/tooling.md`.
fn report_error(e: &LispError) {
    eprintln!("{}", e.located());
    if let (Some(file), Some(pos)) = (&e.file, e.pos) {
        if let Ok(src) = std::fs::read_to_string(file) {
            if let Some(line) = src.lines().nth(pos.line.saturating_sub(1) as usize) {
                eprintln!("    {}", line);
                let pad = " ".repeat(pos.col.saturating_sub(1) as usize);
                eprintln!("    {}^", pad);
            }
        }
    }
}

const HELP: &str = "\
nest — Brood project tooling (the project half of the brood/nest split, ADR-028)

usage:
  nest new <name>   scaffold a new project (project.blsp + src/ + tests/)
  nest run [args…]  run the project's entry point (set via :main in project.blsp;
                    defaults to module `main`, fn `main`). Extra args are passed
                    to the entry fn as strings.
  nest test         discover tests/**/*_test.blsp and run the suite once
  nest check        advisory type-check every .blsp under src/ + tests/
                    (exits non-zero on warnings — for CI)
  nest format       format every .blsp under src/ and tests/ in place
                    (use --check to exit non-zero on diffs instead of writing)
  nest doc [module] emit Markdown docs (whole project, or one named module)

options:
  -j, --max-parallel N   cap concurrent spawned processes (test runs)
  -h, --help             print this help
      --version          print the version

To run the language itself (REPL, a file, a single test file) use `brood`.";

/// Print help to stdout and exit 0 (`--help` is a request, not an error).
fn help() -> ! {
    println!("{}", HELP);
    std::process::exit(0);
}

/// Print help to stderr and exit non-zero (no/unknown command — an error).
fn usage() -> ! {
    eprintln!("{}", HELP);
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        help();
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("nest {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let (positional, max_parallel) = parse_args(args);
    if let Some(n) = max_parallel {
        brood::process::set_max_parallel(n);
    }

    let cmd = match positional.first() {
        Some(c) => c.as_str(),
        None => usage(),
    };

    let mut interp = Interp::new();

    match cmd {
        // `nest test` — discover and run the current project's test suite
        // (ADR-020). The runner (Brood, std/project.blsp) walks up from the cwd
        // to `project.blsp`, loads every tests/**/*_test.blsp, and runs the
        // whole suite once. It raises on failure, so a non-zero exit falls out
        // of the eval error. One output format — structured GNU
        // `FILE:LINE:COL:` blocks; see `docs/tooling.md`.
        "test" => run(
            &mut interp,
            "(require 'project) (load-config) (run-project-tests)",
        ),

        // `nest check` — advisory type-check the whole project (every .blsp under
        // src/ + tests/) without running anything. Mirrors `brood --check <file>`
        // at project scope. Warnings go to stdout (so callers can pipe them); the
        // process exits non-zero when any warning was emitted, so CI can gate on
        // it. The check itself is policy in Brood (`check-project` in
        // `std/project.blsp`); the Rust side just turns the returned count into
        // an exit code. See `docs/types.md`.
        "check" => {
            // Same bootstrap order as the other subcommands. `check-project`
            // prints each warning and returns the total count; we exit with
            // that count clamped to 1 if non-zero.
            //
            // We `(require 'test)` first so the unbound-symbol pass doesn't
            // flag `test` / `describe` / `assert=` / … in `tests/**/*_test.blsp`.
            // Test files normally start with `(require 'test)` themselves, but
            // the checker reads files without executing them, so the require
            // never runs through the checker's eyes — pre-loading the module
            // here makes its exports globally visible, matching what
            // `run-project-tests` does before its embedded check.
            let v = run_for_value(
                &mut interp,
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
        }

        // `nest new <name>` — scaffold a new project (ADR-020): a folder with
        // project.blsp + src/ + tests/ and starter files. The policy (name
        // checks, templates) is in Brood (std/project.blsp); we just pass the
        // name in, escaped so it can't break out of the string literal.
        "new" => {
            let name = positional.get(1).unwrap_or_else(|| {
                eprintln!("nest new: expected a project name, e.g. `nest new foobar`");
                std::process::exit(2);
            });
            let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
            let code = format!(
                "(require 'project) (load-config) (new-project \"{}\")",
                escaped
            );
            run(&mut interp, &code);
        }

        // `nest format` — reformat every .blsp under src/ + tests/ in place
        // (ADR-020 extension). `--check` flips to a read-only mode: same walk
        // but exits non-zero if any file would change, used for CI. Mechanism
        // is one Rust primitive (`parse-source`) + std/format.blsp; this arm
        // just chooses which Brood entry to call. Operands beyond the flag are
        // rejected — there are no other args, and silently ignoring them would
        // mask typos.
        "format" => {
            let mut check = false;
            for a in positional.iter().skip(1) {
                match a.as_str() {
                    "--check" | "-c" => check = true,
                    other => {
                        eprintln!("nest format: unexpected argument {:?}", other);
                        std::process::exit(2);
                    }
                }
            }
            let entry = if check {
                "(format-project-check)"
            } else {
                "(format-project)"
            };
            // Same bootstrap order as the other subcommands: project first
            // (defines `load-config`), then load-config (which writes the
            // default user config if absent), then the feature module. Format
            // depends on project's discovery helpers via its own require.
            let code = format!(
                "(require 'project) (load-config) (require 'format) {}",
                entry
            );
            run(&mut interp, &code);
        }

        // `nest run [args…]` — run the project's entry point (ADR-020). The
        // entry is configured via `:main` in project.blsp (a module symbol, or a
        // `(module fn)` list) and defaults to module `main`, fn `main`. Extra
        // positional args after `run` are passed to the entry as strings. The
        // policy (find the project root, load it, resolve and apply the entry)
        // lives in Brood (std/project.blsp); we just build the args list.
        "run" => {
            let escaped_args = positional
                .iter()
                .skip(1)
                .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
                .collect::<Vec<_>>()
                .join(" ");
            let code = format!(
                "(require 'project) (load-config) (run-project (list {}))",
                escaped_args
            );
            run(&mut interp, &code);
        }

        // `nest doc [module]` — generate Markdown documentation to stdout. With
        // no operand it documents the whole project (every source file under
        // it); with a module name it documents that one module (a baked-in std
        // module, or one on the load-path). The policy (load + introspect via
        // `doc`/`arglist`/`global-names`) is Brood (std/docs.blsp); we pass the
        // optional name through, escaped so it can't break out of the literal.
        "doc" => {
            let code = match positional.get(1) {
                Some(name) => {
                    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
                    format!("(require 'docs) (generate-docs \"{}\")", escaped)
                }
                None => "(require 'docs) (generate-docs)".to_string(),
            };
            run(&mut interp, &code);
        }

        other => {
            eprintln!("nest: unknown command {:?}", other);
            usage();
        }
    }
}

/// Evaluate a bootstrap snippet, reporting any error in editor-parseable form
/// and exiting non-zero on failure.
fn run(interp: &mut Interp, code: &str) {
    if let Err(e) = interp.eval_str(code) {
        report_error(&e);
        std::process::exit(1);
    }
}

/// Like [`run`], but returns the bootstrap's last value so the caller can
/// decide whether to exit non-zero based on it. Used by `nest check` to turn
/// "warnings were emitted" into a non-zero exit without throwing a synthetic
/// error (and the noise that would follow).
fn run_for_value(interp: &mut Interp, code: &str) -> brood::core::value::Value {
    match interp.eval_str(code) {
        Ok(v) => v,
        Err(e) => {
            report_error(&e);
            std::process::exit(1);
        }
    }
}

/// Split CLI args into positional words (the subcommand + its operands) and an
/// optional concurrency cap. Accepts `-j N`, `--jobs N`, `--max-parallel N`,
/// and the `=`/joined forms (`-jN`, `--max-parallel=N`). A bad value is a hard
/// error so a typo never silently runs unbounded.
fn parse_args(args: Vec<String>) -> (Vec<String>, Option<usize>) {
    let mut positional = Vec::new();
    let mut max_parallel = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let value = if a == "-j" || a == "--jobs" || a == "--max-parallel" {
            i += 1;
            args.get(i).cloned()
        } else if let Some(v) = a
            .strip_prefix("--max-parallel=")
            .or_else(|| a.strip_prefix("--jobs="))
        {
            Some(v.to_string())
        } else if let Some(v) = a
            .strip_prefix("-j")
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        {
            Some(v.to_string())
        } else {
            positional.push(a.clone());
            None
        };
        if let Some(v) = value {
            match v.parse::<usize>() {
                Ok(n) => max_parallel = Some(n),
                Err(_) => {
                    eprintln!("nest: {} expects a number, got {:?}", a, v);
                    std::process::exit(2);
                }
            }
        }
        i += 1;
    }
    (positional, max_parallel)
}
