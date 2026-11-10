//! Headless mode (`BROOD_GUI_HEADLESS`): whether this runtime opens real OS windows at all,
//! whether its headless windows are painted offscreen (ADR-609), and the cell grid and
//! pixel size a headless window reports.

use super::*;

/// HEADLESS mode (`BROOD_GUI_HEADLESS=1`): a gui-built runtime that opens NO real
/// OS window — `gui-open` hands back a fake window with a fixed cell grid, every
/// draw/window op is a silent no-op, and no key/mouse events ever arrive. A
/// windowed app's loop runs unchanged (paced by its own `(after)` timer), so it
/// can be tested / soak-run / CI'd with no popup. Read once at first use. Public as
/// `gui/headless?` (ADR-557): a question with no font engine behind it — `gui-font-info`,
/// `gui-cell-size` — errors headless, so an app asks this before asking it.
pub fn headless() -> bool {
    static H: OnceLock<bool> = OnceLock::new();
    *H.get_or_init(|| {
        std::env::var("BROOD_GUI_HEADLESS")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

/// HEADLESS PAINTING (`BROOD_GUI_HEADLESS=paint`, ADR-609): headless as above — no OS
/// window, no input — but every window is painted offscreen by the window's own CPU
/// painter (`offscreen`): it keeps its font, inset, background and the frames drawn to
/// it, reports the grid the real cell metrics give, and `gui/capture` paints its frame.
/// What lets a test or an agent look at an app's real loop with no display. Read once.
pub(super) fn headless_paint() -> bool {
    static P: OnceLock<bool> = OnceLock::new();
    *P.get_or_init(|| {
        headless() && std::env::var("BROOD_GUI_HEADLESS").is_ok_and(|v| v.trim() == "paint")
    })
}

/// Headless: hand the request `make` builds to the offscreen painter when windows are
/// painted headless (ADR-609); drop it otherwise, as every headless window op is.
pub(super) fn headless_send(make: impl FnOnce() -> offscreen::Request) -> Result<(), String> {
    if headless_paint() {
        offscreen::send(make())
    } else {
        Ok(())
    }
}

/// The cell grid a headless window reports for a requested logical pixel `size`
/// (default 840×560 like a real `gui-open`), using a nominal 8×16 px cell.
pub(super) fn headless_cells(size: Option<(f64, f64)>) -> (u16, u16) {
    let (w, h) = size.unwrap_or((840.0, 560.0));
    (((w / 8.0) as u16).max(1), ((h / 16.0) as u16).max(1))
}

/// The pixel size a headless window reports for a requested logical `size` (default
/// 840×560), at a nominal scale of 1.
pub(super) fn headless_px(size: Option<(f64, f64)>) -> (u32, u32) {
    let (w, h) = size.unwrap_or((840.0, 560.0));
    ((w.max(1.0)) as u32, (h.max(1.0)) as u32)
}
