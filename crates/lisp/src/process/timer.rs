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
//! **Lazy cancellation, with compaction.** An entry is not removed from the heap the
//! moment its park is superseded — a `(receive … (after ms …))` woken by `send` each
//! iteration would otherwise churn arm/disarm pairs, and a `BinaryHeap` cannot remove an
//! interior element anyway. Instead each entry carries the park **generation** it was
//! armed under; `wake_for_timeout` drops an entry whose gen the mailbox has since advanced
//! past (see `Mailbox::timer_gen`).
//!
//! Firing-time reaping alone is **not** a bound worth relying on, though. It bounds a
//! stale entry's *lifetime* by the deadline horizon, so the heap's size is bounded by
//! `arm-rate × horizon`, not by the horizon — and the arm rate is the message rate. A
//! gen-server looping `(receive … (after 3600000 …))` at 10k msg/s accrues ~36M dead
//! entries (GB-scale, on one global heap behind one mutex) before the first one comes due,
//! then reaps them in a burst. So [`arm_timer`] also **compacts**: when the heap has grown
//! past twice its live size (and past a floor), it drops every entry whose generation the
//! owning mailbox has moved on from, or whose process is gone. Amortized O(1) per arm —
//! each compaction is paid for by at least as many pushes as it inspects — and it keeps
//! the steady-state size proportional to the number of *currently parked* processes rather
//! than to how many messages they have handled.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use web_time::Instant;

/// Min-heap of `(deadline, pid, gen)`: `Reverse` turns the max-heap into
/// earliest-first. `gen` is the parking process's [`Mailbox::timer_gen`] at arm
/// time — carried so the timer thread can detect (and skip) a superseded deadline.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
/// Whether the timer thread is running (native only — wasm has no timer thread; its
/// cooperative pump fires due timers itself).
///
/// **Not a `Once`, deliberately.** `Once::call_once` is *poisoned* by a panic inside it,
/// and `std::thread::spawn` panics when the OS refuses a thread (EAGAIN under fd/thread
/// pressure). One such refusal therefore made every later `call_once` panic — and this
/// runs from `arm_timer`, which backs `sleep` and every `receive … (after ms …)`, so a
/// single transient failure permanently broke all timeouts runtime-wide. KI-97 item 3.
///
/// A plain CAS instead: exactly one caller wins the right to spawn, and if the spawn
/// fails it **releases the flag** so a later `arm_timer` retries. Deadlines armed in the
/// meantime stay queued and are serviced once a spawn succeeds, so the degradation is
/// "timeouts are late until threads are available" rather than "timeouts are dead".
#[cfg(not(target_arch = "wasm32"))]
static TIMER_STARTED: AtomicBool = AtomicBool::new(false);

/// How many timer threads have actually entered [`timer_loop`].
///
/// One relaxed increment for the life of a process, and it buys the only assertion that
/// distinguishes "the flag is set" from "a thread is really running" — which is exactly
/// the difference between the fixed code and a `Once`-shaped regression that marks itself
/// started without starting anything.
#[cfg(not(target_arch = "wasm32"))]
static TIMER_THREADS_STARTED: AtomicUsize = AtomicUsize::new(0);

/// Arrange to wake green process `pid` at `deadline`. `gen` is the process's park
/// generation (stamped by the caller in `wait_for_message`) — the timer fires the
/// wakeup only while it's still current, giving lazy cancellation of superseded
/// deadlines. Lazily starts the timer thread on first use (programs that never use a
/// `receive` timeout never spawn it). On wasm there is no timer thread — the cooperative
/// scheduler pump fires due timers itself (`fire_next_timer`); we only record the deadline.
pub(super) fn arm_timer(pid: u64, deadline: Instant, gen: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    ensure_timer_thread();
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    q.push(Reverse((deadline, pid, gen)));
    if q.len() >= compact_threshold() {
        compact(&mut q);
    }
    drop(q);
    cv.notify_one();
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
    let kept: Vec<Reverse<(Instant, u64, u64)>> = std::mem::take(q)
        .into_vec()
        .into_iter()
        .filter(|Reverse((_, pid, gen))| {
            *gens
                .entry(*pid)
                .or_insert_with(|| super::mailbox::current_timer_gen(*pid))
                == Some(*gen)
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

/// Fire the earliest pending receive-timeout, waking its parked process (wasm cooperative
/// scheduler — there is no timer thread). Called by the pump when nothing else is runnable:
/// the earliest deadline is then the only thing that can advance the program, so it fires in
/// logical time (real delays aren't honored under wasm — fine for a playground). Messages
/// still win over timeouts, since the pump drains all run queues before firing a timer. A
/// superseded entry (its `gen` advanced) wakes nothing but is still removed. Returns whether
/// an entry was fired.
// The scheduler's clock for receive deadlines. On native this is the real monotonic
// clock. On wasm there is no timer thread, so real time never advances *while a snippet
// runs* — `fire_next_timer` advances this LOGICAL clock to the fired deadline instead, so a
// woken receive sees its deadline as reached and takes its `after` clause at once (rather
// than re-checking `Instant::now()`, finding almost no real time passed, and re-parking —
// which spun the pump at 100% CPU for the full real delay, freezing the tab).
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn sched_now() -> Instant {
    Instant::now()
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LOGICAL_NOW: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

#[cfg(target_arch = "wasm32")]
pub(super) fn sched_now() -> Instant {
    LOGICAL_NOW.with(|c| {
        let now = c.get().unwrap_or_else(Instant::now);
        c.set(Some(now));
        now
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
        Some(Reverse((deadline, pid, gen))) => {
            LOGICAL_NOW.with(|c| {
                if c.get().is_none_or(|now| deadline > now) {
                    c.set(Some(deadline));
                }
            });
            super::mailbox::wake_for_timeout(pid, gen);
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
        eprintln!(
            "brood: cannot start the timer thread ({e}); timeouts are delayed until one starts"
        );
    }
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
#[cfg(not(target_arch = "wasm32"))]
fn timer_loop() {
    TIMER_THREADS_STARTED.fetch_add(1, Ordering::Relaxed);
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    loop {
        match q.peek().copied() {
            // `unwrap_or_else(into_inner)`, not `unwrap`: a poisoned condvar wait would
            // otherwise kill the timer thread and with it every deadline in the runtime —
            // exactly the cascade `core/sync.rs` exists to refuse (KI-97 item 3).
            None => q = cv.wait(q).unwrap_or_else(|e| e.into_inner()),
            Some(Reverse((deadline, _, _))) => {
                let now = Instant::now();
                if now < deadline {
                    q = cv
                        .wait_timeout(q, deadline - now)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                } else {
                    let mut due = Vec::new();
                    while let Some(&Reverse((d, pid, gen))) = q.peek() {
                        if d <= now {
                            q.pop();
                            due.push((pid, gen));
                        } else {
                            break;
                        }
                    }
                    drop(q);
                    // `wake_for_timeout` itself drops a superseded entry (gen no
                    // longer current), so we needn't filter here.
                    for (pid, gen) in due {
                        super::mailbox::wake_for_timeout(pid, gen);
                    }
                    q = crate::core::sync::lock(lock);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// KI-97 item 3: starting the timer thread must be **retryable**, not a `Once`.
    ///
    /// `Once::call_once` is poisoned by a panic inside it, and `std::thread::spawn` panics
    /// when the OS refuses a thread (EAGAIN). Because `arm_timer` backs `sleep` and every
    /// `receive … (after ms …)`, one transient refusal used to break *all* timeouts in the
    /// runtime, permanently and unrecoverably.
    ///
    /// The OS cannot be made to refuse a thread on demand here, so this asserts the
    /// property that makes recovery possible: the started-flag is a plain CAS that a
    /// failed spawn releases, so `ensure_timer_thread` is safe to call repeatedly and
    /// leaves the flag set once a thread is actually running.
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
        arm_timer(
            u64::MAX,
            Instant::now() + std::time::Duration::from_secs(3600),
            0,
        );
        assert!(
            crate::core::sync::lock(lock).len() > before,
            "arm_timer must still queue a deadline"
        );
    }

    use super::*;

    /// Superseded timer entries must be **compacted**, not merely reaped at their
    /// deadlines.
    ///
    /// Lazy cancellation stamps each entry with the park generation it was armed under and
    /// drops a stale one when its deadline comes due. That bounds an entry's *lifetime* by
    /// the deadline horizon — but the heap's *size* is then bounded by `arm-rate × horizon`,
    /// and the arm rate is the message rate: a gen-server looping
    /// `(receive … (after 3600000 …))` at 10k msg/s accrues ~36M dead entries (GB-scale, on
    /// one global heap behind one mutex) before the first one is due, then reaps them in a
    /// burst. Here every entry is stale by construction (the pid was never registered, so
    /// `current_timer_gen` is `None`) with a deadline an hour out, so nothing can be reaped
    /// by firing; only compaction can keep the heap bounded.
    #[test]
    fn superseded_entries_are_compacted_not_left_until_their_deadline() {
        let far = Instant::now() + std::time::Duration::from_secs(3600);
        let dead_pid = u64::MAX - 7; // never in the process REGISTRY
        let armed = 4000;
        for gen in 0..armed {
            arm_timer(dead_pid, far, gen);
        }
        let pending = pending_timer_count();
        assert!(
            pending < armed as usize / 2,
            "{pending} of {armed} superseded entries still pending — compaction did not run"
        );
    }
}
