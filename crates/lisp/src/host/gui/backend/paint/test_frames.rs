//! The pieces the painter's frame tests build their frames from and paint them with: shared
//! by the exactness tests (`incremental_tests`) and the clock readings (`timings`).

use super::*;
use crate::host::gui::{Face, Op};

/// A face of foreground `fg`, the rest default.
pub(super) fn face(fg: [u8; 3]) -> Face {
    Face {
        fg: Some(fg),
        ..Face::default()
    }
}

/// Text `s` at cell `(row, col)` in `fg`.
pub(super) fn text_in(row: u16, col: u16, s: &str, fg: [u8; 3]) -> Op {
    Op::Text {
        row,
        col,
        s: s.into(),
        face: face(fg),
    }
}

/// A `w`×`h`-cell rect at `(row, col)` filled with `bg`: a current-line band, a divider, a
/// mode line's ground.
pub(super) fn band(row: u16, col: u16, w: u16, h: u16, bg: [u8; 3]) -> Op {
    Op::Rect {
        row,
        col,
        w,
        h,
        face: Face {
            bg: Some(bg),
            ..Face::default()
        },
        radius: 0.0,
    }
}

/// The grid a `w`×`h` window holds at `r`'s metrics, as `(columns, rows)`.
pub(super) fn grid(r: &Renderer, w: usize, h: usize) -> (usize, usize) {
    let (columns, rows) = super::super::bleed::grid_cells(r, w, h);
    (columns as usize, rows as usize)
}

/// Paint `frame` at `size` as the window does: its layout taken first (`set_layout`), then
/// rasterised, incrementally unless `force`.
pub(super) fn paint_at(
    r: &mut Renderer,
    frame: &[Op],
    (width, height): (usize, usize),
    force: bool,
) {
    r.set_layout(super::super::rows::frame_layout(frame));
    raster_frame(r, frame, width, height, force);
}

/// One line's spans `(column, text, fg)` — columns of the line, text ASCII so a char is a cell —
/// as an unwrapped pane scrolled `hscroll` columns left draws them on `row`: each span cut to
/// the columns that show, `[hscroll, hscroll + width)`, and placed from the pane's text column
/// `left`.
pub(super) fn scrolled_sideways(
    row: u16,
    spans: &[(usize, String, [u8; 3])],
    hscroll: usize,
    (left, width): (usize, usize),
    out: &mut Vec<Op>,
) {
    for (column, text, fg) in spans {
        let start = (*column).max(hscroll);
        let end = (column + text.len()).min(hscroll + width);
        if end > start {
            out.push(text_in(
                row,
                (left + start - hscroll) as u16,
                &text[start - column..end - column],
                *fg,
            ));
        }
    }
}
