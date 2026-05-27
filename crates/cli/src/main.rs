//! The `brood` command-line tool — the language binary.
//!
//! `brood` only runs the language; project tooling (scaffolding, test
//! discovery, config) lives in the separate `nest` binary — the
//! `rustc`/`cargo`, `elixir`/`mix` split (ADR-027).
//!
//! - With no arguments it starts a REPL.
//! - With file arguments it loads and runs each file in order.
//! - `--test <file>...` loads each file (which registers its cases) and runs
//!   the in-language suite once — a single-file test run, distinct from
//!   `nest test`'s project-wide discovery.
//! - `--version` prints the version.
//! - `-j N` / `--max-parallel N` caps how many spawned processes run on OS
//!   threads at once (0 = unlimited, the default). Useful for bounding a
//!   concurrent test run; see `std/test.blsp`.
//!
//! Interactively (a real terminal) the REPL uses `rustyline` for line editing:
//! arrow keys to move within a line, up/down to walk history, the usual
//! Emacs-style bindings (Ctrl-A/E/K/R, ...), and persistent history. When stdin
//! is not a terminal (piped input, scripts) it falls back to a plain
//! line-by-line reader so non-interactive use stays clean.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use brood::error::LispError;
use brood::Interp;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

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
brood — the Brood language (the language half of the brood/nest split, ADR-028)

usage:
  brood                   start the REPL
  brood <file>...         run each file in order
  brood --test <file>...  run the file(s) as a single in-language test suite

options:
  -j, --max-parallel N    cap concurrent spawned processes (0 = unlimited)
  -h, --help              print this help
      --version           print the version

For project tasks (scaffolding, project-wide test discovery) use `nest`.";

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", HELP);
        return;
    }

    if raw.iter().any(|a| a == "--version" || a == "-V") {
        println!("brood {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    // `--test <file>...` runs the files as a single in-language suite (load to
    // register cases, then `(run-tests)` once). Project-wide discovery lives in
    // `nest test`, not here. The flag is stripped before file parsing.
    let test_mode = raw.iter().any(|a| a == "--test");
    let raw: Vec<String> = raw.into_iter().filter(|a| a != "--test").collect();

    let (files, max_parallel) = parse_args(raw);
    if let Some(n) = max_parallel {
        brood::process::set_max_parallel(n);
    }

    let mut interp = Interp::new();

    if test_mode {
        run_test_files(&mut interp, &files);
        return;
    }

    if !files.is_empty() {
        run_files(&mut interp, &files);
        return;
    }

    if io::stdin().is_terminal() {
        if let Err(e) = repl_interactive(&mut interp) {
            eprintln!("repl error: {}", e);
            std::process::exit(1);
        }
    } else {
        repl_plain(&mut interp);
    }
}

/// Split CLI args into file paths and an optional concurrency cap. Accepts
/// `-j N`, `--jobs N`, `--max-parallel N`, and the `=`/joined forms (`-jN`,
/// `--max-parallel=N`). A bad value is a hard error so a typo never silently
/// runs unbounded.
fn parse_args(args: Vec<String>) -> (Vec<String>, Option<usize>) {
    let mut files = Vec::new();
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
            // Only the joined `-jN` form; otherwise a file like `-justfile` would
            // be misread as a flag. The explicit `=`/spaced forms still error on
            // a bad value above.
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit()))
        {
            Some(v.to_string())
        } else {
            files.push(a.clone());
            None
        };
        if let Some(v) = value {
            match v.parse::<usize>() {
                Ok(n) => max_parallel = Some(n),
                Err(_) => {
                    eprintln!("brood: {} expects a number, got {:?}", a, v);
                    std::process::exit(2);
                }
            }
        }
        i += 1;
    }
    (files, max_parallel)
}

/// `brood --test <file>...`: load each file (registering its cases via the
/// `test` framework), then run the whole in-language suite once. Output is the
/// same structured GNU `FILE:LINE:COL:` block per failure as `nest test`; see
/// `docs/tooling.md`. This is a single-file run — for project-wide discovery
/// (walk to `project.blsp`, load `tests/**/*_test.blsp`) use `nest test`.
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

fn run_files(interp: &mut Interp, files: &[String]) {
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(src) => {
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

/// Interactive REPL with full line editing and history (terminal only).
fn repl_interactive(interp: &mut Interp) -> rustyline::Result<()> {
    println!("brood v0.1 — arrow keys to edit, up/down for history, Ctrl-D to exit");
    let mut rl = DefaultEditor::new()?;
    let history = history_path();
    if let Some(path) = &history {
        let _ = rl.load_history(path);
    }

    // Baseline LOCAL size; after each command we truncate back to it, reclaiming
    // everything that command allocated. Safe because globals live in the shared
    // PRELUDE/RUNTIME regions, never in this process's LOCAL heap.
    let base = interp.heap.checkpoint();

    'repl: loop {
        // Accumulate input lines until the form is delimiter-balanced, so
        // multi-line forms work while each physical line stays editable.
        let mut buffer = String::new();
        let mut prompt = "brood> ";
        loop {
            match rl.readline(prompt) {
                Ok(line) => {
                    buffer.push_str(&line);
                    buffer.push('\n');
                    if is_balanced(&buffer) {
                        break;
                    }
                    prompt = "  ...   ";
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl-C: abandon the current (possibly partial) input.
                    continue 'repl;
                }
                Err(ReadlineError::Eof) => {
                    // Ctrl-D: save history and exit.
                    if let Some(path) = &history {
                        let _ = rl.save_history(path);
                    }
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }

        let src = buffer.trim();
        if src.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(src);

        match interp.eval_str(&buffer) {
            Ok(value) => println!("{}", interp.print(value)),
            Err(e) => eprintln!("{}", e),
        }
        interp.heap.reset_local_to(base); // reclaim this command's allocations

        if let Some(path) = &history {
            let _ = rl.save_history(path);
        }
    }
}

/// Plain reader for non-terminal stdin (pipes, scripts). No prompts, no editing.
fn repl_plain(interp: &mut Interp) {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut pending = String::new();
    let base = interp.heap.checkpoint();

    loop {
        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("input error: {}", e);
                break;
            }
        }

        pending.push_str(&line);
        if !is_balanced(&pending) {
            continue;
        }

        let src = std::mem::take(&mut pending);
        if src.trim().is_empty() {
            continue;
        }

        match interp.eval_str(&src) {
            Ok(value) => {
                println!("{}", interp.print(value));
                io::stdout().flush().ok();
            }
            Err(e) => eprintln!("{}", e),
        }
        interp.heap.reset_local_to(base); // reclaim this command's allocations
    }
}

/// Where to persist REPL history (`$HOME/.brood_history`), if `$HOME` is set.
fn history_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".brood_history");
    Some(path)
}

/// Returns false while there are unclosed `()[]{}` or an open string literal, so
/// the REPL knows to keep reading more input lines for a multi-line form.
fn is_balanced(src: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut in_comment = false;

    for c in src.chars() {
        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            ';' => in_comment = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }

    !in_string && depth <= 0
}
