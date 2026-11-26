//! Receive-timeout (`(after ms expr)`) machinery: a min-heap of pending
//! deadlines + one OS thread that wakes parked green processes at their
//! due times.
//!
//! Hooks into the scheduler via [`super::wake_for_timeout`] — the one piece
//! of mailbox plumbing this module needs. That helper takes a pid + park
//! generation, finds its mailbox in `REGISTRY`, and (if the gen is still
//! current) re-queues the parked waiter so it wakes, re-scans its mailbox,
//! and notices the deadline has passed.
//!
//! **Lazy cancellation, with compaction.** A superseded entry is not removed the moment
//! its park is superseded (a `BinaryHeap` cannot remove an interior element); each entry
//! carries the park **generation** it was armed under, and `wake_for_timeout` drops one
//! whose gen the mailbox has advanced past (see `Mailbox::timer_gen`). Reaping only at
//! the deadline would bound the heap by `arm-rate × horizon` — the message rate times the
//! timeout — so [`arm_timer`] also **compacts** once the heap passes twice its live size
//! (and a floor): amortized O(1) per arm, and the steady-state size tracks the number of
//! *currently parked* processes.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
// Only the native timer thread has a start flag; wasm has no timer thread.
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;
use web_time::Instant;

/// A `receive` deadline, on the scheduler clock ([`sched_now`]) — the only clock it can be
/// minted from or compared with. On native that clock is real time; on wasm it is the
/// logical clock [`fire_next_timer`] advances, and a deadline compared against real time
/// there spins the pump forever (re-queued by one gate, "not yet" to the next). The
/// `Instant` never leaves this module and `sched_now` is private to it, so mixing the two
/// clocks does not compile — what `tests/sched_clock_domain.rs` once checked by reading
/// source text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SchedDeadline(Instant);

impl SchedDeadline {
    /// `ms` milliseconds from now on the scheduler clock.
    pub(super) fn after_millis(ms: u64) -> SchedDeadline {
        SchedDeadline(sched_now() + Duration::from_millis(ms))
    }

    /// Has the scheduler clock reached this deadline?
    pub(super) fn reached(self) -> bool {
        sched_now() >= self.0
    }

    /// How long until the deadline on the scheduler clock; zero once it is reached.
    pub(super) fn remaining(self) -> Duration {
        self.0.saturating_duration_since(sched_now())
    }

    /// How long ago the deadline passed on the scheduler clock; zero until it is reached.
    pub(super) fn overdue_by(self) -> Duration {
        sched_now().saturating_duration_since(self.0)
    }
}

/// Min-heap of `(deadline, pid, gen)`: `Reverse` turns the max-heap into
/// earliest-first. `gen` is the parking process's [`Mailbox::timer_gen`] at arm
/// time — carried so the timer thread can detect (and skip) a superseded deadline.
type TimerQueue = BinaryHeap<Reverse<(SchedDeadline, u64, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
/// Whether the timer thread is running (native only — wasm has no timer thread; its
/// cooperative pump fires due timers itself).
///
/// **Not a `Once`, deliberately** (KI-97 item 3): a `Once` is poisoned by a refused
/// thread spawn, which would break every timeout runtime-wide for good. A plain CAS
/// instead: one caller wins the right to spawn, and a failed spawn **releases the flag**
/// so a later `arm_timer` retries; deadlines armed meanwhile stay queued (late, not dead).
#[cfg(not(target_arch = "wasm32"))]
static TIMER_STARTED: AtomicBool = AtomicBool::new(false);

/// Whether a refused timer thread has been reported. Every `arm_timer` retries, and under
/// a thread limit that is every timeout armed: one line says it, not one per retry.
#[cfg(not(target_arch = "wasm32"))]
static TIMER_REFUSAL_REPORTED: AtomicBool = AtomicBool::new(false);

/// How many timer threads have actually entered [`timer_loop`] — what lets a test tell
/// "the flag is set" from "a thread is really running".
#[cfg(not(target_arch = "wasm32"))]
static TIMER_THREADS_STARTED: AtomicUsize = AtomicUsize::new(0);

/// Arrange to wake green process `pid` at `deadline`. `gen` is the process's park
/// generation (stamped by the caller in `wait_for_message`) — the timer fires the
/// wakeup only while it's still current, giving lazy cancellation of superseded
/// deadlines. Lazily starts the timer thread on first use (programs that never use a
/// `receive` timeout never spawn it). On wasm there is no timer thread — the cooperative
/// scheduler pump fires due timers itself (`fire_next_timer`); we only record the deadline.
pub(super) fn arm_timer(pid: u64, deadline: SchedDeadline, generation: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    ensure_timer_thread();
    #[cfg(not(target_arch = "wasm32"))]
    if fault_drop_timer(deadline) {
        return;
    }
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    // Wake the timer thread only when this deadline is the NEW EARLIEST (or the heap was
    // empty, so the thread is in an untimed `wait`). It sleeps in `wait_timeout` until the
    // head's deadline, and an entry that sorts behind the head cannot need an earlier wake:
    // the thread will re-peek when the head comes due and find it. Notifying on every arm
    // was a futex wake per `(receive … (after ms …))` that parks — one per `gen/call`,
    // since the reply has not arrived when the caller parks — with the timer thread then
    // waking on another core, taking this lock against the next arm, and going straight
    // back to sleep. A `gen/call` deadline (5 s out) is almost never the earliest.
    let earliest = q
        .peek()
        .is_none_or(|Reverse((head, _, _))| deadline < *head);
    q.push(Reverse((deadline, pid, generation)));
    if q.len() >= compact_threshold() {
        compact(&mut q);
    }
    drop(q);
    if earliest {
        cv.notify_one();
    }
}

/// Floor below which compacting is pointless — a few hundred `(deadline, pid, gen)`
/// triples is a few KB, and a program with that many *live* parks is normal.
const COMPACT_FLOOR: usize = 256;

/// Live-entry count as of the last compaction; the heap is allowed to grow to twice this
/// (and past [`COMPACT_FLOOR`]) before the next one. Doubling is what makes the scheme
/// amortized O(1) per arm: a compaction inspecting `n` entries is preceded by ≥ `n/2`
/// pushes that were not compacted.
static LIVE_AFTER_COMPACT: AtomicUsize = AtomicUsize::new(0);

fn compact_threshold() -> usize {
    COMPACT_FLOOR.max(LIVE_AFTER_COMPACT.load(Ordering::Relaxed).saturating_mul(2))
}

/// Drop every superseded entry — one whose owning mailbox has advanced past its park
/// generation (so [`super::mailbox::wake_for_timeout`] would ignore it anyway), or whose
/// process is dead. Exactly the entries the timer thread would discard at their deadline;
/// this just stops them from occupying memory until then.
///
/// The gen lookups are cached per pid: the pathological case this exists for is a *single*
/// hot process with millions of stale entries, so the cache turns O(entries) registry
/// probes into O(distinct pids).
///
/// **Lock order** is TIMERS → registry shard, and it only ever runs that way: `arm_timer`
/// is never called while a registry shard or a mailbox `state` lock is held (`receive_match`
/// drops the state guard before arming), and the registry's own accessors take nothing else.
fn compact(q: &mut TimerQueue) {
    let mut gens: HashMap<u64, Option<u64>> = HashMap::new();
    let before = q.len();
    let kept: Vec<Reverse<(SchedDeadline, u64, u64)>> = std::mem::take(q)
        .into_vec()
        .into_iter()
        .filter(|Reverse((_, pid, generation))| {
            *gens
                .entry(*pid)
                .or_insert_with(|| super::mailbox::current_timer_gen(*pid))
                == Some(*generation)
        })
        .collect();
    let live = kept.len();
    *q = BinaryHeap::from(kept);
    LIVE_AFTER_COMPACT.store(live, Ordering::Relaxed);
    debug_assert!(live <= before);
}

/// How many `(deadline, pid, gen)` entries are pending. Test/diagnostic hook for the
/// compaction above — a stale-entry leak shows up here as unbounded growth.
#[doc(hidden)]
pub fn pending_timer_count() -> usize {
    crate::core::sync::lock(&TIMERS.0).len()
}

/// The scheduler's clock for receive deadlines. On native this is the real monotonic
/// clock. On wasm there is no timer thread, so real time never advances *while a snippet
/// runs* — `fire_next_timer` advances this LOGICAL clock to the fired deadline instead, so
/// a woken receive sees its deadline as reached and takes its `after` clause at once
/// rather than re-parking and spinning the pump for the full real delay. Private: outside
/// this module the clock is read only through a [`SchedDeadline`].
#[cfg(not(target_arch = "wasm32"))]
fn sched_now() -> Instant {
    Instant::now()
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LOGICAL_NOW: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    /// Set by a host that runs a program for as long as it lives — a game in a browser tab
    /// (`pump_due`) — rather than to completion: its receive deadlines are wall-clock
    /// times, reached when the time comes, not jumped to.
    static REAL_TIME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(target_arch = "wasm32")]
fn sched_now() -> Instant {
    if REAL_TIME.with(|r| r.get()) {
        return Instant::now();
    }
    LOGICAL_NOW.with(|c| {
        let now = c.get().unwrap_or_else(Instant::now);
        c.set(Some(now));
        now
    })
}

/// Run receive deadlines on the wall clock from now on (see `REAL_TIME`).
#[cfg(target_arch = "wasm32")]
pub(crate) fn use_real_time() {
    REAL_TIME.with(|r| r.set(true));
}

/// Fire the earliest pending receive-timeout if its deadline has passed on the wall clock;
/// whether one fired. The real-time pump's counterpart of `fire_next_timer`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn fire_due_timer() -> bool {
    let (lock, _cv) = &*TIMERS;
    let due = {
        let mut timers = crate::core::sync::lock(lock);
        match timers.peek() {
            Some(Reverse((SchedDeadline(deadline), _, _))) if *deadline <= Instant::now() => {
                timers.pop()
            }
            _ => None,
        }
    };
    match due {
        Some(Reverse((_, pid, generation))) => {
            super::mailbox::wake_for_timeout(pid, generation);
            true
        }
        None => false,
    }
}

/// How long until the earliest pending receive-timeout is due (zero when it already is),
/// or `None` with none pending — when a real-time host should pump next.
#[cfg(target_arch = "wasm32")]
pub(crate) fn next_timer_in() -> Option<std::time::Duration> {
    let (lock, _cv) = &*TIMERS;
    let timers = crate::core::sync::lock(lock);
    timers
        .peek()
        .map(|Reverse((SchedDeadline(deadline), _, _))| {
            deadline.saturating_duration_since(Instant::now())
        })
}

/// Fire the earliest pending receive-timeout, waking its parked process (wasm cooperative
/// scheduler — there is no timer thread). Called by the pump when nothing else is runnable:
/// the earliest deadline is then the only thing that can advance the program, so it fires in
/// logical time — advance the logical clock to that deadline so the woken receive's gate
/// (`sched_now() >= deadline`) is satisfied and the timeout resolves immediately (no real
/// wait, no busy-spin). Messages still win, since the pump drains all run queues before this
/// runs. A superseded entry (its `gen` advanced) wakes nothing but is still removed. Returns
/// whether an entry was fired.
#[cfg(target_arch = "wasm32")]
pub(crate) fn fire_next_timer() -> bool {
    let (lock, _cv) = &*TIMERS;
    let next = crate::core::sync::lock(lock).pop();
    match next {
        Some(Reverse((SchedDeadline(deadline), pid, generation))) => {
            LOGICAL_NOW.with(|c| {
                if c.get().is_none_or(|now| deadline > now) {
                    c.set(Some(deadline));
                }
            });
            super::mailbox::wake_for_timeout(pid, generation);
            true
        }
        None => false,
    }
}

/// Start the timer thread if it is not already running; safe to call on every arm.
///
/// Uses `Builder::spawn`, which *returns* the OS error rather than panicking on it, so a
/// refused thread is handled instead of unwinding through `arm_timer` into whichever green
/// process happened to call `sleep`.
#[cfg(not(target_arch = "wasm32"))]
fn ensure_timer_thread() {
    if TIMER_STARTED.load(Ordering::Acquire) {
        return;
    }
    // Only the CAS winner spawns; a loser simply proceeds, since the winner is starting
    // the thread that will drain the queue it is about to push onto.
    if TIMER_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if let Err(e) = std::thread::Builder::new()
        .name("brood-timer".into())
        .spawn(timer_loop)
    {
        // Release the claim so the next `arm_timer` tries again — the whole point of not
        // using a `Once` here. Queued deadlines are not lost, only late.
        TIMER_STARTED.store(false, Ordering::Release);
        if !TIMER_REFUSAL_REPORTED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "brood: cannot start the timer thread ({e}); timeouts are delayed until one starts"
            );
        }
    }
}

/// How often the timer thread looks for receives parked past their deadline
/// ([`super::mailbox::report_overdue_receives`]).
#[cfg(not(target_arch = "wasm32"))]
const OVERDUE_WATCH_PERIOD: Duration = Duration::from_secs(2);

/// `BROOD_FAULT_DROP_TIMER=<ms>`: FAULT INJECTION — `arm_timer` queues no deadline that is
/// due within `<ms>` of being armed, so a green `receive … (after …)` that short and parks is
/// never woken by its deadline; longer ones (a test's own sleep) still fire. Exists so the
/// overdue-receive watchdog is testable: a lost timer is what it watches for, and nothing an
/// ordinary program does provokes one. Read once.
#[cfg(not(target_arch = "wasm32"))]
fn fault_drop_timer(deadline: SchedDeadline) -> bool {
    static WITHIN: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    WITHIN
        .get_or_init(|| {
            std::env::var("BROOD_FAULT_DROP_TIMER")
                .ok()
                .and_then(|ms| ms.parse().ok())
                .map(Duration::from_millis)
        })
        .is_some_and(|within| deadline.remaining() <= within)
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed. Every
/// [`OVERDUE_WATCH_PERIOD`] it also looks for green receives still parked well past their
/// deadline and names them (KI-413: the mur forge's pool, 2026-11-19, eight processes parked in
/// `receive`, seven of them with `after`, that never ran again, with nothing on stderr to say
/// which way).
#[cfg(not(target_arch = "wasm32"))]
fn timer_loop() {
    TIMER_THREADS_STARTED.fetch_add(1, Ordering::Relaxed);
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    let mut next_watch = Instant::now() + OVERDUE_WATCH_PERIOD;
    let mut reported = HashMap::new();
    loop {
        let until_watch = next_watch.saturating_duration_since(Instant::now());
        if until_watch.is_zero() {
            // The live entries, as `(pid, gen)`: an overdue process whose current gen is
            // not among them has lost its timer.
            let pending: std::collections::HashSet<(u64, u64)> = q
                .iter()
                .map(|Reverse((_, pid, generation))| (*pid, *generation))
                .collect();
            drop(q);
            // Caught like a wake: a fault in a diagnostic must not end every timeout.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::mailbox::report_overdue_receives(&pending, &mut reported)
            }));
            next_watch = Instant::now() + OVERDUE_WATCH_PERIOD;
            q = crate::core::sync::lock(lock);
            continue;
        }
        match q.peek().copied() {
            // `unwrap_or_else(into_inner)`, not `unwrap`: a poisoned condvar wait would
            // otherwise kill the timer thread and with it every deadline in the runtime —
            // exactly the cascade `core/sync.rs` exists to refuse (KI-97 item 3).
            None => {
                q = cv
                    .wait_timeout(q, until_watch)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
            Some(Reverse((deadline, _, _))) => {
                let remaining = deadline.remaining();
                if !remaining.is_zero() {
                    q = cv
                        .wait_timeout(q, remaining.min(until_watch))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                } else {
                    let mut due = Vec::new();
                    while let Some(&Reverse((d, pid, generation))) = q.peek() {
                        if d.reached() {
                            q.pop();
                            due.push((pid, generation));
                        } else {
                            break;
                        }
                    }
                    drop(q);
                    // `wake_for_timeout` itself drops a superseded entry (gen no
                    // longer current), so we needn't filter here.
                    //
                    // Each wake is caught on its own: this thread is the only one that
                    // fires deadlines, and `TIMER_STARTED` stays set if it dies, so one
                    // panicking wake would otherwise end every timeout in the runtime —
                    // and silently. The panic hook has already reported the panic.
                    for (pid, generation) in due {
                        let _ = std::panic::catch_unwind(|| {
                            super::mailbox::wake_for_timeout(pid, generation)
                        });
                    }
                    q = crate::core::sync::lock(lock);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// KI-97 item 3: starting the timer thread must be **retryable**, not a `Once`. The OS
    /// cannot be made to refuse a thread on demand, so this asserts the property recovery
    /// rests on: `ensure_timer_thread` is safe to call repeatedly and leaves the flag set
    /// once a thread is actually running.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_timer_thread_start_is_retryable_not_a_once() {
        /// Wait for the spawned thread to actually reach `timer_loop` — the count is
        /// incremented by the thread itself, so it lags the spawn by a scheduling hop.
        fn started_at_least(n: usize) -> bool {
            for _ in 0..200 {
                if TIMER_THREADS_STARTED.load(Ordering::Relaxed) >= n {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            false
        }

        // Idempotent: many calls start exactly one thread, and none of them panics.
        for _ in 0..50 {
            super::ensure_timer_thread();
        }
        assert!(started_at_least(1), "a timer thread must be running");
        assert!(
            TIMER_STARTED.load(Ordering::Acquire),
            "and the flag must say so"
        );
        let after_first = TIMER_THREADS_STARTED.load(Ordering::Relaxed);
        assert_eq!(
            after_first, 1,
            "repeated calls must not start extra threads"
        );

        // The recovery shape: releasing the flag is what a FAILED spawn does, and a later
        // call must then start a real thread again. Asserting on the thread count rather
        // than the flag is the point — a regression that marks itself started without
        // starting anything (which is what a `Once` effectively did after poisoning)
        // passes a flag check and fails this one.
        TIMER_STARTED.store(false, Ordering::Release);
        super::ensure_timer_thread();
        assert!(
            started_at_least(2),
            "a released flag must yield a genuinely NEW timer thread — this is what a \
             `Once` could not do"
        );

        // And timeouts still work end to end after all that.
        let (lock, _) = &*TIMERS;
        let before = crate::core::sync::lock(lock).len();
        arm_timer(u64::MAX, SchedDeadline::after_millis(3_600_000), 0);
        assert!(
            crate::core::sync::lock(lock).len() > before,
            "arm_timer must still queue a deadline"
        );
    }

    use super::*;

    /// Superseded timer entries must be **compacted**, not merely reaped at their
    /// deadlines.
    ///
    /// Every entry here is stale by construction (the pid was never registered) with a
    /// deadline an hour out, so nothing can be reaped by firing; only compaction can keep
    /// the heap bounded.
    #[test]
    fn superseded_entries_are_compacted_not_left_until_their_deadline() {
        let far = SchedDeadline::after_millis(3_600_000);
        let dead_pid = u64::MAX - 7; // never in the process REGISTRY
        let armed = 4000;
        for generation in 0..armed {
            arm_timer(dead_pid, far, generation);
        }
        let pending = pending_timer_count();
        assert!(
            pending < armed as usize / 2,
            "{pending} of {armed} superseded entries still pending — compaction did not run"
        );
    }
}
