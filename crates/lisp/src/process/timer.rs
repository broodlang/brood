//! Receive-timeout (`(after ms expr)`) machinery: a min-heap of pending
//! deadlines + one OS thread that wakes parked green processes at their
//! due times.
//!
//! Hooks into the scheduler via [`super::wake_for_timeout`] — the one piece
//! of mailbox plumbing this module needs. That helper takes a pid, finds
//! its mailbox in `REGISTRY`, and re-queues the parked waiter (if any) so
//! it wakes, re-scans its mailbox, and notices the deadline has passed.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{Condvar, LazyLock, Mutex, Once};
use std::time::Instant;

/// Min-heap of `(deadline, pid)`: `Reverse` turns the max-heap into earliest-first.
type TimerQueue = BinaryHeap<Reverse<(Instant, u64)>>;

/// Pending `receive` deadlines for green processes. A dedicated thread wakes each at
/// its deadline so it can fire its `after` clause.
static TIMERS: LazyLock<(Mutex<TimerQueue>, Condvar)> =
    LazyLock::new(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
static TIMER_STARTED: Once = Once::new();

/// Arrange to wake green process `pid` at `deadline`. Lazily starts the timer thread
/// on first use (programs that never use a `receive` timeout never spawn it).
pub(super) fn arm_timer(pid: u64, deadline: Instant) {
    TIMER_STARTED.call_once(|| {
        std::thread::spawn(timer_loop);
    });
    let (lock, cv) = &*TIMERS;
    crate::core::sync::lock(lock).push(Reverse((deadline, pid)));
    cv.notify_one();
}

/// Sleep until the nearest deadline, then wake every process whose deadline passed.
fn timer_loop() {
    let (lock, cv) = &*TIMERS;
    let mut q = crate::core::sync::lock(lock);
    loop {
        match q.peek().copied() {
            None => q = cv.wait(q).unwrap(),
            Some(Reverse((deadline, _))) => {
                let now = Instant::now();
                if now < deadline {
                    q = cv.wait_timeout(q, deadline - now).unwrap().0;
                } else {
                    let mut due = Vec::new();
                    while let Some(&Reverse((d, pid))) = q.peek() {
                        if d <= now {
                            q.pop();
                            due.push(pid);
                        } else {
                            break;
                        }
                    }
                    drop(q);
                    for pid in due {
                        super::wake_for_timeout(pid);
                    }
                    q = crate::core::sync::lock(lock);
                }
            }
        }
    }
}
