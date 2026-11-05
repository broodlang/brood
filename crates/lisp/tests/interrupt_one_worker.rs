//! A `receive` inside an interrupt window, on a pool of ONE worker (ADR-494).
//!
//! The VM runs `proc/interruptible` as a region on its own frame stack (ADR-491), so a
//! `receive` in the thunk parks. ADR-494 closed the paths that still ran the thunk nested
//! under the builtin: `(apply proc/interruptible …)` on the VM, and every call on the
//! tier-0 machine (`BROOD_TIER=0`, which CI runs this binary under too), where the call is
//! now a frame. With one worker, a process that blocked its thread would leave the next
//! process to an overflow thread; the dirty-receive count is what tells a park from a block.
//!
//! A fold over a vector or a range calls a closure reducer from the prelude's walk, not from
//! the native span (ADR-502), so a window in a `map` / `fold` / `reduce` callback parks too.
//! A builtin that would call Brood from Rust — a record's `num/add` under `+`, its `->seq`
//! under `first` — hands the call back to the engine (ADR-505), so a window there parks as
//! well. What still runs nested (`isolate`'s thunk, a JIT fallback's callback) blocks its
//! worker, which leaves the executors so the queue moves on (ADR-100 §7.4), and an interrupt
//! reaches it once it runs again, as it reaches a parked one.
//!
//! Its own binary: the pool size is process-wide and fixed at the first spawn. The tests
//! share the dirty-receive counter, so they run one at a time.

use std::sync::Mutex;

use brood::{Interp, process};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Evaluate `src` in a fresh interpreter on a pool of one worker, printed.
fn run(src: &str) -> String {
    process::set_max_parallel(1);
    let mut interp = Interp::new();
    match interp.eval_str(src) {
        Ok(value) => interp.print(value),
        Err(error) => format!("error: {}", error.message),
    }
}

/// Helpers every program here shares: `await-waiting` polls a pid until it is parked in
/// `receive` (a barrier, not a guessed delay; `:dead` if it died instead), `interrupt-until` asks until a running
/// process takes the interrupt.
const PRELUDE: &str = r#"
    (def root (self))
    (defn spin (n) (spin (inc n)))
    (defn await-waiting (pid)
      (let (info (proc/info pid))
        (cond (nil? info) :dead
              (= :waiting (:status info)) :parked
              else (do (proc/sleep 1) (await-waiting pid)))))
    (defn interrupt-until (pid tries)
      (cond (proc/interrupt pid) :landed
            (= tries 0) :never
            else (do (proc/sleep 1) (interrupt-until pid (dec tries)))))
"#;

/// A process waits in a window opened by `open` (`(open ceiling thunk)`), and the program
/// checks: another process runs on the one worker meanwhile, the waiter answers once
/// released, a ceiling passed while it waited lands once it runs, and so does
/// `proc/interrupt` — which a waiting process refuses.
///
/// Each waiter answers before the next is spawned: a suspend-host latch lands as the blocked
/// walk EXITS, so a waiter still in its walk when the next began would let that one block too
/// (KI-313: `late` was interrupted while `woken` walked natively beside it).
fn waits_in_a_window(open: &str) -> String {
    run(&format!(
        r#"{PRELUDE}
        (def open {open})
        (def waiter (spawn (send root [:waited (open nil (fn () (receive (:go :went))))])))
        (await-waiting waiter)
        (spawn (send root :ran))
        (def ran (receive (:ran :ran) (after 10000 :starved)))
        (send waiter :go)
        (def waited (receive ([:waited r] r) (after 10000 :waiter-lost)))
        (def late (spawn (send root [:late (first (open 20 (fn () (do (receive (:go nil)) (spin 0)))))])))
        (await-waiting late)
        (proc/sleep 30)
        (send late :go)
        (def late-answer (receive ([:late r] r) (after 10000 :late-lost)))
        (def woken (spawn (send root [:woken (first (open nil (fn () (do (receive (:go nil)) (spin 0)))))])))
        (await-waiting woken)
        (def refused (proc/interrupt woken))
        (send woken :go)
        [ran waited
         late-answer
         refused
         (interrupt-until woken 10000)
         (receive ([:woken r] r) (after 10000 :woken-lost))]
        "#
    ))
}

const ANSWERED: &str = "[:ran [:ok :went] :interrupted false :landed :interrupted]";

#[test]
fn a_receive_in_a_window_parks() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dirty = process::dirty_receive_block_count();
    assert_eq!(
        waits_in_a_window("(fn (ceiling thunk) (proc/interruptible ceiling thunk))"),
        ANSWERED
    );
    assert_eq!(
        process::dirty_receive_block_count(),
        dirty,
        "a receive inside a window blocked its worker"
    );
}

#[test]
fn a_receive_in_an_applied_window_parks() {
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dirty = process::dirty_receive_block_count();
    assert_eq!(
        waits_in_a_window("(fn (ceiling thunk) (apply proc/interruptible [ceiling thunk]))"),
        ANSWERED
    );
    assert_eq!(
        process::dirty_receive_block_count(),
        dirty,
        "a receive inside an applied window blocked its worker"
    );
}

/// `waits_in_a_window(open)` answers [`ANSWERED`] and no receive in it blocked the worker.
fn parks_in_a_window(open: &str) {
    let dirty = process::dirty_receive_block_count();
    assert_eq!(waits_in_a_window(open), ANSWERED, "{open}");
    assert_eq!(
        process::dirty_receive_block_count(),
        dirty,
        "a receive blocked its worker: {open}"
    );
}

#[test]
fn a_window_in_a_fold_callback_parks() {
    // ADR-502: a fold over a vector or a range calls a closure reducer from the prelude's
    // walk, on the process's own engine, not from the native span's loop — so the window it
    // opens, and the receive inside, park the process. The reducers are bound to a name so
    // the compiler's counted-loop rewrite of a literal (ADR-360) does not take the call.
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for open in [
        // the reduce ADR-494 had to leave blocking
        "(fn (ceiling thunk) (let (step (fn (_ c) (proc/interruptible c thunk))) (reduce [ceiling] nil step)))",
        // a range
        "(fn (ceiling thunk) (let (step (fn (_ i) (proc/interruptible ceiling thunk))) (fold (range 1) nil step)))",
        // what folds underneath: map, and through it mapv, sort-by, each
        "(fn (ceiling thunk) (first (map [ceiling] (fn (c) (proc/interruptible c thunk)))))",
        "(fn (ceiling thunk) (first (seq/mapv [ceiling] (fn (c) (proc/interruptible c thunk)))))",
        // a ui-run turn: the window outside, the receive in a callback of the update
        "(fn (ceiling thunk) (proc/interruptible ceiling (fn () (first (map [1] (fn (_) (thunk)))))))",
        // a window inside a callback inside a window: the inner is the callback's
        "(fn (ceiling thunk)
           (second (proc/interruptible nil (fn () (first (map [ceiling] (fn (c) (proc/interruptible c thunk))))))))",
    ] {
        parks_in_a_window(open);
    }
}

#[test]
fn a_window_under_a_kernel_dispatch_parks() {
    // A builtin that would call Brood from Rust — a record's method under an operator, its
    // `->seq` under `first` — hands the call back to the engine instead (ADR-505), which
    // makes it as an ordinary call. So a window the method opens, and the receive inside,
    // park the process: until then these blocked the worker (ADR-502's "what still blocks").
    // A method that must answer an int (`compare-to`, `->count`) sends the window's answer
    // to its caller instead. Each one is bound to a name, then called through `open`.
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for open in [
        // a record's `num/add` under `+`, the case ADR-502 pinned as blocking
        "(do (defrecord knock (ceiling thunk))
             (defmethod num/add [knock knock] (a b) (proc/interruptible (get a :ceiling) (get a :thunk)))
             (fn (ceiling thunk) (+ (knock ceiling thunk) (knock ceiling thunk))))",
        // through a function of two arguments: the operator's operands are its locals
        "(do (defrecord knock (ceiling thunk))
             (defmethod num/sub [knock knock] (a b) (proc/interruptible (get a :ceiling) (get a :thunk)))
             (defn minus (a b) (- a b))
             (fn (ceiling thunk) (minus (knock ceiling thunk) (knock ceiling thunk))))",
        // `compare-to` under `<`
        "(do (defrecord knock (ceiling thunk))
             (defmethod compare-to [knock knock] (a b)
               (do (send (self) [:window (proc/interruptible (get a :ceiling) (get a :thunk))]) 0))
             (fn (ceiling thunk)
               (do (< (knock ceiling thunk) (knock ceiling thunk)) (receive ([:window r] r)))))",
        // a record's `->seq` under `first`
        "(do (defrecord knock (ceiling thunk))
             (impl Seqable knock (->seq (k) (list (proc/interruptible (get k :ceiling) (get k :thunk)))))
             (fn (ceiling thunk) (first (knock ceiling thunk))))",
        // its `->count` under `count`, and under `empty?`
        "(do (defrecord knock (ceiling thunk))
             (impl Counted knock
               (->count (k) (do (send (self) [:window (proc/interruptible (get k :ceiling) (get k :thunk))]) 1)))
             (fn (ceiling thunk) (do (count (knock ceiling thunk)) (receive ([:window r] r)))))",
        "(do (defrecord knock (ceiling thunk))
             (impl Counted knock
               (->count (k) (do (send (self) [:window (proc/interruptible (get k :ceiling) (get k :thunk))]) 1)))
             (fn (ceiling thunk) (do (empty? (knock ceiling thunk)) (receive ([:window r] r)))))",
        // `apply` taken as a value: the engine unfolds it wherever it is called from Brood
        "(fn (ceiling thunk) (let (call apply) (call proc/interruptible [ceiling thunk])))",
        // a wrapper over the builtin bound under another name (ADR-494 §3)
        "(do (def interruptibly proc/interruptible)
             (defn through-alias (ceiling thunk) (interruptibly ceiling thunk))
             through-alias)",
    ] {
        parks_in_a_window(open);
    }
}

#[test]
fn a_receive_lane_parks_inside_a_window_and_out() {
    // `proc/receive-lane` (ADR-490) waits as `%receive` does, so on a capturing machine it
    // suspends: the tier-0 machine has to root and redo it as it does a `%receive`, or the
    // suspend surfaces as an empty `runtime error:`. Inside a window — how `ui-run` polls a
    // terminal once the window is a frame — and outside one.
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dirty = process::dirty_receive_block_count();
    let printed = run(&format!(
        r#"{PRELUDE}
        (def windowed (spawn (send root [:windowed (proc/interruptible nil (fn () (proc/receive-lane nil)))])))
        (def bare (spawn (send root [:bare (proc/receive-lane nil)])))
        (await-waiting windowed)
        (await-waiting bare)
        (send windowed :go)
        (proc/send-priority bare :key)
        [(receive ([:windowed r] r) (after 10000 :windowed-lost))
         (receive ([:bare r] r) (after 10000 :bare-lost))]
        "#
    ));
    assert_eq!(printed, "[[:ok [:go :normal]] [:key :priority]]");
    assert_eq!(
        process::dirty_receive_block_count(),
        dirty,
        "a receive-lane blocked its worker"
    );
}

#[test]
fn a_native_walk_that_hosts_a_park_blocks_once_and_then_parks() {
    // The walk is a self-tail loop, so the JIT compiles it once it is hot. Native code cannot
    // be captured, so the first receive under a native walk blocks the worker; the
    // suspend-host latch then takes the walk off native code for the runtime (ADR-410's net,
    // the fate `%fold-loop` over a list always had), and the program's two later waits park.
    // On the tree-walker or the bytecode VM alone none blocks.
    let _serial = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let hot = "(fn (ceiling thunk)
                 (let (step (fn (acc i) (if (= i 4999) (proc/interruptible ceiling thunk) acc)))
                   (fold (into [] (range 5000)) nil step)))";
    let dirty = process::dirty_receive_block_count();
    assert_eq!(waits_in_a_window(hot), ANSWERED);
    let blocks = process::dirty_receive_block_count() - dirty;
    assert!(
        blocks <= 1,
        "a native walk blocked {blocks} times: the latch did not take it off native code"
    );
}
