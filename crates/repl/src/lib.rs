//! Shared REPL used by both the `brood` and `nest` binaries.
//!
//! Why a separate crate: `rustyline` is a CLI-side UX dep (ADR-005 / CLAUDE.md
//! "Dev/UX deps in the CLI crate are fine"). Both the `cli` and `nest` binaries
//! need a REPL — but the `brood-lsp` server, which also depends on the brood
//! lib, has no REPL. Living in its own crate keeps `rustyline` out of the LSP's
//! transitive dependency graph.
//!
//! The REPL itself is the smallest possible loop: accumulate input until the
//! delimiters balance (multi-line forms), `interp.eval_str`, print the value or
//! the error, reset the LOCAL heap back to a baseline so each command's
//! allocations are reclaimed. Globals live in the shared PRELUDE/RUNTIME
//! regions, so resetting LOCAL doesn't lose any `def`s.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use brood::Interp;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

/// Dispatch to the interactive (rustyline) REPL when stdin is a terminal, or
/// the plain line reader when it isn't (pipes, scripts). One entry point that
/// the binaries can call without their own `is_terminal` check.
pub fn repl(interp: &mut Interp) {
    if io::stdin().is_terminal() {
        if let Err(e) = repl_interactive(interp) {
            eprintln!("repl error: {}", e);
            std::process::exit(1);
        }
    } else {
        repl_plain(interp);
    }
}

/// Interactive REPL with full line editing and history (terminal only).
pub fn repl_interactive(interp: &mut Interp) -> rustyline::Result<()> {
    println!("brood v0.1 — arrow keys to edit, up/down for history, Ctrl-D to exit");
    let mut rl = DefaultEditor::new()?;
    let history = history_path();
    if let Some(path) = &history {
        let _ = rl.load_history(path);
    }

    // Baseline LOCAL size; after each command we truncate back to it,
    // reclaiming everything that command allocated. Safe because globals live
    // in the shared PRELUDE/RUNTIME regions, never in this process's LOCAL.
    let base = interp.heap.checkpoint();

    'repl: loop {
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
                Err(ReadlineError::Interrupted) => continue 'repl, // Ctrl-C
                Err(ReadlineError::Eof) => {
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
        interp.heap.reset_local_to(base);

        if let Some(path) = &history {
            let _ = rl.save_history(path);
        }
    }
}

/// Plain reader for non-terminal stdin (pipes, scripts). No prompts, no editing.
pub fn repl_plain(interp: &mut Interp) {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut pending = String::new();
    let base = interp.heap.checkpoint();

    loop {
        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => break,
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
        interp.heap.reset_local_to(base);
    }
}

/// Where to persist REPL history (`$HOME/.brood_history`), if `$HOME` is set.
fn history_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut path = PathBuf::from(home);
    path.push(".brood_history");
    Some(path)
}

/// Returns false while there are unclosed `()[]{}` or an open string literal,
/// so the REPL knows to keep reading for a multi-line form.
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

#[cfg(test)]
mod tests {
    use super::is_balanced;

    #[test]
    fn balanced_recognises_simple_forms() {
        assert!(is_balanced(""));
        assert!(is_balanced("(+ 1 2)\n"));
        assert!(is_balanced("[1 2 3]"));
        assert!(is_balanced("{:a 1}"));
    }

    #[test]
    fn unbalanced_recognises_dangling_open() {
        assert!(!is_balanced("(+ 1"));
        assert!(!is_balanced("[1 2"));
        assert!(!is_balanced("{:a"));
    }

    #[test]
    fn comments_ignore_delimiters() {
        assert!(is_balanced("; (this is a comment\n"));
    }

    #[test]
    fn strings_ignore_delimiters() {
        assert!(is_balanced("\"(unclosed inside string\""));
        // Unterminated string is *not* balanced — caller keeps reading.
        assert!(!is_balanced("\"open"));
    }

    #[test]
    fn escaped_quote_in_string_does_not_close_it() {
        assert!(is_balanced("\"hi \\\"there\\\"\""));
    }
}
