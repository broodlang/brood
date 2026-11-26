//! The timer thread's **overdue-receive watchdog** must actually fire, and say which link of
//! the wake broke.
//!
//! KI-413. The mur forge (2026-11-19, Brood 1383f37): its Postgres pool and six connection
//! processes (and its ticker), parked in `receive` (all but the pool with `after`), stopped
//! running at once and never ran again, for a week, while the rest of the runtime served
//! requests. Nothing on stderr said
//! whether their timers were lost, never fired, fired into a run queue nobody drained, or
//! whether the processes themselves had been lost. Every two seconds the timer thread now
//! names any green receive still waiting five seconds past its deadline, with the verdict.
//!
//! `BROOD_FAULT_DROP_TIMER=1000` makes `arm_timer` drop any deadline due within a second — a
//! lost timer — so the child below parks for good; the report must name it as such. The
//! control runs the same program
//! without the fault and must stay silent: the child's timeout fires on time.
//!
//! Sabotage-verified: removing the `report_overdue_receives` call from `timer_loop` fails
//! the fault run (no report) while the control still passes.

use std::process::Command;

mod support;
use support::script;

fn run(path: &std::path::Path, fault: bool) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1");
    if fault {
        cmd.env("BROOD_FAULT_DROP_TIMER", "1000");
    }
    let output = cmd.arg(path).output().expect("run brood");
    (
        String::from_utf8_lossy_owned(output.stdout),
        String::from_utf8_lossy_owned(output.stderr),
        output.status.success(),
    )
}

/// A child parks on a 100 ms timeout; the root (which blocks on its own condvar, not the
/// timer heap) outlives the report window and finishes either way.
const WORKLOAD: &str = r#"
(spawn (do (receive (after 100 nil)) (io/puts "CHILD-WOKE")))
(proc/sleep 8500)
(io/puts "AWAKE")
"#;

const REPORT: &str = "[timer] OVERDUE RECEIVE";

#[test]
fn a_receive_whose_timeout_fires_is_not_reported() {
    let path = script("overdue-control", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, false);
    assert!(
        ok,
        "control run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("CHILD-WOKE") && stdout.contains("AWAKE"),
        "control run did not finish.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(REPORT),
        "the watchdog reported a receive whose timeout fired — noise.\nstderr:\n{stderr}"
    );
}

#[test]
fn a_receive_parked_past_its_deadline_is_named_with_the_broken_link() {
    let path = script("overdue-fault", WORKLOAD);
    let (stdout, stderr, ok) = run(&path, true);
    assert!(
        ok,
        "fault run failed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("AWAKE"),
        "fault run did not finish.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Sanity: the fault must have held, or the test proves nothing.
    assert!(
        !stdout.contains("CHILD-WOKE"),
        "the child woke — the fault did not drop its timer.\nstdout:\n{stdout}"
    );
    assert!(
        stderr.contains(REPORT),
        "a receive sat 8 s past its deadline and nothing said so.\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("parked with NO live timer entry: a lost timer"),
        "the report did not name the broken link (a lost timer).\nstderr:\n{stderr}"
    );
    assert_eq!(
        stderr.matches(REPORT).count(),
        1,
        "one report per process and deadline, not one per look.\nstderr:\n{stderr}"
    );
}
