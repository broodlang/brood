//! The busy cursor (ADR-491): a window shows the wait cursor while the loop that takes its
//! input is stuck in one turn — asked locally, from an input until the loop is idle again,
//! or told by a thin client's daemon — and puts the hover shape back when the turn ends.

use super::*;

/// How often a window asks whether its app's loop is busy, from an input until the loop is
/// idle again (ADR-491): the busy cursor appears at most this long after the loop's quit
/// key would interrupt it, and goes as soon after the turn ends.
pub(super) const BUSY_PROBE: Duration = Duration::from_millis(100);

/// The busy cursor (ADR-491): while the loop that takes this window's input has been in one
/// `update` or `view` past its quit key's busy-ms, show the wait cursor — the C-g that would
/// stop it is live — and put the hover shape back when the turn ends. Asked only from an
/// input until the loop is idle again (`process::interrupt_busy` answers None), so it costs
/// a window nothing at rest. When to ask next, if at all.
pub(super) fn busy_cursor(w: &mut Win, now: Instant) -> Option<Instant> {
    let due = w.busy_check?;
    if now < due {
        return Some(due);
    }
    let busy = crate::process::interrupt_busy(w.subscriber);
    w.busy_local = busy == Some(true);
    show_busy(w);
    // idle (no window open): stop asking until the next input; a busy cursor showing is
    // never left behind, since that answer also cleared it above
    w.busy_check = busy.map(|_| now + BUSY_PROBE);
    w.busy_check
}

/// Show the wait cursor while the loop is busy — the one this window asks (`busy_local`) or
/// a thin client's daemon, which says so over the link (`busy_remote`) — and the hover
/// shape otherwise. Only a change calls `set_cursor`.
pub(super) fn show_busy(w: &mut Win) {
    let show = w.busy_local || w.busy_remote;
    if show != w.busy_shown {
        w.busy_shown = show;
        w.window.set_cursor(if show {
            CursorIcon::Wait
        } else {
            w.shape.map(cursor_icon).unwrap_or(CursorIcon::Default)
        });
    }
}
