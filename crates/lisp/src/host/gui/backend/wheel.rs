//! The mouse wheel's scrolling: the calibration of a notch's kinetic glide (its impulse,
//! decay and streak boost), the trackpad's coast decay, and the `{:wheel :lines}` backlog
//! that goes out as one scroll message per drawn frame. The event loop's `MouseWheel` arm
//! and `about_to_wait` drive it.

use super::*;

/// The velocity a wheel notch of `dy` (±1 per notch on most mice) leaves the kinetic
/// scroll with, on top of the `carried` velocity of a glide already running the same
/// way: the notch's impulse, boosted by its place in a `streak` of quick notches.
pub(super) fn wheel_velocity(carried: f64, streak: u32, dy: f64) -> f64 {
    let boost = (1.0 + WHEEL_STREAK_BOOST * streak as f64).min(WHEEL_STREAK_CAP);
    carried + WHEEL_IMPULSE * boost * dy.signum() * dy.abs().max(1.0)
}

/// Where a glide starting at `v0` lines per tick ends up, in lines, decaying at `decay`
/// per tick until the `about_to_wait` cutoff — the geometric sum, stepped as the ticker
/// steps it (the test's oracle for the calibration constants).
#[cfg(test)]
pub(super) fn glide_distance(mut v: f64, decay: f64) -> f64 {
    let mut total = 0.0;
    while v.abs() >= 0.0005 {
        total += v;
        v *= decay;
    }
    total
}

/// Kinetic scrolling (`about_to_wait`): the velocity decay per nominal 12 ms tick. A
/// trackpad flick coasts long; a wheel notch glides for about half a second.
pub(super) const TRACKPAD_DECAY: f64 = 0.97;
pub(super) const WHEEL_DECAY: f64 = 0.85;
/// Lines one wheel notch scrolls in total, as the geometric sum of its glide:
/// `WHEEL_IMPULSE / (1 - WHEEL_DECAY)` — 0.45 per tick decaying at 0.85 is 3 lines.
pub(super) const WHEEL_IMPULSE: f64 = 0.45;
/// A notch within this many ms of the previous one joins a streak; each notch of a
/// streak scrolls `1 + WHEEL_STREAK_BOOST × streak` as far, up to `WHEEL_STREAK_CAP` —
/// the slight acceleration of a spun wheel, without a flung page.
pub(super) const WHEEL_STREAK_MS: u128 = 220;
pub(super) const WHEEL_STREAK_BOOST: f64 = 0.18;
pub(super) const WHEEL_STREAK_CAP: f64 = 2.2;
/// Lines one wheel notch scrolls under `{:wheel :lines}` — the distance a glide covers
/// too, so switching modes changes how a notch moves, not how far.
pub(super) const WHEEL_LINES: f64 = 3.0;
/// How long a `{:wheel :lines}` scroll waits for the app to draw the last one before the
/// backlog goes anyway — a raw loop that never draws must not lose its wheel.
pub(super) const WHEEL_HOLD: Duration = Duration::from_millis(100);

/// Deliver a `{:wheel :lines}` window's wheel backlog as ONE scroll message, unless the
/// last one is still undrawn (and younger than `WHEEL_HOLD`) — then it waits and keeps
/// summing. True when nothing is left waiting.
pub(super) fn flush_wheel(w: &mut Win, now: Instant) -> bool {
    if w.wheel_backlog == 0.0 {
        return true;
    }
    let undrawn = w.scroll_pending
        && w.wheel_sent
            .is_some_and(|sent| now.duration_since(sent) < WHEEL_HOLD);
    if undrawn {
        return false;
    }
    deliver_scroll(w, w.wheel_backlog);
    w.wheel_backlog = 0.0;
    w.scroll_pending = true;
    w.wheel_sent = Some(now);
    true
}
