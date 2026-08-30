//! The in-browser Brood playground: a wasm-bindgen shim exposing `run(src)` to JS,
//! plus editor helpers — `highlight(src)` (syntax spans) and `completions(prefix)`
//! (builtin/prelude names from the runtime's own global table).
//!
//! `run` builds a fresh interpreter (a clean image per run — no state leaks between
//! snippets), captures anything the snippet prints, evaluates it, and returns the
//! captured output followed by the value of the last form (or an error report).
//! Evaluation runs on the single wasm thread; snippets that `spawn` green processes or
//! touch the network/filesystem will error (those subsystems are stubbed on wasm — see
//! `crates/lisp` wasm gating), which is fine for a language playground.

use std::sync::OnceLock;

use brood::Interp;
use wasm_bindgen::prelude::*;

/// Evaluate Brood `source` and return its output as text: captured stdout (from
/// `print`/`println`) followed by the printed value of the last form, or an error
/// report if a form raised or failed to parse.
#[wasm_bindgen]
pub fn run(source: &str) -> String {
    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();
    // On wasm, run the snippet as a green process driven by the cooperative single-thread
    // scheduler, so `spawn`/`send`/`receive` work; `run_program_repr` returns the printed
    // last value across the process-heap boundary. Off wasm (the host workspace build),
    // fall back to `eval_source` on this thread.
    #[cfg(target_arch = "wasm32")]
    let result = interp.run_program_repr(source);
    #[cfg(not(target_arch = "wasm32"))]
    let result = match interp.eval_source(source) {
        Ok(value) => Ok(interp.print(value)),
        Err(error) => Err(error),
    };
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    match result {
        Ok(printed) => {
            if captured.is_empty() {
                printed
            } else if printed.is_empty() {
                captured
            } else {
                format!("{captured}\n{printed}")
            }
        }
        Err(error) => {
            if captured.is_empty() {
                format!("error: {error}")
            } else {
                format!("{captured}\nerror: {error}")
            }
        }
    }
}

/// The Brood version string, for the playground UI to show which build it runs.
///
/// `brood::VERSION`, not `env!("CARGO_PKG_VERSION")` — the latter expands to THIS crate's
/// version, so the playground reported `0.1.0` as the Brood build it was running.
#[wasm_bindgen]
pub fn version() -> String {
    brood::VERSION.to_string()
}

// ---- editor helpers ---------------------------------------------------------

/// Syntax-highlight `source` into HTML: the same text, with tokens wrapped in
/// `<span class="tk-…">` (comment / string / paren / keyword / number / const). The
/// caller renders it in a `<pre>` overlay behind a transparent `<textarea>`, so the
/// output must escape HTML and preserve every byte (whitespace included) 1:1 with the
/// input, or the overlay drifts out of alignment with the editor.
#[wasm_bindgen]
pub fn highlight(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len() + source.len() / 2);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == ';' {
            out.push_str("<span class=\"tk-comment\">");
            while i < chars.len() && chars[i] != '\n' {
                esc(chars[i], &mut out);
                i += 1;
            }
            out.push_str("</span>");
        } else if c == '"' {
            out.push_str("<span class=\"tk-string\">");
            esc('"', &mut out);
            i += 1;
            while i < chars.len() {
                let ch = chars[i];
                esc(ch, &mut out);
                i += 1;
                // an escape consumes the next char, so a `\"` doesn't close the string
                if ch == '\\' && i < chars.len() {
                    esc(chars[i], &mut out);
                    i += 1;
                    continue;
                }
                if ch == '"' {
                    break;
                }
            }
            out.push_str("</span>");
        } else if matches!(c, '(' | ')' | '[' | ']' | '{' | '}') {
            out.push_str("<span class=\"tk-paren\">");
            esc(c, &mut out);
            out.push_str("</span>");
            i += 1;
        } else if is_delimiter(c) {
            // whitespace, quote/quasiquote/unquote — plain
            esc(c, &mut out);
            i += 1;
        } else {
            let start = i;
            while i < chars.len() && !is_delimiter(chars[i]) {
                i += 1;
            }
            let atom: String = chars[start..i].iter().collect();
            match classify_atom(&atom) {
                Some(cls) => {
                    out.push_str("<span class=\"");
                    out.push_str(cls);
                    out.push_str("\">");
                    for ch in atom.chars() {
                        esc(ch, &mut out);
                    }
                    out.push_str("</span>");
                }
                None => {
                    for ch in atom.chars() {
                        esc(ch, &mut out);
                    }
                }
            }
        }
    }
    out
}

/// Completions for `prefix`: globally-bound names (builtins + prelude) that start with
/// it, newline-separated, capped at 30. Sourced from the runtime's own `reflect/global-names`,
/// computed once and cached — a fresh interpreter's globals never change.
#[wasm_bindgen]
pub fn completions(prefix: &str) -> String {
    if prefix.is_empty() {
        return String::new();
    }
    globals()
        .iter()
        .filter(|n| n.len() > prefix.len() && n.starts_with(prefix))
        .take(30)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- internals --------------------------------------------------------------

static GLOBALS: OnceLock<Vec<String>> = OnceLock::new();

fn globals() -> &'static Vec<String> {
    GLOBALS.get_or_init(|| {
        let mut interp = Interp::new();
        // Load every bundled module first. A bare `Interp` holds only the prelude, so
        // `(reflect/global-names)` returned no `math/…`, `string/…` or `json/…` at all — typing
        // `math/ze` matched nothing, and the completion list could only ever offer core
        // names. The reference documents the whole library, so the playground offering a
        // fraction of it is the site disagreeing with itself.
        let _ = interp
            .eval_str("(doseq (m (reflect/builtin-modules)) (try (require-one m) (catch _ nil)))");
        // Public names only. Raw `(reflect/global-names)` includes private helpers, so the menu
        // offered `map-get`, `macroexpand-loop`, `%map-pairs` and friends — internals that
        // appear in no documentation and that a user has no business calling.
        let query = "(filter (reflect/global-names) (fn (s) (not (reflect/private? s))))";
        let mut names: Vec<String> = match interp.eval_str(query) {
            Ok(value) => {
                // `(reflect/global-names)` prints as a bare list `(a b c …)`; symbols never contain
                // whitespace or parens, so splitting the trimmed body recovers the names.
                let printed = interp.print(value);
                printed
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .split_whitespace()
                    .map(|token| token.to_string())
                    .collect()
            }
            Err(_) => Vec::new(),
        };
        names.sort();
        names.dedup();
        names
    })
}

fn esc(c: char, out: &mut String) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(c),
    }
}

/// A token boundary for the highlighter — matches the reader's atom delimiters closely
/// enough for highlighting: whitespace, the bracket family, and the reader sigils.
fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | '\'' | '`' | ','
        )
}

fn classify_atom(atom: &str) -> Option<&'static str> {
    let first = atom.chars().next()?;
    if first == ':' {
        return Some("tk-keyword");
    }
    if atom == "true" || atom == "false" || atom == "nil" {
        return Some("tk-const");
    }
    let mut cs = atom.chars();
    let is_number = match cs.next() {
        Some(d) if d.is_ascii_digit() => true,
        Some('-') | Some('+') | Some('.') => cs.next().is_some_and(|d| d.is_ascii_digit()),
        _ => false,
    };
    if is_number {
        return Some("tk-number");
    }
    None
}
