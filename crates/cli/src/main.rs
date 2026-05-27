//! The `mylisp` command-line tool.
//!
//! - With no arguments it starts a REPL.
//! - With file arguments it loads and runs each file in order.
//!
//! The REPL is intentionally dependency-free for v0.1 (just line-buffered
//! stdin). A richer line editor (history, completion) is on the roadmap.

use std::io::{self, BufRead, Write};

use mylisp::{printer, Interp};

fn main() {
    let interp = Interp::new();
    let files: Vec<String> = std::env::args().skip(1).collect();

    if files.is_empty() {
        repl(&interp);
        return;
    }

    for path in &files {
        match std::fs::read_to_string(path) {
            Ok(src) => {
                if let Err(e) = interp.eval_str(&src) {
                    eprintln!("{}: {}", path, e);
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("mylisp: cannot read {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }
}

fn repl(interp: &Interp) {
    println!("mylisp v0.1 — type an expression, Ctrl-D to exit");
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut pending = String::new();

    loop {
        let prompt = if pending.is_empty() { "mylisp> " } else { "  ...   " };
        print!("{}", prompt);
        io::stdout().flush().ok();

        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => {
                println!();
                break; // EOF (Ctrl-D)
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("input error: {}", e);
                break;
            }
        }

        pending.push_str(&line);
        // Keep reading lines until the form is delimiter-balanced.
        if !is_balanced(&pending) {
            continue;
        }

        let src = std::mem::take(&mut pending);
        if src.trim().is_empty() {
            continue;
        }

        match interp.eval_str(&src) {
            Ok(value) => println!("{}", printer::print(&value)),
            Err(e) => eprintln!("{}", e),
        }
    }
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
