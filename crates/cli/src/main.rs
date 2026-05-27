//! The `brood` command-line tool.
//!
//! - With no arguments it starts a REPL.
//! - With file arguments it loads and runs each file in order.
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

use brood::Interp;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

fn main() {
    let (files, max_parallel) = parse_args(std::env::args().skip(1).collect());
    if let Some(n) = max_parallel {
        brood::process::set_max_parallel(n);
    }

    let mut interp = Interp::new();

    // `brood test` — discover and run the current project's test suite (ADR-020).
    // The runner (Brood, std/project.blsp) walks up from the cwd to `project.blsp`,
    // loads every tests/**/*_test.blsp, and runs the whole suite once. It raises
    // on failure, so a non-zero exit falls out of the eval error.
    if files.first().map(String::as_str) == Some("test") {
        if let Err(e) = interp.eval_str("(require 'project) (run-project-tests :trace)") {
            eprintln!("{}", e);
            std::process::exit(1);
        }
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
            .or_else(|| a.strip_prefix("-j"))
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

fn run_files(interp: &mut Interp, files: &[String]) {
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(src) => {
                if let Err(e) = interp.eval_source(&src) {
                    // GNU `FILE:LINE:COL: message` so editors (compilation-mode,
                    // flymake) can jump to the error; see `docs/tooling.md`.
                    match e.pos {
                        Some(p) => eprintln!("{}:{}:{}: {}", path, p.line, p.col, e),
                        None => eprintln!("{}: {}", path, e),
                    }
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
