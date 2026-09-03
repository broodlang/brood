//! The scheduler's **stranded-work watchdog** must actually fire.
//!
//! KI-88's signature is a process that is created, promoted, registered — and never
//! scheduled: no death line, no trace, and thirty seconds later a collector times out with
//! the evidence gone. Every sighting so far happened in a run nobody had instrumented, so
//! the watchdog in `scheduler/pool.rs` is default-ON: when `STEALABLE` says work is queued
//! but no worker has found anything to run for three seconds, it prints one report naming
//! every queued pid and the state of every worker, then latches until progress resumes.
//!
//! A detector nobody has ever seen fire is indistinguishable from one that cannot, so
//! `BROOD_FAULT_STRANDED=1` over-counts `STEALABLE` by one at pool start — exactly what a
//! stranded process looks like from the probe's side — and this test asserts the report
//! appears within a program that parks for longer than the window. The control asserts it
//! does NOT appear in the same program without the fault, because a watchdog that reports on
//! a healthy idle pool is noise that trains everyone to ignore the real one.
//!
//! Sabotage-verified: removing the `stranded_probe()` call from the park path fails the
//! fault run (no report) while the control still passes.

use std::io::Write;
use std::process::Command;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-stranded-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

fn run(path: &std::path::Path, fault: bool) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1");
    if fault {
        cmd.env("BROOD_FAULT_STRANDED", "1");
    }
    let output = cmd.arg(path).output().expect("run brood");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

/// The main process parks for longer than the 3 s window while the pool sits idle. A
/// healthy idle pool has `STEALABLE == 0`, so the probe resets every cycle; with the
/// over-count in place the window opens on the first parked cycle and expires while the
/// program is still asleep.
const WORKLOAD: &str = r#"
(sleep 4500)
(io/puts "AWAKE")
"#;

const REPORT: &str = "[sched] STRANDED WORK (KI-88 signature)";

#[test]
fn an_idle_pool_with_nothing_queued_is_not_reported() {
    let path = script("control", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, false);
    assert!(
        ok,
        "control run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("AWAKE"),
        "control run did not finish.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(REPORT),
        "the watchdog reported on a HEALTHY idle pool — that is noise, and noise trains \
         everyone to ignore the real report.\nstderr:\n{stderr}"
    );
}

#[test]
fn queued_work_no_worker_can_find_is_reported_within_the_window() {
    let path = script("fault", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, true);
    assert!(
        ok,
        "fault run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Sanity: the fault must actually have been injected, or the test proves nothing.
    assert!(
        stderr.contains("BROOD_FAULT_STRANDED"),
        "the fault never fired — the test is not exercising what it claims.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains(REPORT),
        "the pool sat for 4.5 s believing a process was queued and never said so — the \
         watchdog did not fire.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The report carries the per-worker state — the fact every KI-88 sighting lacked.
    assert!(
        stderr.contains("w0: parked="),
        "the report did not name the workers' queues.\nstderr:\n{stderr}"
    );
    // Latched: one report per starvation episode, not one per parked cycle.
    assert_eq!(
        stderr.matches(REPORT).count(),
        1,
        "the report should latch after firing once.\nstderr:\n{stderr}"
    );
}
