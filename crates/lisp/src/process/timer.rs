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
//! **Lazy cancellation.** Entries are never removed from the heap when a park
//! is superseded — a `(receive … (after ms …))` woken by `send` each iteration
//! would otherwise churn arm/disarm pairs. Instead each entry carries the park
//! **generation** it was armed under; `wake_for_timeout` drops an entry whose gen
//! the mailbox has since advanced past (see `Mailbox::timer_gen`). So the heap can
//! briefly hold superseded entries, but they're reaped at their deadline and fire
//! no spurious wakeup — growth stays bounded by the deadline horizon.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Condvar, LazyLock, Mutex, Once};
use web_time::Instant;

/// Min-heap of `(deadline, pid, gen)`: `Reverse` turns the max-heap into
/// earliest-first. `gen` is the parking process's [`Mailbox::timer_gen`] at arm
/// time — carried so the timer thread can detect (and skip) a superseded deadline.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
#[cfg(not(target_arch = "wasm32"))]
static TIMER_STARTED: Once = Once::new();

/// Arrange to wake green process `pid` at `deadline`. `gen` is the process's park
/// generation (stamped by the caller in `wait_for_message`) — the timer fires the
/// wakeup only while it's still current, giving lazy cancellation of superseded
/// deadlines. Lazily starts the timer thread on first use (programs that never use a
/// `receive` timeout never spawn it). On wasm there is no timer thread — the cooperative
/// scheduler pump fires due timers itself (`fire_next_timer`); we only record the deadline.
pub(super) fn arm_timer(pid: u64, deadline: Instant, gen: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    TIMER_STARTED.call_once(|| {
        std::thread::spawn(timer_loop);
    });
    let (lock, cv) = &*TIMERS;
    crate::core::sync::lock(lock).push(Reverse((deadline, pid, gen)));
    cv.notify_one();
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

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
#[cfg(not(target_arch = "wasm32"))]
fn timer_loop() {
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    loop {
        match q.peek().copied() {
            None => q = cv.wait(q).unwrap(),
            Some(Reverse((deadline, _, _))) => {
                let now = Instant::now();
                if now < deadline {
                    q = cv.wait_timeout(q, deadline - now).unwrap().0;
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
