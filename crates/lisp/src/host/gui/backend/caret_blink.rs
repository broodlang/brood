//! A window's blinking caret (ADR-469): the clock the window keeps for the frame's
//! `[:cursor … blink]`, which the window toggles itself between frames, and how an input
//! or a focus change wakes it.

use super::*;

/// A window's blinking caret (ADR-469): the `Op::Cursor` that asked to blink, how, and
/// when its current run of blinks began. `steady` holds it shown — a window without
/// focus does not blink, as Emacs's unselected frames do not.
pub(super) struct CaretBlink {
    pub(super) caret: Op,
    spec: crate::host::gui::CursorBlink,
    since: Instant,
    steady: bool,
}

impl CaretBlink {
    /// The clock for `caret`, starting now, shown; `None` for an op that does not blink.
    pub(super) fn of(caret: &Op) -> Option<CaretBlink> {
        match caret {
            Op::Cursor {
                blink: Some(spec), ..
            } => Some(CaretBlink {
                caret: caret.clone(),
                spec: *spec,
                since: Instant::now(),
                steady: false,
            }),
            _ => None,
        }
    }

    /// Whether the caret is in its hidden half at `now`.
    pub(super) fn hidden(&self, now: Instant) -> bool {
        !self.steady
            && self
                .spec
                .hidden_after(now.saturating_duration_since(self.since))
    }

    /// When it next changes visibility after `now`, or `None` while it is held still or
    /// has blinked its `blinks`.
    pub(super) fn next_toggle(&self, now: Instant) -> Option<Instant> {
        if self.steady {
            return None;
        }
        self.spec.next_toggle(self.since, now)
    }
}

/// The first blinking caret of `ops`, walking into the regions and fragments (where a
/// pane's caret is).
pub(super) fn blinking_caret(ops: &[Op]) -> Option<&Op> {
    ops.iter().find_map(|op| match op {
        Op::Cursor { blink: Some(_), .. } => Some(op),
        Op::ScrollRegion { ops, .. } | Op::CellRegion { ops, .. } | Op::AboveSeam { ops } => {
            blinking_caret(ops)
        }
        Op::Fragment(fragment) => blinking_caret(&fragment.0),
        _ => None,
    })
}

/// Show `w`'s blinking caret now and restart its blinks — `blinking` false holds it
/// shown until the next wake (focus lost). A caret that was hidden is repainted.
pub(super) fn wake_caret(w: &mut Win, blinking: bool) {
    if let Some(clock) = &mut w.caret_blink {
        if w.renderer.caret_hidden {
            w.window.request_redraw();
        }
        clock.since = Instant::now();
        clock.steady = !blinking;
    }
}
