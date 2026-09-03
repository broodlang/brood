//! A ROOT program inherits its caller's stdout capture.
//!
//! The in-browser playground calls `begin_stdout_capture()` and then runs the snippet
//! through `run_program_repr` → `spawn_root_program`, so the snippet executes as a green
//! process. `spawn` inherits the spawner's capture stack; the root-program path built its
//! process with `capture: Vec::new()`, so it inherited nothing and every `io/puts` went to
//! the real stdout. In a browser that is nowhere: `(io/puts "hello, brood")` printed
//! nothing on the page and only the last form's value appeared.
//!
//! Asserted through the same public entry points the playground uses, so a future change to
//! how the snippet is run has to keep the property rather than the implementation.

use brood::Interp;

#[test]
fn a_root_program_inherits_the_callers_stdout_capture() {
    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();
    let ran = interp.run_program("(io/puts \"hello, brood\")\n(+ 1 2 3)", None);
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    assert!(ran.is_ok(), "the program should run: {ran:?}");
    assert_eq!(
        captured, "hello, brood\n",
        "a root program's output must land in the caller's capture — this is what the \
         playground shows on the page"
    );
}

/// The same snippet on the non-wasm playground path, which was never broken — kept beside
/// it so the two readings are asserted to agree rather than assumed to.
#[test]
fn the_direct_eval_path_captures_the_same_text() {
    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();
    let value = interp.eval_source("(io/puts \"hello, brood\")\n(+ 1 2 3)");
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    assert!(value.is_ok(), "{value:?}");
    assert_eq!(captured, "hello, brood\n");
}
