//! `read-line` parks the calling **process**, never a scheduler **worker** (ADR-059 Phase 2).
//!
//! Before this, `read-line` was a Rust builtin that took the global stdin lock on whichever
//! worker the calling process happened to be on. A process waiting for a line that never
//! came — an interactive terminal nobody typed into, a pipe the parent never wrote — pinned
//! that worker for good, and as many such processes as there are workers pinned the whole
//! pool: every other process starved, with nothing wrong on their side and no diagnostic
//! (the stranded-work watchdog, KI-88's tool, would now at least name it). This was the last
//! open item of KI-97's "untimed blocking calls on scheduler workers".
//!
//! Now the read happens on the kernel's `brood-stdin` reader thread, which delivers the line
//! to the caller's mailbox; `read-line` is a prelude function parking in a selective receive.
//!
//! **The assertion is causal, not timed.** The workload spawns far more `read-line` callers
//! than any machine has workers, on a stdin pipe that is held open and never written, and
//! THEN runs a wave of ordinary spawned work that the root process collects. Pre-fix the wave
//! cannot run — every worker is inside a blocking read. Post-fix the readers are parked
//! processes, the pool is idle, and the wave completes. The stdin pipe is dropped only after
//! the program has printed, so the readers never see EOF while the wave runs.
//!
//! Sabotage-verified: making `%read-line-start` perform the read synchronously on the caller
//! (the old shape) fails the wave test while the delivery test still passes. It fails by
//! **hanging to nextest's 2-minute cap**, not by printing `WAVE 0`: the root process's own
//! resume from `(sleep 300)` needs a worker too, so a pinned pool freezes the whole program,
//! which is exactly the hazard.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-readline-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

/// 256 processes each block in `read-line` on a pipe nobody writes; then a wave of 200
/// ordinary spawns must still run and be collected. `me` is bound OUTSIDE the spawn: `(self)`
/// inside the spawned form would evaluate in the child.
const WAVE: &str = r#"
(defn reader () (read-line))
(dotimes (i 256) (spawn (reader)))

(defn worker (parent i) (send parent [:done i]))
(defn collect (n got)
  (if (= n 0)
    got
    (receive
      ([:done _] (collect (- n 1) (+ got 1)))
      (after 20000 got))))
(defn wave (n)
  (let (me (self))
    (do (dotimes (i n) (spawn (worker me i)))
        (collect n 0))))

;; Let the readers get scheduled (and, pre-fix, pin the pool) before the wave is queued.
(sleep 300)
(io/puts (str "WAVE " (wave 200)))
"#;

#[test]
fn processes_blocked_in_read_line_do_not_pin_the_worker_pool() {
    let path = script("wave", WAVE);
    let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(&path)
        .stdin(Stdio::piped()) // held open below and never written — the whole point
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brood");
    let stdin = child.stdin.take().expect("stdin pipe");

    // Read stdout to completion while the pipe stays open. The program exits on its own
    // after printing (parked readers do not keep a runtime alive — they die with it), so
    // this returns. Pre-fix nothing is ever printed and nextest's 2-minute cap ends it.
    let mut out = String::new();
    child
        .stdout
        .take()
        .expect("stdout pipe")
        .read_to_string(&mut out)
        .expect("read stdout");
    let mut err = String::new();
    child
        .stderr
        .take()
        .expect("stderr pipe")
        .read_to_string(&mut err)
        .expect("read stderr");
    drop(stdin);
    let status = child.wait().expect("wait brood");

    assert!(
        status.success(),
        "brood failed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("WAVE 200"),
        "the wave did not complete while 256 processes waited in read-line — the pool \
         was pinned.\nstdout:\n{out}\nstderr:\n{err}"
    );
}

/// The other half: lines are actually delivered, in order, to root AND spawned callers,
/// and EOF is `nil` for every later call. Pins the plumbing so the wave test cannot pass
/// by `read-line` having stopped reading anything.
const DELIVERY: &str = r#"
(io/puts (str "root=" (read-line)))
(let (me (self))
  (spawn (send me [:got (read-line) (read-line)]))
  (receive
    ([:got a b] (io/puts (str "child=" a "," b)))
    (after 10000 (io/puts "child TIMEOUT"))))
(io/puts (str "eof=" (pr-str (read-line)) "," (pr-str (read-line))))
"#;

#[test]
fn lines_are_delivered_in_order_to_root_and_spawned_callers_then_nil_at_eof() {
    let path = script("delivery", DELIVERY);
    let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brood");
    {
        let mut stdin = child.stdin.take().expect("stdin pipe");
        stdin
            .write_all(b"alpha\nbeta\ngamma\n")
            .expect("write stdin");
        // Dropping the pipe closes it: the two trailing reads must see EOF.
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let output = loop {
        if child.try_wait().expect("try_wait").is_some() {
            break child.wait_with_output().expect("output");
        }
        assert!(std::time::Instant::now() < deadline, "brood hung");
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "brood failed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(out.contains("root=alpha"), "stdout:\n{out}\nstderr:\n{err}");
    assert!(
        out.contains("child=beta,gamma"),
        "stdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("eof=nil,nil"),
        "stdout:\n{out}\nstderr:\n{err}"
    );
}
