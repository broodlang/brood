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

/// The program keeps scheduling work long past the injected fault: many short-lived
/// processes, each reporting back, then a second wave *after* the first has been
/// collected. Every wave is monitored with a bounded `receive`, so a lost process shows
/// up as a missing reply rather than a hang, and the final line only prints if the pool
/// was still scheduling at the end.
const WORKLOAD: &str = r#"
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

;; The first wave straddles the injected fault; the later ones prove the pool still
;; schedules afterwards. The run has to OUTLIVE the panicking worker's unwind — the
;; panic hook symbolizes a backtrace and appends a crash dump, which takes far longer
;; than these processes do, and an exit before it finishes would race the very recovery
;; this is meant to observe.
(io/puts (str "WAVE1 " (wave 400)))
(sleep 500)
(io/puts (str "WAVE2 " (wave 400)))
(io/puts (str "WAVE3 " (wave 400)))
(io/puts "ALIVE")
"#;

/// The control: with no fault, both waves complete in full. Pins the workload itself as
/// sound, so a failure in the injected run below cannot be blamed on a flaky program.
#[test]
fn the_workload_completes_with_no_fault_injected() {
    let path = script("control", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, None);
    assert!(
        ok,
        "control run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("WAVE1 400")
            && stdout.contains("WAVE2 400")
            && stdout.contains("WAVE3 400")
            && stdout.contains("ALIVE"),
        "control run did not complete both waves.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// The guard: a panic injected into the quantum tail costs *at most* the one process
/// whose quantum it struck. The worker survives, the process is retired (loudly, and
/// reported as lost rather than vanishing), and the runtime keeps scheduling — the
/// second wave completes in full and the program exits cleanly.
///
/// Pre-fix this run hangs until it is killed: the worker thread is gone and the process
/// it was carrying was destroyed without ever being deregistered.
#[test]
fn a_quantum_tail_panic_costs_one_process_not_the_runtime() {
    let path = script("fault", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, Some(50));

    assert!(
        stderr.contains("the scheduler's post-quantum tail panicked"),
        "the injected fault did not reach the tail's recovery path — the test is not \
         exercising what it claims.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The runtime survived the panic: it kept scheduling to the very end of the program.
    assert!(
        stdout.contains("ALIVE"),
        "the runtime did not survive a quantum-tail panic.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // And the damage is bounded to the single struck process — a wave started *after*
    // the fault loses nothing at all.
    assert!(
        stdout.contains("WAVE2 400") && stdout.contains("WAVE3 400"),
        "a wave scheduled after the fault came back short — the pool did not fully \
         recover.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(ok, "exit status.\nstdout:\n{stdout}\nstderr:\n{stderr}");
}
