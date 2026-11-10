//! KI-268: a closure copied into another process of the same runtime keeps its compiled
//! code, so a `receive` in it parks instead of holding an OS thread.
//!
//! A closure that crossed between processes used to have its body rebuilt as LOCAL data in
//! the receiver. A LOCAL body has no `cache_key`, so it never compiled and ran on the
//! tree-walker, where a `receive` cannot park: it blocks its OS thread, and the scheduler
//! starts an overflow thread for the work queued behind it. ADR-405 made `spawn` copy every
//! captured value this way, closures inside them included, so store-postgres's 200 clients
//! of a 3-connection pool each held a thread; once the OS refused one more, the connection
//! processes had nothing to run on and the test hung.
//!
//! Both copy paths are covered: a closure inside a value `spawn` captures, and a closure
//! that arrives in a message. Each runs 200 clients of a 3-token pool through a closure
//! made at run time (as `pg/adaptor` makes its functions) and asserts the clients never ran
//! more at once than there are workers — parked, not holding threads. Evaluated as the root
//! process through `Interp`, like `demonitor_race.rs`.

use brood::Interp;

const POOL: &str = r#"
(defn conn-loop ()
  (receive
    ([:query from r n] (do (send from [:reply r n]) (conn-loop)))
    ([:stop] nil)))

(defn pool-loop (free waiters)
  (receive
    ([:checkout from r]
      (if (empty? free)
        (pool-loop free (append waiters (list [from r])))
        (do (send from [:reply r (first free)]) (pool-loop (rest free) waiters))))
    ([:checkin pid]
      (if (empty? waiters)
        (pool-loop (cons pid free) waiters)
        (let ([from r] (first waiters))
          (send from [:reply r pid])
          (pool-loop free (rest waiters)))))))

(defn checkout (pool)
  (let (r (ref))
    (send pool [:checkout (self) r])
    (receive ([:reply ^r pid] pid))))

(defn query (pid n)
  (let (r (ref))
    (send pid [:query (self) r n])
    (receive ([:reply ^r v] v) (after 10000 :timeout))))

(defn with-conn (pool f)
  (let (pid (checkout pool))
    (try
      (let (result (f pid))
        (send pool [:checkin pid])
        result)
      (catch e (do (send pool [:checkin pid]) (throw e))))))

;; Made at run time, inside a function, as store-postgres's `pg/adaptor` makes its own.
(defn make-adaptor () {:exec (fn (pool n) (with-conn pool (fn (pid) (query pid n))))})

(defn start-pool ()
  (spawn (pool-loop (map (range 3) (fn (_) (spawn (conn-loop)))) '())))

(defn collect (n)
  (into [] (map (range n) (fn (_) (receive ([:done got want] (= got want)))))))
"#;

/// Run `clients` as the program's last form, then answer
/// `[every-answer-right peak-running workers]`.
fn run(clients: &str) -> (bool, i64, i64) {
    let mut interp = Interp::new();
    interp
        .eval_str(POOL)
        .expect("the pool definitions evaluate");
    let out = interp
        .eval_str(&format!(
            "(let (results {clients} stats (%sched-stats))
               [(every? results (fn (ok) ok)) (get stats :peak-threads) (get stats :workers)])"
        ))
        .expect("the clients evaluate");
    let printed = interp.print(out);
    let fields: Vec<&str> = printed
        .trim_matches(|c| c == '[' || c == ']')
        .split_whitespace()
        .collect();
    assert_eq!(fields.len(), 3, "unexpected result shape: {printed}");
    (
        fields[0] == "true",
        fields[1].parse().expect("peak is an int"),
        fields[2].parse().expect("workers is an int"),
    )
}

/// Whether this run uses a compiled engine. These tests are about compiled code crossing
/// between processes; under the tree-walker (`BROOD_TIER=0`, CI's differential job) every
/// `receive` holds its thread by design, so 200 clients need ~200 threads and the outcome
/// depends on the machine's thread limit, not on the copy. Skipped by ceiling, as
/// `process_floor.rs` does.
fn compiled_engine() -> bool {
    let compiled = brood::eval::compile::tier_ceiling() != brood::eval::compile::Tier::TreeWalk;
    if !compiled {
        eprintln!("closure_copy_stays_compiled: the tree-walker compiles nothing — skipped");
    }
    compiled
}

/// The clients parked rather than holding threads: no more processes ran at once than there
/// are workers.
fn assert_parked(peak: i64, workers: i64, what: &str) {
    assert!(
        peak <= workers,
        "{peak} processes ran at once on {workers} workers: the clients held OS threads in \
         their receives instead of parking ({what} ran on the tree-walker)"
    );
}

#[test]
fn a_closure_inside_a_spawned_capture_stays_compiled() {
    if !compiled_engine() {
        return;
    }
    let (ok, peak, workers) = run(
        "(let (repo {:adaptor (make-adaptor) :pool (start-pool)} parent (self))
           (doseq (c (range 200))
             (spawn (send parent [:done ((get (get repo :adaptor) :exec) (get repo :pool) c) c])))
           (collect 200))",
    );
    assert!(ok, "a client got a wrong answer or timed out");
    assert_parked(peak, workers, "a copied closure");
}

#[test]
fn a_closure_received_in_a_message_stays_compiled() {
    if !compiled_engine() {
        return;
    }
    let (ok, peak, workers) = run(
        "(let (adaptor (make-adaptor) pool (start-pool) parent (self))
           (doseq (c (range 200))
             (let (client (spawn (receive ([:work f] (f)))))
               (send client [:work (fn () (send parent [:done ((get adaptor :exec) pool c) c]))])))
           (collect 200))",
    );
    assert!(ok, "a client got a wrong answer or timed out");
    assert_parked(peak, workers, "a sent closure");
}
