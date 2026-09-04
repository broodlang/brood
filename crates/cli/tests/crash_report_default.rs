//! A crash in an unsupervised process still prints a report through the LAZY arm.
//!
//! ADR-313 moved the arm out of `crash-report` and into the prelude, so the module is
//! loaded by the first crash rather than by every run (9.0 ms of ~24 ms — see
//! `crates/lisp/tests/crash_report_lazy.rs`). That test proves the module stays unloaded;
//! this one proves the reporting still WORKS once it is, which is the half a laziness
//! optimization breaks. It runs the real `brood file` entry point — the one a user
//! reaches — rather than calling `take-over` directly, because the handoff from the
//! prelude shim to the module is exactly the seam under test.

use std::io::Write;
use std::process::Command;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-crashrep-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

/// Run `path` and return its stderr **up to the moment `marker` appears**, or everything it
/// wrote if it exits first. A deadline, not a window.
///
/// The first version of this test slept a fixed 500 ms in the SCRIPT and read stderr after
/// exit. The lazy arm's first crash loads the reporter's nine modules before it prints
/// anything, and on 2026-09-04 one run under a full-suite load produced an entirely empty
/// stderr: the root exited before the report landed, 5/5 green in isolation at 0.53 s. That
/// is KI-79's class — a wall clock standing in for synchronisation — and the fix is the same
/// one: wait on the condition with a generous deadline. Nothing in-language can observe
/// "report printed" (the shim already holds the `:crash-reporter` name from arm time), so the
/// HARNESS watches the pipe instead, and kills the child once the marker is there. Healthy
/// runs still finish in ~0.6 s; a loaded box gets up to `deadline` rather than a coin flip.
fn stderr_until(path: &std::path::Path, marker: &str, deadline: std::time::Duration) -> String {
    use std::io::Read;
    let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env_remove("BROOD_NO_CRASH_REPORT")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn brood");
    let mut pipe = child.stderr.take().expect("stderr pipe");
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let start = std::time::Instant::now();
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(chunk) => seen.extend_from_slice(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // child closed stderr
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        let text = String::from_utf8_lossy(&seen);
        // Wait for the reason line too, not only the header, so a report is read whole.
        if text.contains(marker) && text.contains("division by zero") {
            break;
        }
        if start.elapsed() > deadline {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    String::from_utf8_lossy(&seen).into_owned()
}

#[test]
fn an_unsupervised_crash_is_reported_through_the_lazy_arm() {
    // The spawned process is unsupervised and unmonitored: nothing but the default
    // reporter can say anything about it. The root sleeps so the report lands before exit.
    // The script parks for longer than any sane load needs; the harness stops it the moment
    // the report is on the pipe (see `stderr_until`), so this is a ceiling, not a cost.
    let path = script(
        "boom",
        "(spawn (fn () (/ 1 0)))\n(sleep 15000)\n(io/puts \"done\")\n",
    );
    let err = stderr_until(&path, "[crash]", std::time::Duration::from_secs(15));
    assert!(
        err.contains("[crash]"),
        "no crash report for an unsupervised crash — the lazy arm subscribed but never \
         handed off to `crash-report/take-over` (ADR-313).\nstderr was:\n{err}"
    );
    assert!(
        err.contains("division by zero"),
        "the report reached stderr but without the reason.\nstderr was:\n{err}"
    );
}

#[test]
fn the_opt_out_still_suppresses_the_report() {
    let path = script(
        "quiet",
        "(spawn (fn () (/ 1 0)))\n(sleep 500)\n(io/puts \"done\")\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(&path)
        .output()
        .expect("run brood");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !err.contains("[crash]"),
        "BROOD_NO_CRASH_REPORT=1 still produced a crash report.\nstderr was:\n{err}"
    );
}
