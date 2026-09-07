//! A quantum driven on the CALLER's thread must not destroy the caller's context.
//!
//! This is the wasm playground's shape, reproduced on the host. There are no worker threads
//! in a browser: `ProgramExit::wait` drives the run queue on the calling thread, so a
//! quantum runs on top of whatever context that thread already had. `playground::run` calls
//! `begin_stdout_capture()` — which creates that context and puts the capture buffer in it —
//! then runs the snippet, then reads the buffer back.
//!
//! `save_ctx` used to CLEAR `CURRENT` rather than restore it. On a worker thread the two are
//! the same thing, because the thread has no context of its own; on one thread they are not,
//! and the snippet's own quantum threw away the context holding the capture. `take_capture`
//! then found nothing and every `io/puts` vanished from the page (KI-115).
//!
//! Its own test binary: `set_test_no_workers` is process-global, so it must not race the
//! other tests.

use brood::{process, Interp};

#[test]
fn a_quantum_on_the_callers_thread_leaves_the_callers_capture_intact() {
    process::set_test_no_workers(true);

    let mut interp = Interp::new();
    brood::builtins::begin_stdout_capture();

    // Deliberately NOT from a fresh thread. `test_drive_quanta` used to require that, so the
    // per-quantum ctx install would not clobber the caller — which is precisely the property
    // under test, and precisely what wasm cannot do.
    interp
        .spawn_program_for_test("(io/puts \"hello, brood\")\n(+ 1 2 3)")
        .expect("spawn");
    process::test_drive_quanta(64);

    let captured = brood::builtins::take_captured_stdout();
    process::set_test_no_workers(false);

    assert_eq!(
        captured.as_deref(),
        Some("hello, brood\n"),
        "the caller's capture must survive a quantum driven on its own thread"
    );
}
