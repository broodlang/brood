//! The `brood` command-line tool.
//!
//! - With no arguments it starts a REPL.
//! - With file arguments it loads and runs each file in order.
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
    let mut interp = Interp::new();
    let files: Vec<String> = std::env::args().skip(1).collect();

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

fn run_files(interp: &mut Interp, files: &[String]) {
    for path in files {
        match std::fs::read_to_string(path) {
            Ok(src) => {
                if let Err(e) = interp.eval_str(&src) {
                    eprintln!("{}: {}", path, e);
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
