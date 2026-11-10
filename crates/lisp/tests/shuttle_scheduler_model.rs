//! Shuttle model-check of the scheduler's **park / spawn handshake** and **steal ownership**
//! (`process/scheduler/pool.rs`).
//!
//! Like `loom_table_protocol.rs`, this is a **faithful miniature**, not the production code:
//! the real queues hold `Box<Process>` behind `std` locks that Shuttle cannot schedule, so the
//! protocol's steps are transliterated onto Shuttle's primitives — the same orderings, the
//! same atomics, the same lock discipline. Shuttle explores schedules at random and with PCT
//! (probabilistic concurrency testing), which reaches the four-thread shapes below where
//! loom's exhaustive search would not finish. A pass is evidence, not proof.
//!
//! Two properties:
//!
//! 1. **The handshake never loses a spawn** (`wake_a_parked_peer` against the park path of
//!    `worker_loop`). A spawner bumps `STEALABLE` and then reads `PARKED_COUNT`; a parker
//!    bumps `PARKED_COUNT` and then reads `STEALABLE`. All SeqCst, so at least one side sees
//!    the other: the spawner wakes the parker, or the parker sees the work and parks on the
//!    short `STEAL_BACKOFF` rather than the long idle backoff. The failure this guards is
//!    a worker asleep for the idle backoff while a freshly spawned child waits behind a busy
//!    owner. `the_check_finds_a_reordered_handshake` runs the same model with the spawner's
//!    two steps swapped and requires Shuttle to find the bad schedule, so this check is known
//!    to be able to fail.
//! 2. **Every queued process runs exactly once, and `STEALABLE` returns to zero** (`run_one`'s
//!    single decrement against `push_runnable`'s increment), with owners popping the front of
//!    their queues and thieves taking the back of a peer's through `try_lock` (INV-2).
//!
//! Keep this in sync with `push_runnable`, `wake_a_parked_peer`, `try_steal` and the park
//! block of `worker_loop` when they change.
//!
//! Run: `make shuttle` (cargo test -p brood --release --features brood/shuttle-model
//! --test shuttle_scheduler_model).
#![cfg(feature = "shuttle-model")]

use shuttle::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use shuttle::sync::{Arc, Mutex};
use shuttle::thread;
use std::collections::VecDeque;

/// Schedules tried per property. Each run is microseconds; the count is what buys coverage.
const ITERATIONS: usize = 20_000;
/// PCT's bug depth: how many ordering constraints a bug needs. The handshake's is two.
const PCT_DEPTH: usize = 3;

// ---- 1. the park / spawn handshake --------------------------------------------------------

struct Handshake {
    stealable: AtomicUsize,
    parked_count: AtomicUsize,
    worker_parked: AtomicBool,
    /// The parker's queue lock — the park re-check happens under it.
    parker_queue: Mutex<VecDeque<u64>>,
    /// The spawner's own queue, where the child lands (spawn placement: the spawner's queue).
    spawner_queue: Mutex<VecDeque<u64>>,
    /// Set when `wake_a_parked_peer` notifies the parker.
    woke: AtomicBool,
    /// Set when the parker read `STEALABLE == 0` and so chose the long idle backoff.
    parked_long: AtomicBool,
}

/// `push_runnable` onto the spawner's queue, then `wake_a_parked_peer`. With `reordered`, the
/// peer probe runs BEFORE the increment — the bug the property exists to catch.
fn spawn_child(h: &Handshake, reordered: bool) {
    let probe = |h: &Handshake| {
        if h.parked_count.load(SeqCst) == 0 {
            return;
        }
        if h.worker_parked.load(SeqCst) {
            // `notify_one` on the parker's condvar.
            h.woke.store(true, SeqCst);
        }
    };
    if reordered {
        probe(h);
    }
    h.stealable.fetch_add(1, SeqCst);
    h.spawner_queue.lock().unwrap().push_back(1);
    if !reordered {
        probe(h);
    }
}

/// The park block of `worker_loop`: own queue empty under the lock, advertise the park, read
/// `STEALABLE` to choose the backoff.
fn park(h: &Handshake) {
    let q = h.parker_queue.lock().unwrap();
    if q.is_empty() {
        h.worker_parked.store(true, SeqCst);
        h.parked_count.fetch_add(1, SeqCst);
        let pool_empty = h.stealable.load(SeqCst) == 0;
        if pool_empty {
            h.parked_long.store(true, SeqCst);
        }
    }
    drop(q);
}

fn handshake_model(reordered: bool) {
    let h = Arc::new(Handshake {
        stealable: AtomicUsize::new(0),
        parked_count: AtomicUsize::new(0),
        worker_parked: AtomicBool::new(false),
        parker_queue: Mutex::new(VecDeque::new()),
        spawner_queue: Mutex::new(VecDeque::new()),
        woke: AtomicBool::new(false),
        parked_long: AtomicBool::new(false),
    });
    let spawner = {
        let h = h.clone();
        thread::spawn(move || spawn_child(&h, reordered))
    };
    let parker = {
        let h = h.clone();
        thread::spawn(move || park(&h))
    };
    spawner.join().unwrap();
    parker.join().unwrap();
    // The child is queued. Either the parker was told, or it saw the work and kept the
    // short backstop — never asleep for the idle backoff with nobody to wake it.
    assert!(
        h.woke.load(SeqCst) || !h.parked_long.load(SeqCst),
        "a spawn was lost: the peer parked on the idle backoff and the spawner did not wake it"
    );
    assert_eq!(h.spawner_queue.lock().unwrap().len(), 1);
}

#[test]
fn the_handshake_never_loses_a_spawn_random() {
    shuttle::check_random(|| handshake_model(false), ITERATIONS);
}

#[test]
fn the_handshake_never_loses_a_spawn_pct() {
    shuttle::check_pct(|| handshake_model(false), ITERATIONS, PCT_DEPTH);
}

/// The sabotage, kept: with the spawner's increment moved after its probe, the bad schedule
/// exists (probe sees nobody parked → parker parks → parker reads STEALABLE 0 → increment),
/// and the check must find it. If this ever passes, the property above has gone vacuous.
#[test]
fn the_check_finds_a_reordered_handshake() {
    let found = std::panic::catch_unwind(|| {
        shuttle::check_pct(|| handshake_model(true), ITERATIONS, PCT_DEPTH);
    });
    assert!(
        found.is_err(),
        "Shuttle did not find the lost spawn in a handshake built to lose one"
    );
}

// ---- 2. steal ownership -------------------------------------------------------------------

const WORKERS: usize = 3;
const TASKS: u64 = 6;

struct Pool {
    queues: Vec<Mutex<VecDeque<u64>>>,
    stealable: AtomicUsize,
    /// How many times each task ran.
    ran: Mutex<Vec<u32>>,
    done: AtomicUsize,
}

/// `run_one`: the single `STEALABLE` decrement, then the work.
fn run_one(pool: &Pool, task: u64) {
    pool.stealable.fetch_sub(1, SeqCst);
    pool.ran.lock().unwrap()[task as usize] += 1;
    pool.done.fetch_add(1, SeqCst);
}

/// `try_steal`: gated on `STEALABLE`, a `try_lock` scan of the peers, the back of the queue.
/// With `racy`, the thief reads the back under one lock and takes it under another — the
/// check-then-act gap the single locked `pop_back` exists to close.
fn try_steal(pool: &Pool, thief: usize, racy: bool) -> Option<u64> {
    if pool.stealable.load(SeqCst) == 0 {
        return None;
    }
    for off in 1..WORKERS {
        let victim = (thief + off) % WORKERS;
        if racy {
            let seen = pool.queues[victim]
                .try_lock()
                .ok()
                .and_then(|q| q.back().copied());
            if let Some(task) = seen {
                pool.queues[victim].lock().unwrap().retain(|&t| t != task);
                return Some(task);
            }
            continue;
        }
        if let Ok(mut q) = pool.queues[victim].try_lock()
            && let Some(task) = q.pop_back()
        {
            return Some(task);
        }
    }
    None
}

fn worker(pool: &Pool, wid: usize, racy: bool) {
    while pool.done.load(SeqCst) < TASKS as usize {
        // The pop is its own statement, so the guard drops before `run_one` (as in
        // `worker_loop`, where holding it across the run would deadlock a re-enqueue).
        let own = pool.queues[wid].lock().unwrap().pop_front();
        if let Some(task) = own {
            run_one(pool, task);
            continue;
        }
        if let Some(task) = try_steal(pool, wid, racy) {
            run_one(pool, task);
            continue;
        }
        thread::yield_now();
    }
}

fn ownership_model(racy: bool) {
    let pool = Arc::new(Pool {
        queues: (0..WORKERS).map(|_| Mutex::new(VecDeque::new())).collect(),
        stealable: AtomicUsize::new(0),
        ran: Mutex::new(vec![0; TASKS as usize]),
        done: AtomicUsize::new(0),
    });
    let workers: Vec<_> = (0..WORKERS)
        .map(|wid| {
            let pool = pool.clone();
            thread::spawn(move || worker(&pool, wid, racy))
        })
        .collect();
    // The spawner: every task onto worker 0's queue (spawn placement while that queue has a
    // backlog is the shape stealing exists for), each counted before it is visible.
    for task in 0..TASKS {
        pool.stealable.fetch_add(1, SeqCst);
        pool.queues[0].lock().unwrap().push_back(task);
    }
    for w in workers {
        w.join().unwrap();
    }
    // A double run over-counts `done`, so a racy run can leave a task unrun; either way
    // the tally says so.
    assert_eq!(
        *pool.ran.lock().unwrap(),
        vec![1; TASKS as usize],
        "a task ran twice or never"
    );
    assert_eq!(pool.stealable.load(SeqCst), 0, "STEALABLE did not balance");
}

#[test]
fn every_queued_process_runs_exactly_once_random() {
    shuttle::check_random(|| ownership_model(false), ITERATIONS / 4);
}

#[test]
fn every_queued_process_runs_exactly_once_pct() {
    shuttle::check_pct(|| ownership_model(false), ITERATIONS / 4, PCT_DEPTH);
}

/// The sabotage for property 2, kept: a thief that peeks and takes under two locks lets the
/// owner pop the same task in between, and the check must catch the double run.
#[test]
fn the_check_finds_a_racy_steal() {
    let found = std::panic::catch_unwind(|| {
        shuttle::check_pct(|| ownership_model(true), ITERATIONS / 4, PCT_DEPTH);
    });
    assert!(
        found.is_err(),
        "Shuttle did not find the double run in a steal built to race"
    );
}
