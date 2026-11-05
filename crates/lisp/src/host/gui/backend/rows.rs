//! THE ROW MAP (ADR-610): where a grid row lands in the window, in one place.
//!
//! The grid is uniform in cells: an app lays every op out in whole rows and never sees a
//! pixel. The window is rarely a whole number of rows tall, so something has to take the
//! leftover pixels (the window less its margins, mod one row). Which rows sit above that
//! leftover and which below it is the whole layout question, and it is answered here:
//!
//! - **Rows above the seam are anchored to the top.** Row 0 starts at `top`: the window's
//!   top edge when row 0 is a band (a header line, a tab bar), the inset below it when not.
//! - **Rows from the seam down are anchored to the bottom.** The last grid row ends exactly
//!   at the bottom inset, whatever the window's height — an editor's echo area never floats
//!   off the window's bottom edge.
//! - **The leftover sits at the seam**, between row `seam - 1` and row `seam`, and belongs
//!   to the row above it: a background whose bottom edge is the seam reaches down through
//!   it, and the hit-test gives its pixels to row `seam - 1`. An editor declares its bottom
//!   mode line as the seam, so the leftover is the text body's partial line, as Emacs and
//!   VS Code give it to the text.
//! - **No seam: the leftover sits under the last row**, which is the layout every frame
//!   without an `Op::Seam` gets.
//!
//! Every pixel↔row conversion goes through [`RowMap`]: both painters, the edge bleed, the
//! CPU painter's damage strips and scroll blits, the offscreen picture and the mouse
//! hit-test. A row's top is [`RowMap::y`]; a span of rows is `y(end) - y(start)`, so a span
//! that ends at the seam covers the leftover too and a span that starts there begins below
//! it. The inverse is [`RowMap::row_at`]. Nothing else computes `origin + row * cell_h`.

/// The rows of one grid as window pixels: `top`, the cell height `ch`, and the `gap`
/// (the leftover) inserted before row `seam`. A grid without a seam has `gap == 0`, and
/// is then the plain `top + row * ch`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct RowMap {
    /// The pixel row grid row 0 starts at.
    pub(crate) top: usize,
    /// The cell height in physical pixels (at least 1).
    pub(crate) ch: usize,
    /// The first bottom-anchored row; `u16::MAX` for none.
    seam: u16,
    /// The leftover pixels between row `seam - 1` and row `seam`.
    gap: usize,
}

impl RowMap {
    /// A grid with no seam: rows from `top`, `ch` apart. What a cell region's own grid is
    /// (it is laid out inside its rect, which the window's map already placed).
    pub(crate) fn uniform(top: usize, ch: usize) -> RowMap {
        RowMap {
            top,
            ch: ch.max(1),
            seam: u16::MAX,
            gap: 0,
        }
    }

    /// The map of a window `h_px` tall holding `rows` rows of `ch` pixels from `top`, with
    /// `inset` pixels of margin under the last row, and the frame's `seam`. The leftover is
    /// `h_px - inset - top - rows * ch`; a seam inside the grid puts it before that row, a
    /// seam of 0 (every row bottom-anchored) above row 0, and no seam (or one at or past
    /// `rows`) under the last row, where it always was.
    pub(crate) fn window(
        top: usize,
        ch: usize,
        rows: u16,
        inset: usize,
        h_px: usize,
        seam: Option<u16>,
    ) -> RowMap {
        let ch = ch.max(1);
        let gap = h_px
            .saturating_sub(inset)
            .saturating_sub(top + rows as usize * ch);
        match seam {
            Some(0) => RowMap::uniform(top + gap, ch),
            Some(seam) if seam < rows && gap > 0 => RowMap { top, ch, seam, gap },
            _ => RowMap::uniform(top, ch),
        }
    }

    /// The map as plain words — `[top, ch, seam, gap]` — for a cache key (the GPU fragment
    /// cache: a fragment expanded under another map landed on other pixels).
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn key(&self) -> [u32; 4] {
        [
            self.top as u32,
            self.ch as u32,
            self.seam as u32,
            self.gap as u32,
        ]
    }

    /// The pixel row grid row `row` starts at — and, the same number, where row `row - 1`
    /// ends. THE row→pixel rule: `top + row * ch`, plus the gap from the seam down.
    /// Saturating, since a frame is app data and a row can be anything.
    pub(crate) fn y(&self, row: isize) -> isize {
        let shift = if self.gap > 0 && row >= self.seam as isize {
            self.gap as isize
        } else {
            0
        };
        (self.top as isize)
            .saturating_add(row.saturating_mul(self.ch as isize))
            .saturating_add(shift)
    }

    /// [`RowMap::y`] at a fractional row (an `FRect`'s edge, a scroll region's glide): the
    /// same rule, so an edge at exactly the seam is below the gap and one short of it is
    /// above.
    pub(crate) fn yf(&self, row: f32) -> f32 {
        let shift = if self.gap > 0 && row >= self.seam as f32 {
            self.gap as f32
        } else {
            0.0
        };
        self.top as f32 + row * self.ch as f32 + shift
    }

    /// The pixel height of the rows `[row, row + n)`: `y(row + n) - y(row)`, which takes in
    /// the gap when the span runs into the seam from above.
    pub(crate) fn span(&self, row: isize, n: isize) -> isize {
        self.y(row.saturating_add(n)).saturating_sub(self.y(row))
    }

    /// [`RowMap::span`] at fractional rows.
    pub(crate) fn spanf(&self, row: f32, n: f32) -> f32 {
        self.yf(row + n) - self.yf(row)
    }

    /// THE INVERSE: the fractional row under window pixel row `py`, clamped at 0 — so
    /// `floor` of it is the row painted there. Each row is `[y(k), y(k + 1))`, and the
    /// fraction is the position inside that, so the gap's pixels are row `seam - 1`
    /// (whose backgrounds paint them), its fraction running over `ch + gap` pixels.
    pub(crate) fn row_at(&self, py: f64) -> f64 {
        let ch = self.ch as f64;
        let rel = (py - self.top as f64).max(0.0);
        if self.gap == 0 {
            return rel / ch;
        }
        let seam = self.seam as f64;
        let above = seam * ch; // pixels from `top` to the gap
        if rel < above - ch {
            rel / ch
        } else if rel < above + self.gap as f64 {
            // row `seam - 1`, stretched over the gap
            (seam - 1.0) + (rel - (above - ch)) / (ch + self.gap as f64)
        } else {
            (rel - self.gap as f64) / ch
        }
    }

    /// The row painted at window pixel row `py` (`floor` of [`RowMap::row_at`]), clamped to
    /// `u16`.
    pub(crate) fn row_of(&self, py: f64) -> u16 {
        self.row_at(py).floor().min(u16::MAX as f64) as u16
    }

    /// A pixel position in the grid's UNIFORM space (`row * ch` from row 0, no gap: where
    /// cursor zones and the caret are measured) as a window pixel row.
    pub(crate) fn window_y(&self, uniform: isize) -> isize {
        let ch = self.ch as isize;
        let row = uniform.div_euclid(ch);
        self.y(row).saturating_add(uniform.rem_euclid(ch))
    }

    /// The inverse of [`RowMap::window_y`]: window pixel row `py` in the grid's uniform
    /// space, the gap folded into row `seam - 1` as [`RowMap::row_at`] folds it. Above row
    /// 0 it is negative (a pointer in the top margin is over no zone).
    pub(crate) fn uniform_y(&self, py: f64) -> f64 {
        if py < self.top as f64 {
            py - self.top as f64
        } else {
            self.row_at(py) * self.ch as f64
        }
    }
}

/// The layout a frame declares: its first top-level `Op::Seam`'s row and top band, or no
/// seam and row 0 a band (the layout of a frame without one). Top level only: `%gui-draw`
/// splices a memoised fragment that holds one (`fragments::frame_global`), and inside a
/// region a seam would mean nothing.
pub(crate) fn frame_layout(frame: &[crate::host::gui::Op]) -> (Option<u16>, bool) {
    frame
        .iter()
        .find_map(|op| match op {
            crate::host::gui::Op::Seam { row, top_band } => Some((*row, *top_band)),
            _ => None,
        })
        .unwrap_or((None, true))
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 10 rows of 20 px under a 6 px inset in a 219 px window, the seam at row 8: the
    /// leftover is 219 - 6 - 200 = 13 px.
    fn seamed() -> RowMap {
        RowMap::window(0, 20, 10, 6, 219, Some(8))
    }

    #[test]
    fn without_a_seam_rows_are_top_anchored_and_the_leftover_is_below() {
        let rows = RowMap::window(0, 20, 10, 6, 219, None);
        assert_eq!(rows.gap, 0);
        assert_eq!(rows.y(0), 0);
        assert_eq!(rows.y(10), 200);
        // a seam at or past the grid is the same layout
        assert_eq!(RowMap::window(0, 20, 10, 6, 219, Some(10)), rows);
    }

    #[test]
    fn the_last_row_ends_on_the_bottom_inset_and_the_gap_sits_at_the_seam() {
        let rows = seamed();
        assert_eq!((rows.seam, rows.gap), (8, 13));
        assert_eq!(rows.y(7), 140, "above the seam, top-anchored");
        assert_eq!(rows.y(8), 173, "row 8 starts after the 13 px gap");
        assert_eq!(rows.y(10), 219 - 6, "the last row ends on the bottom inset");
        // a span into the seam covers the gap; one from it does not
        assert_eq!(rows.span(7, 1), 33);
        assert_eq!(rows.span(8, 1), 20);
        assert_eq!(rows.span(0, 10), 213);
        assert_eq!(rows.yf(7.5), 150.0);
        assert_eq!(rows.yf(8.0), 173.0);
    }

    #[test]
    fn a_seam_of_zero_anchors_every_row_to_the_bottom() {
        let rows = RowMap::window(0, 20, 10, 6, 219, Some(0));
        assert_eq!(rows.y(0), 13);
        assert_eq!(rows.y(10), 213);
    }

    #[test]
    fn the_inverse_gives_every_pixel_to_the_row_painted_there() {
        let rows = seamed();
        for k in 0..10isize {
            let (top, bottom) = (rows.y(k), rows.y(k + 1));
            for py in top..bottom {
                assert_eq!(rows.row_of(py as f64 + 0.5), k as u16, "pixel {py}");
            }
            let middle = (top + bottom) as f64 / 2.0;
            assert_eq!(rows.row_of(middle), k as u16);
        }
        // the fraction is continuous across the stretched row
        assert!((rows.row_at(140.0) - 7.0).abs() < 1e-9);
        assert!((rows.row_at(173.0) - 8.0).abs() < 1e-9);
        assert!(rows.row_at(172.9) < 8.0);
    }

    #[test]
    fn uniform_space_round_trips_through_the_window() {
        let rows = seamed();
        assert_eq!(rows.window_y(0), 0);
        assert_eq!(rows.window_y(8 * 20), 173);
        assert_eq!(rows.window_y(8 * 20 + 5), 178);
        assert_eq!(rows.uniform_y(178.0), 165.0);
        assert_eq!(rows.uniform_y(20.0), 20.0);
    }

    #[test]
    fn wild_rows_saturate() {
        let rows = seamed();
        assert_eq!(rows.y(isize::MAX), isize::MAX);
        assert_eq!(rows.y(isize::MIN), isize::MIN);
        let _ = rows.span(isize::MAX, isize::MAX);
    }
}
