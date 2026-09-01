//! `run-process` must not hand a child an inherited stdin (KI-97 item 2).
//!
//! `Command::status()` inherits stdin, stdout and stderr. An inherited stdin is an
//! unbounded, **uncatchable** block on a scheduler worker: a child that reads it waits
//! forever on a terminal nobody is typing at, the scheduler cannot preempt a thread parked
//! in a syscall (ADR-059), and no timeout or `try` in Brood can recover it. The realistic
//! trigger is `git` hitting a credential prompt — and `std/tool/workspace.blsp` runs `git`
//! across sibling repos, so it is reachable from the shipped toolchain.
//!
//! **Why the test has to look like this.** The hazard only exists when the *parent's* stdin
//! is a stream that never reaches EOF. Under an ordinary test harness stdin is already
//! closed or `/dev/null`, so a child reading it gets EOF immediately and the bug is
//! invisible — a test that just ran `run-process` would pass either way and prove nothing.
//! So this spawns `brood` with a **pipe** as stdin and deliberately never writes to or
//! closes it: pre-fix the child inherits that pipe and blocks forever; post-fix it gets
//! `/dev/null` and returns at once. Sabotage-verified by removing the `Stdio::null()`.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generous next to the milliseconds the fixed path needs, and far below the "forever"
/// the broken one takes — the assertion is hang-vs-not, so it does not need to be tight.
const LIMIT: Duration = Duration::from_secs(20);

#[test]
fn run_process_does_not_inherit_a_blocking_stdin() {
    let path = std::env::temp_dir().join("brood-run-process-stdin.blsp");
    // `sh -c 'read line'` blocks until its stdin yields a line or EOF. With stdin at
    // /dev/null that is instant; with the parent's never-closed pipe it is forever.
    std::fs::write(
        &path,
        "(io/puts (str \"CODE \" (os/run-process \"sh\" (list \"-c\" \"read line\"))))\n",
    )
    .expect("write script");

    let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .arg(&path)
        .stdin(Stdio::piped()) // held open below — the whole point
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brood");

    // Hold the write end open and never write: the child's stdin never reaches EOF.
    let pipe = child.stdin.take().expect("stdin pipe");

    let started = Instant::now();
    let finished = loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => break Some(status),
            None if started.elapsed() > LIMIT => break None,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    if finished.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        drop(pipe);
        panic!(
            "run-process handed the child an inherited stdin: it blocked for {LIMIT:?} on a \
             pipe that never closes. On a scheduler worker that is unrecoverable (KI-97)."
        );
    }

    let out = child.wait_with_output().expect("output");
    drop(pipe);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // It ran and reported an exit code; `read` seeing EOF is a non-zero status, which is
    // the point — a diagnosable failure rather than a hang. The code itself is the shell's
    // business, so assert only that we got one.
    assert!(
        stdout.contains("CODE "),
        "expected run-process to return an exit code.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The complement: `/dev/null` on stdin must not disturb a child that ignores stdin, and
/// the exit code must still come back faithfully. Without this, the fix above could have
/// been "always return 0" and the first test would not have noticed.
#[test]
fn run_process_still_reports_a_childs_exit_code() {
    let path = std::env::temp_dir().join("brood-run-process-code.blsp");
    std::fs::write(
        &path,
        "(io/puts (str \"OK \" (os/run-process \"sh\" (list \"-c\" \"exit 7\"))))\n",
    )
    .expect("write script");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1").arg(&path);
    let out = cmd.output().expect("run brood");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OK 7"),
        "exit code did not survive.\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::io::stdout().flush();
}
