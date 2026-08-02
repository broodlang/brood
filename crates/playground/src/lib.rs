//! The in-browser Brood playground: a wasm-bindgen shim exposing `eval(src)` to JS.
//!
//! Each call builds a fresh interpreter (a clean image per run — no state leaks
//! between snippets), captures anything the snippet prints, evaluates it, and
//! returns the captured output followed by the value of the last form (or an error
//! report). Evaluation runs on the single wasm thread; snippets that `spawn` green
//! processes or touch the network/filesystem will error (those subsystems are
//! stubbed on wasm — see `crates/lisp` wasm gating), which is fine for a language
//! playground.

use brood::Interp;
use wasm_bindgen::prelude::*;

/// Evaluate Brood `source` and return its output as text: captured stdout (from
/// `print`/`println`) followed by the printed value of the last form, or an error
/// report if a form raised or failed to parse.
#[wasm_bindgen]
pub fn run(source: &str) -> String {
    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();
    let result = interp.eval_source(source);
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    match result {
        Ok(value) => {
            let printed = interp.print(value);
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
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
