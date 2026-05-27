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
//!   nest test         discover tests/**/*_test.blsp and run the suite once
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
  nest test         discover tests/**/*_test.blsp and run the suite once
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
