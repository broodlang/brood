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
use std::time::Instant;

/// Min-heap of `(deadline, pid, gen)`: `Reverse` turns the max-heap into
/// earliest-first. `gen` is the parking process's [`Mailbox::timer_gen`] at arm
/// time — carried so the timer thread can detect (and skip) a superseded deadline.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
static TIMER_STARTED: Once = Once::new();

/// Arrange to wake green process `pid` at `deadline`. `gen` is the process's park
/// generation (stamped by the caller in `wait_for_message`) — the timer fires the
/// wakeup only while it's still current, giving lazy cancellation of superseded
/// deadlines. Lazily starts the timer thread on first use (programs that never use a
/// `receive` timeout never spawn it).
pub(super) fn arm_timer(pid: u64, deadline: Instant, gen: u64) {
    TIMER_STARTED.call_once(|| {
        std::thread::spawn(timer_loop);
    });
    let (lock, cv) = &*TIMERS;
    crate::core::sync::lock(lock).push(Reverse((deadline, pid, gen)));
    cv.notify_one();
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
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
