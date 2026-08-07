//! KI-29 — a `brood` child spawned by a test must not survive the test.
//!
//! The bug this guards was not a failure of any kind: leaked children left no red test
//! behind, so it sat for nine days while a target node kept listening and burning ~4% of a
//! core. There is no way to observe "nothing leaked" from inside a passing test, so instead
//! each test here drives **one** of the two cleanup nets in [`support::BroodChild`] with the
//! other one defeated, and asserts the child actually died.
//!
//! - [`the_drop_guard_kills_a_running_brood_child_ki29`] — spawn on the test's own thread, so
//!   the parent-death signal cannot have fired yet (it fires when that thread exits, which is
//!   after the test body). Only `Drop` can have killed it.
//! - [`a_brood_child_dies_when_its_spawning_thread_exits_ki29`] — `forget` the guard, so `Drop`
//!   never runs. Only `PR_SET_PDEATHSIG` can have killed it.
//!
//! Both are Linux-only: they read process state from `/proc`, and the parent-death signal is a
//! Linux `prctl`.

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

mod support;
use support::*;

/// A program that becomes a running `brood` runtime and then parks forever — so if it is
/// found dead, something killed it. Announces itself by writing `marker`, which lets a test
/// wait for a *fully booted* child rather than racing one still inside `exec`.
fn parked_src(marker: &std::path::Path) -> String {
    format!(
        "(spit \"{}\" \"up\")\n(defn park () (receive (_ (park))))\n(park)\n",
        marker.display()
    )
}

/// Is `pid` still a live process? A zombie counts as dead — it has exited and is only waiting
/// to be reaped, which is exactly the state a killed-but-unreaped child is left in.
///
/// Parsed off the end of `comm` rather than by splitting on whitespace: `comm` is
/// parenthesised and may itself contain spaces, so field 3 is only findable after the last
/// `)`.
fn alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false; // reaped and gone
    };
    match stat.rsplit_once(')') {
        Some((_, rest)) => rest.split_whitespace().next() != Some("Z"),
        None => false,
    }
}

/// Poll until `pid` is dead, or panic after `secs`.
fn assert_dies_within(pid: u32, secs: u64, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while alive(pid) {
        if Instant::now() >= deadline {
            let cmd = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            panic!(
                "{what}: brood child pid {pid} was still alive after {secs}s \
                 — this is the KI-29 leak.\ncmdline: {}",
                cmd.replace('\0', " ").trim()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait for the child to write its marker, so the test acts on a booted runtime.
fn wait_until_up(marker: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "child never wrote its marker at {}{}",
            marker.display(),
            // KI-38: this fires at 30 s against a boot measured at 151 ms idle / 4 s worst
            // case under a full suite, so the report says which mode it was.
            stall_report("wait_until_up gave up after 30 s")
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Net 1: an early panic or return drops the guard, and the guard kills the child.
#[test]
fn the_drop_guard_kills_a_running_brood_child_ki29() {
    let dir = std::env::temp_dir().join(format!("brood-ki29-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("up");

    let child = spawn_brood(&dir, "park.blsp", &parked_src(&marker));
    let pid = child.id();
    wait_until_up(&marker);
    assert!(alive(pid), "the child should be parked, not finished");

    drop(child); // what a panicking test body does implicitly

    assert_dies_within(pid, 10, "the Drop guard did not kill the child");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Net 2: the guard never runs (the binary was killed), and the kernel does it instead.
///
/// `mem::forget` stands in for "no destructor ran" — the same end state as a SIGKILLed test
/// binary, but deterministic and without this test having to kill itself.
#[test]
fn a_brood_child_dies_when_its_spawning_thread_exits_ki29() {
    let dir = std::env::temp_dir().join(format!("brood-ki29-pdeath-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("up");

    let spawn_dir = dir.clone();
    let spawn_marker = marker.clone();
    let pid = std::thread::spawn(move || {
        let child = spawn_brood(&spawn_dir, "park.blsp", &parked_src(&spawn_marker));
        let pid = child.id();
        wait_until_up(&spawn_marker);
        assert!(alive(pid), "the child should be parked, not finished");
        std::mem::forget(child); // defeat net 1: nothing will kill this but the kernel
        pid
    })
    .join()
    .expect("spawner thread");

    assert_dies_within(pid, 10, "PR_SET_PDEATHSIG did not kill the child");

    // Nobody reaped it (that was the point), so it is a zombie. Reap it here rather than
    // leave the litter this test exists to prevent.
    // SAFETY: `pid` is our own child and has already exited, so this cannot block.
    unsafe { libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0) };
    let _ = std::fs::remove_dir_all(&dir);
}
