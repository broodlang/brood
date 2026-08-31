//! A panic in the scheduler's **post-quantum tail** must not take the runtime with it.
//!
//! `run_one` wraps `drive()` in `catch_unwind`, but everything after it — `save_ctx`,
//! `finish_quantum`, and the outcome routing (`store_resume`/`park_on_receive`/
//! `deregister`/`enqueue`) — used to run unprotected, and `worker_loop` has no catch of
//! its own. A panic there (a bad handle, an OOB slab index, a broken invariant) therefore
//! did two silent, unrecoverable things at once: it killed that worker thread for good
//! (the pool shrank permanently, and nothing restarts a worker), and the unwind dropped
//! the `Box<Process>` on its way out, so the process vanished with **no `deregister`** —
//! no death line, no monitors fired, no `[:down …]`. Anything waiting on that process
//! waited forever, so one tail panic hung the whole runtime.
//!
//! That is also, exactly, KI-88's recorded signature (a `run` with no `end`, a ledger
//! entry no thread is inside, no death line, a collector timing out). KI-88 is dormant
//! and was never caught with a panic on stderr, so this is not a diagnosis of it — it
//! closes the mechanism, so a future sighting is known not to be this.
//!
//! `BROOD_FAULT_QUANTUM_TAIL=<n>` injects the panic on the nth quantum, because nothing
//! an ordinary program can do provokes one on demand.
//!
//! **These assertions are causal, not timed.** An earlier draft watched for the recovery's
//! own stderr line while the program did unrelated work, and that is a race the test loses
//! under load: the panicking worker is still symbolizing a backtrace (the hook also writes
//! `.brood_crash_dump`) when a short program exits, so the line never lands. It passed solo
//! and failed inside a full suite run. The workload below instead **monitors** the process
//! whose quantum is struck and blocks on its `[:down …]`, so the program cannot reach its
//! own last line until the retire has actually happened — the very thing under test orders
//! the output, and no sleep is involved.

use std::io::Write;
use std::process::Command;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-qtail-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

fn run(path: &std::path::Path, fault_at: Option<u32>) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1");
    if let Some(n) = fault_at {
        cmd.env("BROOD_FAULT_QUANTUM_TAIL", n.to_string());
    }
    let output = cmd.arg(path).output().expect("run brood");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

/// One long-running process that is preempted many times, monitored by a main process
/// that then parks. Because main is parked it consumes almost no quanta, so an injected
/// fault lands on the spinner — and main is released only by the spinner's `[:down …]`,
/// which only a real retire can send.
///
/// The trailing wave proves the pool still schedules afterwards: pre-fix, the worker
/// thread that took the panic is gone for good.
const WORKLOAD: &str = r#"
(defn spin (n) (if (= n 0) :done (spin (- n 1))))

(defn worker (parent i) (send parent [:done i]))

(defn collect (n got)
  (if (= n 0)
    got
    (receive
      ([:done _] (collect (- n 1) (+ got 1)))
      (after 10000 got))))

;; `me` is bound OUTSIDE the spawn: `(self)` inside the spawned form would evaluate in
;; the child, and every worker would message itself.
(defn wave (n)
  (let (me (self))
    (do (dotimes (i n) (spawn (worker me i)))
        (collect n 0))))

(def w (spawn (spin 40000000)))
(def m (monitor w))
;; Blocks until the spinner is retired. Pre-fix it never is — it vanishes mid-unwind with
;; no deregister — so this times out and reports NO-DOWN.
(receive
  ([:down mref _ reason] (io/puts (str "DOWN " reason)))
  (after 30000 (io/puts "NO-DOWN")))
(io/puts (str "WAVE " (wave 200)))
(io/puts "ALIVE")
"#;

/// The control: with no fault the spinner finishes normally, so the monitor fires
/// `:normal`, the wave completes, and the program exits cleanly. Pins the workload itself
/// as sound, so a failure in the injected run cannot be blamed on a flaky program.
#[test]
fn the_workload_completes_with_no_fault_injected() {
    let path = script("control", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, None);
    assert!(
        ok,
        "control run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("DOWN :normal"),
        "the spinner should have exited normally.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("WAVE 200") && stdout.contains("ALIVE"),
        "control run did not finish.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The guard: a panic injected into the quantum tail costs *at most* the one process whose
/// quantum it struck, and that process is properly retired rather than silently destroyed
/// — its monitor still fires. The worker thread survives to keep scheduling.
///
/// Pre-fix, the spinner vanishes with no `deregister`: no `[:down …]` ever arrives, the
/// `receive` burns its full 30 s and reports `NO-DOWN`.
#[test]
fn a_quantum_tail_panic_retires_the_process_and_spares_the_runtime() {
    let path = script("fault", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, Some(20));

    // Sanity: the fault must actually have been injected, or the test proves nothing.
    assert!(
        stderr.contains("BROOD_FAULT_QUANTUM_TAIL"),
        "the fault never fired — the test is not exercising what it claims.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The substantive guarantee: the struck process was RETIRED, not dropped on the floor.
    // Causal — the DOWN can only come from the recovery path's `deregister`.
    assert!(
        stdout.contains("DOWN :killed"),
        "the struck process vanished without firing its monitor.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("NO-DOWN"),
        "the monitor never fired for the struck process.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // And the pool kept scheduling: work queued after the fault completes in full.
    assert!(
        stdout.contains("WAVE 200") && stdout.contains("ALIVE"),
        "the runtime did not survive a quantum-tail panic.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(ok, "exit status.\nstdout:\n{stdout}\nstderr:\n{stderr}");
}
