//! The display doing less than it was asked (ADR-636): a fallback that keeps the window up
//! is reported to the app as `[:display-degraded {:what … :reason …}]`, to the window's own
//! app, every open window's, or — with none open yet — the next window opened.

use super::*;

// ---- the display doing less than it was asked (ADR-636) ---------------------------------
// A fallback that keeps the window up — the CPU painter for a GPU that would not start, the
// bundled DejaVu for a font that is not installed, grey text for a driver that rejects
// subpixel text — used to say so only on stderr, which a desktop launch throws away. Now the
// app is told, as `[:display-degraded {:what :gpu :reason "…"}]` in its mailbox like any
// other input, so an editor can say it. A window's own fallback goes to its app; a
// runtime-wide one (a font) to every open window's, or, with none open yet, to the next
// window opened — fonts are registered before the first window.

/// What has degraded and nobody has been told yet: no window was open when it happened.
pub(super) fn undelivered_degradations() -> &'static Mutex<Vec<Message>> {
    static D: OnceLock<Mutex<Vec<Message>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(Vec::new()))
}

/// `[:display-degraded {:what :<what> :reason "<reason>"}]`.
fn degraded_message(what: &str, reason: &str) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("display-degraded")),
        Message::Map(vec![
            (
                Message::Keyword(value::intern("what")),
                Message::Keyword(value::intern(what)),
            ),
            (
                Message::Keyword(value::intern("reason")),
                Message::Str(reason.to_string()),
            ),
        ]),
    ])
}

/// The display could not do `what` as asked and fell back (`reason` says to what). Tell
/// `to` (the window's app), or with `None` every open window's app, or the next window's.
/// Also logged to stderr, as before. Safe from any thread.
pub(in crate::host::gui) fn report_degraded(what: &str, reason: &str, to: Option<u64>) {
    eprintln!("brood: gui: {what} degraded: {reason}");
    if let Some(pid) = to {
        deliver(pid, degraded_message(what, reason));
        return;
    }
    // Under the undelivered list's lock, so a window opening now either is in `windows()`
    // already or drains the list after this pushes to it (`open` inserts, then drains).
    let mut undelivered = lock(undelivered_degradations());
    let pids = lost_subscribers(&lock(windows()));
    if pids.is_empty() {
        undelivered.push(degraded_message(what, reason));
    }
    for pid in pids {
        deliver(pid, degraded_message(what, reason));
    }
}

/// A window just opened for `subscriber`: hand it what degraded before any window was open.
pub(super) fn deliver_undelivered_degradations(subscriber: u64) {
    let pending = std::mem::take(&mut *lock(undelivered_degradations()));
    for message in pending {
        deliver(subscriber, message);
    }
}
