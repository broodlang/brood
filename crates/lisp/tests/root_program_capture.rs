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

/// The playground calls `run_program_repr`, not `run_program`. The two tests above assert the
/// NEIGHBOUR of the shipped path; this one asserts the path itself.
#[test]
fn the_repr_path_the_playground_actually_calls_captures_too() {
    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();
    let ran = interp.run_program_repr("(io/puts \"hello, brood\")\n(+ 1 2 3)");
    let captured = brood::builtins::take_captured_stdout().unwrap_or_default();
    assert!(ran.is_ok(), "the program should run: {ran:?}");
    assert_eq!(ran.unwrap(), "6", "the last form's printed value");
    assert_eq!(
        captured, "hello, brood\n",
        "run_program_repr is what `playground::run` calls on wasm"
    );
}

/// `run_program` must NOT render each top-level form's value — and this asserts the
/// observable consequence rather than the flag, so a regression that sets `want_result`
/// without honouring it still fails.
///
/// The cost is invisible until a program's top level binds something big: ungating the
/// result path (7a72135b) rendered EVERY form's value to a string on the native path,
/// where `run_program` throws it away, and the `sort` benchmark — `(def data (sort …))`
/// over a 375k-element list — paid **+10.6%** for it (132ms -> 146ms, interleaved against
/// a 0.7% control). A `Some` here means that work is back.
#[test]
fn run_program_does_not_render_a_result_nobody_reads() {
    let interp = Interp::new();
    let exit = brood::process::spawn_root_program(
        &interp.heap,
        "(def data (list 1 2 3))\n(+ 1 2 3)",
        None,
        None,
        false,
    )
    .expect("spawn");
    exit.wait().expect("the program should run");
    assert_eq!(
        exit.take_result(),
        None,
        "run_program discards the value, so the driver must not have rendered one"
    );
}

/// The other side of the same rule: when the caller DOES want the result, it is there.
/// Without this, the test above passes trivially if rendering were removed altogether.
#[test]
fn run_program_repr_still_renders_the_result() {
    let interp = Interp::new();
    let exit = brood::process::spawn_root_program(
        &interp.heap,
        "(def data (list 1 2 3))\n(+ 1 2 3)",
        None,
        None,
        true,
    )
    .expect("spawn");
    exit.wait().expect("the program should run");
    assert_eq!(exit.take_result().as_deref(), Some("6"));
}
