//! Superseded code is reclaimed in a multi-process runtime (ADR-091): a drain armed while
//! other processes run must complete, so redefinitions do not accumulate.
//!
//! Two faults kept the first drain armed for the whole run, and since a new drain waits for
//! the previous free, nothing was reclaimed after it (120 000 redefinitions kept 116 000
//! closures):
//!
//! 1. A ROOT context blocked waiting on a top-level program (every `brood FILE` run, ADR-135)
//!    stayed in the live set but reached no safepoint and was not a parked green process the
//!    coordinator could inspect, so it never acknowledged a drain. The wait now reports for
//!    itself (`report_root_wait_liveness`).
//! 2. The free was attempted once per 64 collector entries, and entries come only at
//!    doublings of the code region — so in practice never. It is attempted at every entry.
//!
//! The program below has both: the root context is minted before the program starts (as
//! the check-cache replay's `require` mints it), a peer process parks for the whole run, and a
//! reloader redefines one function 36 000 times.
//!
//! Still open, and not this test's subject: a process whose RUNNING code reaches the draining
//! generation pins it until that code returns (`rehome_drain_pins` cannot rewrite a live
//! arm), so a driver that never parks between bursts holds a drain open while it runs.

use brood::Interp;

const PROGRAM: &str = r#"
(def parked (spawn (receive (:stop :ok))))
(defn reloader ()
  (receive
    ([:reload n] (do (reflect/eval-string (str "(defn drain-probe-f (x) (+ x " n "))")) (reloader)))
    ([:closures from] (do (send from [:closures (get (dev/gc-stats) :runtime-closures)]) (reloader)))
    (:stop :ok)))
(def r (spawn (reloader)))
(defn batch (round k) (when (> k 0) (send r [:reload (+ (* round 3000) k)]) (batch round (- k 1))))
;; Each round waits for the reloader to catch up, as an editor's loop waits for input: the
;; driver parks between bursts, so it holds no code of its own across a drain.
(defn rounds (round closures)
  (if (< round 12)
    (do
      (batch round 3000)
      (send r [:closures (self)])
      (rounds (+ round 1) (receive ([:closures n] n))))
    closures))
(def closures (rounds 0 0))
(send r :stop)
(send parked :stop)
(when (> closures 20000)
  (error "superseded code was not reclaimed: " closures " RUNTIME closures after 36000 redefinitions"))
"#;

#[test]
fn redefinitions_are_reclaimed_while_other_processes_run() {
    let outcome = std::thread::Builder::new()
        .stack_size(brood::process::WORKER_STACK_BYTES)
        .spawn(|| {
            let mut interp = Interp::new();
            // Mint this thread's root context before the program starts, as a `brood FILE`
            // run's check-cache replay does: the context the program's wait must answer for.
            interp.eval_str("(self)").expect("mint the root context");
            interp
                .run_program(PROGRAM, None)
                .map_err(|e| e.message.clone())
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
    assert_eq!(outcome, Ok(()), "the program reported a leak or failed");
}
