//! KI-225: an exit signal that kills the **root program process** must end the program.
//!
//! `brood file.blsp` runs the program as one green process (ADR-135) while the OS main
//! thread blocks on its `ProgramExit`. The program published that slot only when its
//! forms finished or raised, so a death by exit signal — a link to a dead pid (`:noproc`),
//! a linked child crashing, `(exit root reason)` — retired the process and left the main
//! thread waiting forever. Every shape below hung before the fix, whatever the root was
//! doing (sleeping, receiving, computing).
//!
//! Each run is bounded: the root is given far more work than the deadline allows (a
//! minute-long sleep, a billion-step loop), so passing proves the signal interrupted it.
//! Every shape runs on every tier: the tree-walker delivers the kill as an unwinding
//! error rather than a VM outcome, and must end the program the same way.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generous for a debug binary under a loaded machine; the work each script would do
/// without the signal is far longer.
const DEADLINE: Duration = Duration::from_secs(30);

const TIERS: [&str; 3] = ["0", "1", "2"];

struct Run {
    /// `name@tier`, for failure messages.
    label: String,
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Run `source` with `brood` once per tier.
fn run_on_every_tier(name: &str, source: &str) -> Vec<Run> {
    let path = std::env::temp_dir().join(format!(
        "brood-root-exit-{}-{name}.blsp",
        std::process::id()
    ));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    drop(file);
    let runs = TIERS
        .iter()
        .map(|tier| run_once(&format!("{name}@tier{tier}"), tier, &path))
        .collect();
    let _ = std::fs::remove_file(&path);
    runs
}

fn run_once(label: &str, tier: &str, path: &std::path::Path) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_TIER", tier)
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run brood");
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll brood") {
            break status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{label}: the program hung — the root never heard it was killed");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = child.wait_with_output().expect("collect output");
    Run {
        label: label.to_string(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: status.code(),
    }
}

/// The program died of `reason`: exit 1, the error names the signal, and nothing after
/// the kill point ran.
fn assert_killed(runs: &[Run], reason: &str) {
    for run in runs {
        let label = &run.label;
        assert_eq!(
            run.code,
            Some(1),
            "{label}: expected exit 1\nstdout: {}\nstderr: {}",
            run.stdout,
            run.stderr
        );
        let expected = format!("killed by an exit signal: {reason}");
        assert!(
            run.stderr.contains(&expected),
            "{label}: stderr lacks `{expected}`:\n{}",
            run.stderr
        );
        assert!(
            run.stdout.contains("before") && !run.stdout.contains("after"),
            "{label}: the program must run up to the signal and no further:\n{}",
            run.stdout
        );
    }
}

/// The program ran to completion: exit 0, exactly `stdout`, no kill reported.
fn assert_clean(runs: &[Run], stdout: &str) {
    for run in runs {
        let label = &run.label;
        assert_eq!(run.code, Some(0), "{label}: stderr: {}", run.stderr);
        assert_eq!(run.stdout, stdout, "{label}");
        assert!(!run.stderr.contains("killed by"), "{label}: {}", run.stderr);
    }
}

#[test]
fn a_link_to_a_dead_pid_kills_a_sleeping_root() {
    let runs = run_on_every_tier(
        "sleep",
        r#"(let (dead (spawn :done)
      watch (monitor dead))
  (receive ([:down ^watch _ _] nil))
  (io/puts "before")
  (link dead)
  (proc/sleep 60000)
  (io/puts "after"))
(io/puts "after the form")"#,
    );
    assert_killed(&runs, ":noproc");
}

// The next two need the signal to land while the root is INSIDE the shape, so it comes
// from a linked child that exits a while after the root says `:go` — not from a link to a
// dead pid, whose kill may already land at `link` itself. The child waits for `:go` rather
// than sleeping from its start: a tree-walker booting from source can take longer than any
// fixed sleep to reach its first `io/puts` (it loads `io` there), and a kill landing first
// left "before" unprinted.

#[test]
fn a_linked_exit_kills_a_root_in_receive_after() {
    let runs = run_on_every_tier(
        "receive",
        r#"(let (child (spawn-link (receive (:go (proc/sleep 200) (exit (self) :boom)))))
  (io/puts "before")
  (send child :go)
  (receive (:never nil) (after 60000 :timeout)))
(io/puts "after")"#,
    );
    assert_killed(&runs, ":boom");
}

#[test]
fn a_linked_exit_kills_a_computing_root() {
    let runs = run_on_every_tier(
        "compute",
        r#"(let (child (spawn-link (receive (:go (proc/sleep 200) (exit (self) :boom)))))
  (io/puts "before")
  (send child :go)
  (doseq (i (range 1000000000)) i))
(io/puts "after")"#,
    );
    assert_killed(&runs, ":boom");
}

#[test]
fn an_exit_from_a_child_kills_the_root() {
    let runs = run_on_every_tier(
        "exit",
        r#"(let (root (self))
  (io/puts "before")
  (spawn (exit root :boom))
  (proc/sleep 60000)
  (io/puts "after"))"#,
    );
    assert_killed(&runs, ":boom");
}

#[test]
fn a_linked_child_crashing_kills_the_root() {
    let runs = run_on_every_tier(
        "crash",
        r#"(io/puts "before")
(spawn-link (do (proc/sleep 50) (error "child crash")))
(proc/sleep 60000)
(io/puts "after")"#,
    );
    assert_killed(&runs, "[:error …]");
}

/// Trapping turns the same link signal into a message, and the program goes on.
#[test]
fn a_trapping_root_receives_the_exit_as_a_message() {
    let runs = run_on_every_tier(
        "trap",
        r#"(proc/trap-exit true)
(let (dead (spawn :done)
      watch (monitor dead))
  (receive ([:down ^watch _ _] nil))
  (link dead)
  (receive
    ([:EXIT ^dead reason] (io/puts (str "trapped " reason)))
    (after 10000 (io/puts "no exit message"))))
(io/puts "end")"#,
    );
    assert_clean(&runs, "trapped :noproc\nend\n");
}

/// A linked child that finishes normally does not touch a non-trapping root.
#[test]
fn a_normal_exit_of_a_linked_child_leaves_the_root_running() {
    let runs = run_on_every_tier(
        "normal-link",
        r#"(let (child (spawn-link (receive (:go :done)))
      watch (monitor child))
  (send child :go)
  (receive ([:down ^watch _ _] nil)))
(io/puts "end")"#,
    );
    assert_clean(&runs, "end\n");
}

/// `(exit root :normal)` stops the root as it stops any process — with the reason
/// `:normal`, which ends the program cleanly: exit 0, no error.
#[test]
fn an_exit_normal_ends_the_program_cleanly() {
    let runs = run_on_every_tier(
        "exit-normal",
        r#"(let (root (self))
  (io/puts "before")
  (spawn (exit root :normal))
  (proc/sleep 60000)
  (io/puts "after"))"#,
    );
    assert_clean(&runs, "before\n");
}

/// Not the root, but the same gap: a hard kill must stop a spawned process inside a
/// JIT'd loop, whose back-edge poll preempts without passing the VM's kill safepoint.
#[test]
fn a_hard_kill_stops_a_computing_child() {
    let runs = run_on_every_tier(
        "kill-child",
        r#"(let (child (spawn (do (doseq (i (range 1000000000)) i) (io/puts "child finished")))
      ref (monitor child))
  (proc/sleep 200)
  (exit child :kill)
  (receive
    ([:down ^ref _ reason] (io/puts (str "down " reason)))
    (after 20000 (io/puts "never died"))))"#,
    );
    assert_clean(&runs, "down :killed\n");
}
