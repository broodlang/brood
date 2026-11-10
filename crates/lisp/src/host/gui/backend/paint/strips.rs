//! The incremental half of a paint: what changed between two frames. Per cell row, `strip_diff`
//! says which rows' ops differ, `strip_columns` narrows a row to the columns its changed ops can
//! paint, and `strip_blits` finds rows that only moved, so their pixels are copied rather than
//! drawn. Pure over the ops and the row map: nothing here touches a pixel.

use super::*;

// ---- the strip diff ----------------------------------------------------------

/// The signed pixel-row band `[top, bottom)` an op paints, before clipping, given the
/// grid's row map `rows` (ADR-610) and the scroll shift `dy` in effect where it
/// sits; `None` for an op that paints nothing (a cursor zone). Conservative by
/// construction — a band may be wider than the pixels actually touched, never
/// narrower — since it decides which rows a change re-rasterises. A `Clear` is the
/// whole framebuffer; a `ScrollRegion` is not an entry (its children are, each with
/// its shift), so it never gets here.
pub(super) fn op_band(op: &Op, dy: isize, rows: &RowMap) -> Option<(isize, isize)> {
    let ch = rows.ch;
    let ch_i = ch as isize;
    let cell_top = |row: u16| rows.y(row as isize).saturating_sub(dy);
    // The height of `n` rows from `row`, through the map: a span into the seam takes the gap.
    let span = |row: u16, n: usize| rows.span(row as isize, n.min(isize::MAX as usize) as isize);
    // A height in cells as pixels, saturating: the op vocabulary allows u16::MAX rows.
    let px_h = |cells: usize| (cells as isize).saturating_mul(ch_i);
    let band = |top: isize, h: isize| Some((top, top.saturating_add(h)));
    match op {
        Op::Clear => Some((isize::MIN, isize::MAX)),
        Op::CursorZone { .. } => None,
        // A sound paints nothing.
        Op::Sound { .. } | Op::Music { .. } => None,
        // An event and state that paint nothing themselves.
        Op::Burst { .. } => None,
        // Lighting, post-processing and particles reach the whole frame (a frame carrying any
        // of them is rasterised whole: `raster_frame`).
        Op::Ambient { .. }
        | Op::Light { .. }
        | Op::Overlay
        | Op::Bloom { .. }
        | Op::Crt { .. }
        | Op::Emitter { .. } => Some((isize::MIN, isize::MAX)),
        // A pixel-space quad turned about its centre: the band of its bounding box —
        // the centre plus or minus half the diagonal, whatever the angle (conservative).
        Op::Sprite { y, w, h, .. } | Op::Quad { y, w, h, .. } => {
            let (cy, half) = quad_extent(*y, *w, *h);
            if !cy.is_finite() || !half.is_finite() {
                return Some((isize::MIN, isize::MAX));
            }
            let top = (cy - half)
                .floor()
                .clamp(isize::MIN as f32, isize::MAX as f32) as isize;
            let bottom = (cy + half)
                .ceil()
                .clamp(isize::MIN as f32, isize::MAX as f32) as isize;
            Some((top, bottom.max(top)))
        }
        // Pixel text sits where it says, whatever the grid or scroll: its band is its own.
        Op::TextPx { y, face, .. } => {
            let top = y.round() as isize;
            band(top, px_h(face.scale.max(1) as usize))
        }
        Op::Text { row, face, .. } => band(cell_top(*row), px_h(face.scale.max(1) as usize)),
        Op::Cursor { row, .. } => band(cell_top(*row), ch_i),
        Op::Rect { row, h, .. } => band(cell_top(*row), span(*row, *h as usize)),
        Op::Image { row, h, .. } => band(cell_top(*row), span(*row, *h as usize)),
        Op::FRect { y, h, .. } => {
            // shifted like a cell (`cell_top`): an frect inside a scroll region moves
            let top = rows.yf(*y) - dy as f32;
            let bottom = top + rows.spanf(*y, *h);
            if !top.is_finite() || !bottom.is_finite() {
                // Non-finite geometry paints nothing sensible; treat it as everywhere
                // so whatever it did paint is repainted.
                return Some((isize::MIN, isize::MAX));
            }
            // A hairline snaps to whole device pixels around its centre (`snap_hairline`),
            // which can land a row above `floor(top)` — widen by more than any snap moves.
            Some((
                (top.floor() as isize).saturating_sub(4),
                (bottom.ceil() as isize).saturating_add(4),
            ))
        }
        Op::VSpans { row0, cols, .. } => {
            let tallest = cols
                .iter()
                .map(|segs| {
                    segs.iter()
                        .fold(0usize, |a, (h, _)| a.saturating_add(*h as usize))
                })
                .max()
                .unwrap_or(0);
            band(cell_top(*row0) + dy, span(*row0, tallest)) // VSpans ignore the scroll shift
        }
        Op::Cells { row0, w, bytes, .. } | Op::CellsRgb { row0, w, bytes, .. } => {
            let bits = bytes.len().saturating_mul(8);
            let board_rows = bits.div_ceil((*w).max(1) as usize);
            band(cell_top(*row0) + dy, span(*row0, board_rows)) // as do the bitboards
        }
        Op::ScrollRegion { .. } => Some((isize::MIN, isize::MAX)),
        // A region paints only inside its own rect, whose height is in PARENT cells — so
        // this band is exact even though the ops inside it are on a different grid.
        Op::CellRegion { y, h, .. } => band(cell_top(*y), span(*y, *h as usize)),
        // Never a leaf: `flatten` splices a fragment's ops into the frame's leaves, and
        // `render_ops` descends into it — so a fragment itself has no band of its own.
        Op::Fragment(_) => None,
        // A seam places the rows; it paints nothing (a new one invalidates the whole frame:
        // `Renderer::set_layout`).
        Op::Seam { .. } => None,
        // Never a leaf either (`flatten` descends, each child carrying the group's map); for
        // `render_ops`' skip test, everything above where the group is cut off.
        Op::AboveSeam { .. } => Some((isize::MIN, rows.seam_top().unwrap_or(isize::MAX))),
    }
}

/// One flattened op: a leaf of the frame's op tree with the scroll shift in effect
/// where it sits. Two frames are compared as sequences of these.
pub(super) struct Entry<'a> {
    op: &'a Op,
    dy: isize,
    /// Whether the leaf is, or (a cell region) holds, a blinking caret: its pixels depend on
    /// the blink phase as well as on the op (ADR-469).
    caret: bool,
    /// Whether the leaf sits in an `Op::AboveSeam` group: laid out on the rows above the seam
    /// carried on ([`RowMap::above_seam_rows`]) and cut off at the seam (ADR-615).
    above_seam: bool,
}

impl Entry<'_> {
    /// The rows this leaf is laid out on: the window's, or the uniform rows of a group.
    fn rows(&self, rows: &RowMap) -> RowMap {
        if self.above_seam {
            rows.above_seam_rows()
        } else {
            *rows
        }
    }

    /// Its pixel band (`op_band` on its own rows), cut off at the seam inside a group.
    fn band(&self, rows: &RowMap) -> Option<(isize, isize)> {
        let (top, bottom) = op_band(self.op, self.dy, &self.rows(rows))?;
        if !self.above_seam {
            return Some((top, bottom));
        }
        let bottom = bottom.min(rows.seam_top().unwrap_or(isize::MAX));
        (bottom > top).then_some((top, bottom))
    }
}

/// Whether `op` is a blinking caret or a block holding one — a leaf (or, on the GPU target, a
/// fragment) whose pixels depend on the blink phase.
pub(crate) fn holds_blinking_caret(op: &Op) -> bool {
    match op {
        Op::Cursor { blink, .. } => blink.is_some(),
        Op::ScrollRegion { ops, .. } | Op::CellRegion { ops, .. } | Op::AboveSeam { ops } => {
            ops.iter().any(holds_blinking_caret)
        }
        Op::Fragment(fragment) => fragment.0.iter().any(holds_blinking_caret),
        _ => false,
    }
}

/// Flatten a frame's op tree into leaves, each carrying its region's shift.
pub(super) fn flatten<'a>(
    ops: &'a [Op],
    ch: usize,
    dy: isize,
    above_seam: bool,
    out: &mut Vec<Entry<'a>>,
) {
    for op in ops {
        match op {
            Op::ScrollRegion { dy_frac, ops } => {
                flatten(ops, ch, scroll_px(*dy_frac, ch), above_seam, out)
            }
            // A memoised fragment is its ops in place (ADR-470): its leaves are the frame's.
            Op::Fragment(fragment) => flatten(&fragment.0, ch, dy, above_seam, out),
            // A group above the seam is its ops in place too, each on the group's rows (ADR-615)
            Op::AboveSeam { ops } => flatten(ops, ch, dy, true, out),
            // A cell region is ONE leaf: its children are on the region's grid, not this one,
            // so their bands cannot be computed with `ch` and their positions mean nothing
            // here. The whole region is compared (and repainted) as a unit.
            _ => out.push(Entry {
                op,
                dy,
                caret: holds_blinking_caret(op),
                above_seam,
            }),
        }
    }
}

/// The cell-row strips of a `fb_h`-tall framebuffer whose rows the map `rows` places:
/// strip 0 is the top margin `[0, top)`, strip `k ≥ 1` the cell row `k-1` (`[y(k-1),
/// y(k))`, so the row above a seam is one strip with its gap), and the strips past the
/// grid whatever is left below it, a cell at a time. The unit of the frame diff: a strip
/// is repainted whole or not at all, so the diff never has to reason about pixels.
pub(super) struct Strips {
    rows: RowMap,
    fb_h: usize,
    pub(super) count: usize,
}

impl Strips {
    pub(super) fn new(rows: RowMap, fb_h: usize) -> Strips {
        let top = rows.top.min(fb_h);
        // the rows it takes to reach the bottom: at most one past the uniform count, since
        // the gap only pushes rows down
        let mut n = (fb_h - top).div_ceil(rows.ch);
        while n > 0 && rows.y(n as isize - 1) >= fb_h as isize {
            n -= 1;
        }
        while rows.y(n as isize) < fb_h as isize {
            n += 1;
        }
        Strips {
            rows,
            fb_h,
            count: 1 + n,
        }
    }

    /// Strip `k`'s pixel rows, clipped to the framebuffer.
    pub(super) fn rows(&self, k: usize) -> (usize, usize) {
        let clip = |y: isize| (y.max(0) as usize).min(self.fb_h);
        if k == 0 {
            (0, clip(self.rows.top as isize))
        } else {
            let row = k as isize - 1;
            (clip(self.rows.y(row)), clip(self.rows.y(row + 1)))
        }
    }

    /// The strips a signed pixel band `[top, bottom)` overlaps, as a half-open index
    /// range (empty when the band misses the framebuffer).
    fn covering(&self, top: isize, bottom: isize) -> std::ops::Range<usize> {
        let top = top.max(0);
        let bottom = bottom.min(self.fb_h as isize);
        if bottom <= top {
            return 0..0;
        }
        let strip_of = |y: isize| -> usize {
            if y < self.rows.top as isize {
                0
            } else {
                (1 + self
                    .rows
                    .row_at_folded(y as f64 + 0.5)
                    .floor()
                    .min(u16::MAX as f64) as usize)
                    .min(self.count - 1)
            }
        };
        strip_of(top)..strip_of(bottom - 1) + 1
    }
}

/// A frame flattened for the diff: its leaves, each leaf's signed pixel band (`None`
/// for one that paints nothing), per strip the leaves whose band covers it, and whether
/// the frame's blinking caret was in its hidden half when it was painted.
pub(super) struct Leaves<'a> {
    leaves: Vec<Entry<'a>>,
    bands: Vec<Option<(isize, isize)>>,
    per_strip: Vec<Vec<u32>>,
    caret_hidden: bool,
}

impl<'a> Leaves<'a> {
    pub(super) fn of(ops: &'a [Op], strips: &Strips, caret_hidden: bool) -> Leaves<'a> {
        let rows = &strips.rows;
        let mut leaves = Vec::new();
        flatten(ops, rows.ch, 0, false, &mut leaves);
        let bands: Vec<Option<(isize, isize)>> = leaves.iter().map(|e| e.band(rows)).collect();
        let mut per_strip = vec![Vec::new(); strips.count];
        for (i, band) in bands.iter().enumerate() {
            if let Some((top, bottom)) = band {
                for k in strips.covering(*top, *bottom) {
                    per_strip[k].push(i as u32);
                }
            }
        }
        Leaves {
            leaves,
            bands,
            per_strip,
            caret_hidden,
        }
    }

    /// The leaves whose band overlaps the pixel rows `[y0, y1)`, in frame order — what
    /// a strip's `per_strip` list is, for an arbitrary band.
    fn covering(&self, y0: usize, y1: usize) -> impl Iterator<Item = usize> + '_ {
        let (y0, y1) = (y0 as isize, y1 as isize);
        self.bands
            .iter()
            .enumerate()
            .filter(move |(_, b)| matches!(b, Some((top, bottom)) if *top < y1 && *bottom > y0))
            .map(|(i, _)| i)
    }
}

/// Which strips of the framebuffer must be re-rasterised to turn the frame `old`
/// painted into the frame `new` would paint: those whose sequence of covering leaf
/// ops differs — an op added, removed, changed, reordered, or shifted by a scroll
/// region. Per strip, the ops are compared as values (`Op: PartialEq`), so an
/// identical frame dirties nothing and a one-line edit dirties one strip. A leaf of the
/// same memoised fragment in both frames is the same op in memory (ADR-470), so the
/// pointer settles it before any field is compared; a blinking caret's strip is dirty
/// when the phase flipped (ADR-469). Returns a bool per strip.
pub(super) fn strip_diff(new: &Leaves, old: &Leaves, strips: &Strips) -> Vec<bool> {
    (0..strips.count)
        .map(|k| {
            let (a, b) = (&new.per_strip[k], &old.per_strip[k]);
            a.len() != b.len()
                || a.iter()
                    .zip(b)
                    .any(|(&i, &j)| !same_leaf(new, i as usize, old, j as usize))
        })
        .collect()
}

/// Whether leaf `i` of `new` paints exactly what leaf `j` of `old` painted: the same op
/// (by pointer first, ADR-470) under the same shift, and not a blinking caret whose phase
/// flipped in between (ADR-469).
pub(super) fn same_leaf(new: &Leaves, i: usize, old: &Leaves, j: usize) -> bool {
    let (x, y) = (&new.leaves[i], &old.leaves[j]);
    x.dy == y.dy
        && x.above_seam == y.above_seam
        && !(x.caret && new.caret_hidden != old.caret_hidden)
        && (std::ptr::eq(x.op, y.op) || x.op == y.op)
}

// ---- the column damage ----------------------------------------------------------
//
// A typed character changes one text op of one row; the strip diff dirties that row,
// and re-rasterising the whole row repaints every pane beside the one typed in. The
// strip's leaves say more: two strips whose covering leaves agree on a common prefix and
// suffix differ only by the leaves in between, and pixels none of those leaves can reach
// are painted by the shared leaves alone — the same ops, in the same order, over the same
// cleared ground — so they are the pixels the canvas already holds. A strip is
// re-rasterised in the columns its differing leaves can paint, old and new; the rest of
// the row stays. Exact for the same reason the strip diff is: a leaf's columns
// (`leaf_columns`) are conservative, and a leaf whose columns are not known narrower than
// the window (a clear, a cell region, anything rotated or non-finite) widens the strip
// back to the whole row.

/// The base cell a cell-grid text op ends at: the `Text` arm of `render_ops`' walk,
/// advancing over the units it draws — a raw tab to the next screen stop, a zero-width
/// unit not at all, any other `unit_cells × scale`.
pub(super) fn text_end_cell(s: &str, col: usize, scale: usize, ligatures: bool) -> usize {
    let mut cell = col;
    for unit in text_units(s, ligatures) {
        if unit == "\t" {
            cell += cluster_cells_at(unit, cell, TAB_WIDTH);
            continue;
        }
        cell += unit_cells(unit) * scale;
    }
    cell
}

/// The signed pixel columns `[left, right)` leaf `op` can paint on a grid at `ox` with
/// cells `cw` wide — conservative, like `op_band` — or `None` when that is not known to
/// be narrower than the window. A text unit's glyph canvas is exactly its cell block
/// (`Renderer::build_cluster`), so a text op covers its cells and nothing beside them.
pub(super) fn leaf_columns(
    op: &Op,
    ox: usize,
    cw: usize,
    ligatures: bool,
) -> Option<(isize, isize)> {
    let (ox, cw_i) = (ox as isize, cw as isize);
    let cells = |from: usize, to: usize| {
        Some((
            ox.saturating_add((from as isize).saturating_mul(cw_i)),
            ox.saturating_add((to as isize).saturating_mul(cw_i)),
        ))
    };
    let pixels = |left: f32, right: f32| {
        // a pixel-space op: its float extent widened past any rounding or hairline snap
        (left.is_finite() && right.is_finite()).then(|| {
            (
                (left.floor() as isize).saturating_sub(4),
                (right.ceil() as isize).saturating_add(4),
            )
        })
    };
    match op {
        Op::Text { col, s, face, .. } => {
            let col = *col as usize;
            cells(
                col,
                text_end_cell(s, col, face.scale.max(1) as usize, ligatures),
            )
        }
        // a block over a wide character covers its cells (ADR-515)
        Op::Cursor { col, under, .. } => {
            let col = *col as usize;
            cells(
                col,
                col + under.as_ref().map_or(1, |(text, _)| caret_cells(text)),
            )
        }
        Op::Rect { col, w, .. } => cells(*col as usize, *col as usize + *w as usize),
        Op::Image { col, w, .. } => cells(*col as usize, *col as usize + *w as usize),
        Op::VSpans { col0, cols, .. } => cells(*col0 as usize, *col0 as usize + cols.len()),
        Op::Cells {
            col0, w, aspect, ..
        }
        | Op::CellsRgb {
            col0, w, aspect, ..
        } => cells(
            *col0 as usize,
            *col0 as usize + (*w).max(1) as usize * (*aspect).max(1) as usize,
        ),
        Op::FRect { x, w, .. } => {
            pixels(ox as f32 + x * cw as f32, ox as f32 + (x + w) * cw as f32)
        }
        Op::Sprite { x, y, w, h, .. } | Op::Quad { x, y, w, h, .. } => {
            // turned about its centre: the centre plus or minus half the diagonal
            let (_, half) = quad_extent(*y, *w, *h);
            pixels(x + w / 2.0 - half, x + w / 2.0 + half)
        }
        Op::TextPx {
            x, s, face, align, ..
        } => {
            let width = text_px_width(s, cw, face.scale.max(1) as usize) as f32;
            let left = match align {
                crate::host::gui::Align::Left => *x,
                crate::host::gui::Align::Center => *x - width / 2.0,
                crate::host::gui::Align::Right => *x - width,
            };
            pixels(left, left + width)
        }
        _ => None,
    }
}

/// The pixel columns `[x0, x1)` of strip `k` that can differ between `old`'s raster and
/// `new`'s: the union of the columns of the leaves that differ — outside the strip's common
/// prefix and suffix, and position by position when what is left is one length in both —
/// in either frame, clamped to the `fb_w`-wide window. The whole row when any of those
/// leaves has no known columns.
pub(super) fn strip_columns(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    fb_w: usize,
    ox: usize,
    cw: usize,
    ligatures: bool,
) -> (usize, usize) {
    let (a, b) = (&new.per_strip[k], &old.per_strip[k]);
    let same = |p: usize, q: usize| same_leaf(new, a[p] as usize, old, b[q] as usize);
    let shorter = a.len().min(b.len());
    let prefix = (0..shorter).take_while(|&n| same(n, n)).count();
    let suffix = (0..shorter - prefix)
        .take_while(|&n| same(a.len() - 1 - n, b.len() - 1 - n))
        .count();
    let (middle_new, middle_old) = (prefix..a.len() - suffix, prefix..b.len() - suffix);
    // Middles of one length pair up position by position (a typed character changes the
    // line's text op and moves the caret, with the leaves between them alike): only the
    // pairs that differ count, since a pixel no differing pair reaches is painted by the
    // same ops in the same order in both frames. Otherwise the whole middle counts.
    let mut differing: Vec<&Op> = Vec::new();
    if middle_new.len() == middle_old.len() {
        for (p, q) in middle_new.zip(middle_old) {
            if !same(p, q) {
                differing.push(new.leaves[a[p] as usize].op);
                differing.push(old.leaves[b[q] as usize].op);
            }
        }
    } else {
        differing.extend(middle_new.map(|p| new.leaves[a[p] as usize].op));
        differing.extend(middle_old.map(|q| old.leaves[b[q] as usize].op));
    }
    let (mut left, mut right) = (isize::MAX, isize::MIN);
    for op in differing {
        let Some((from, to)) = leaf_columns(op, ox, cw, ligatures) else {
            return (0, fb_w);
        };
        left = left.min(from);
        right = right.max(to);
    }
    let x0 = left.clamp(0, fb_w as isize) as usize;
    let x1 = right.clamp(0, fb_w as isize) as usize;
    if x1 <= x0 {
        // nothing the differing leaves paint is on screen: the row whole, rather than a
        // zero-width rect
        return (0, fb_w);
    }
    (x0, x1)
}

// ---- the scroll blit ----------------------------------------------------------
//
// A scroll changes every strip of a pane, so the strip diff alone re-rasterises the
// whole pane — 6 ms of glyph blitting at 1080p, four times that on a 4K display — for
// pixels the canvas already holds one line up or down. The blit finds them: a dirty
// strip whose ops are the previous frame's ops at some other rows, TRANSLATED by a whole
// number of pixels, paints exactly the pixels the retained canvas has there, so those
// rows are copied instead of drawn. Only the strips that really changed (the line
// scrolled in, the mode line's `L12`, the scrollbar thumb) are rasterised.
//
// The rule is exact, not heuristic: a strip's pixels are a function of the leaves
// whose band overlaps it, each painted at a position; if the new strip's leaves are
// the old band's leaves in the same order, each the same op at a position `delta`
// pixels lower (a text line, the cursor, a one-row band), or a solid fill that covers
// both bands entirely (the gutter wash, a divider, the frame clear — a fill has no
// row-dependent pixels away from its corners), the strips are pixel-identical. Bands are
// conservative (`op_band`), so a leaf that only nearly touches the strip can veto a
// blit but never fake one.

/// The pixel top of a leaf that is a translatable cell-row op (text, cursor, a rect), as
/// `op_band` places it; `None` for the others.
pub(super) fn leaf_top(op: &Op, dy: isize, rows: &RowMap) -> Option<isize> {
    match op {
        Op::Text { row, .. } | Op::Cursor { row, .. } | Op::Rect { row, .. } => {
            Some(rows.y(*row as isize).saturating_sub(dy))
        }
        _ => None,
    }
}

/// Whether painting `new` into the rows `[y0, y1)` produces the pixels `old` painted
/// into `[y0 - delta, y1 - delta)`: the same op translated by `delta` pixels, or a solid
/// fill that covers both bands with its straight part (rounded corners kept `radius`
/// clear of the bands, and a hairline's snap slop on top).
pub(super) fn leaf_translates(
    new: &Entry,
    old: &Entry,
    delta: isize,
    (y0, y1): (isize, isize),
    rows: &RowMap,
    cw: usize,
) -> bool {
    // a leaf in a group above the seam is on other rows than one outside it (ADR-615)
    if new.above_seam != old.above_seam {
        return false;
    }
    let rows = &new.rows(rows);
    let (src0, src1) = (y0 - delta, y1 - delta);
    let top = |row: u16, dy: isize| rows.y(row as isize).saturating_sub(dy);
    let covers = |top: f32, bottom: f32, margin: f32, lo: isize, hi: isize| {
        top + margin <= lo as f32 && bottom - margin >= hi as f32
    };
    match (new.op, old.op) {
        (Op::Clear, Op::Clear) => true,
        (
            Op::Text {
                row: nr,
                col: nc,
                s: ns,
                face: nf,
            },
            Op::Text {
                row: or,
                col: oc,
                s: os,
                face: of,
            },
        ) => {
            nc == oc
                && nf == of
                && ns == os
                && top(*nr, new.dy) == top(*or, old.dy).saturating_add(delta)
        }
        (
            Op::Cursor {
                row: nr,
                col: nc,
                style: nst,
                color: ncolor,
                blink: nblink,
                under: nunder,
            },
            Op::Cursor {
                row: or,
                col: oc,
                style: ost,
                color: ocolor,
                blink: oblink,
                under: ounder,
            },
        ) => {
            nc == oc
                && nst == ost
                && ncolor == ocolor
                && nblink == oblink
                && nunder == ounder
                && top(*nr, new.dy) == top(*or, old.dy).saturating_add(delta)
        }
        (
            Op::Rect {
                row: nr,
                col: nc,
                w: nw,
                h: nh,
                face: nf,
                radius: nrad,
            },
            Op::Rect {
                row: or,
                col: oc,
                w: ow,
                h: oh,
                face: of,
                radius: orad,
            },
        ) => {
            if nc != oc || nw != ow || nf != of || nrad != orad {
                return false;
            }
            let (ntop, otop) = (top(*nr, new.dy), top(*or, old.dy));
            // heights through the map: the row above a seam is taller by its gap
            let (nheight, oheight) = (
                rows.span(*nr as isize, *nh as isize),
                rows.span(*or as isize, *oh as isize),
            );
            if nheight == oheight && ntop == otop.saturating_add(delta) {
                return true;
            }
            // a solid fill: the same pixels wherever it is, away from its corners
            let margin = if *nrad > 0.0 {
                (nrad * cw as f32).ceil() + 1.0
            } else {
                0.0
            };
            covers(
                ntop as f32,
                ntop.saturating_add(nheight) as f32,
                margin,
                y0,
                y1,
            ) && covers(
                otop as f32,
                otop.saturating_add(oheight) as f32,
                margin,
                src0,
                src1,
            )
        }
        (
            Op::FRect {
                x: nx,
                y: ny,
                w: nw,
                h: nh,
                face: nf,
                opacity: nop,
                radius: nrad,
            },
            Op::FRect {
                x: ox_,
                y: oy_,
                w: ow,
                h: oh,
                face: of,
                opacity: oop,
                radius: orad,
            },
        ) => {
            if nx != ox_ || nw != ow || nf != of || nop != oop || nrad != orad {
                return false;
            }
            let ntop = rows.yf(*ny) - new.dy as f32;
            let otop = rows.yf(*oy_) - old.dy as f32;
            let (nheight, oheight) = (rows.spanf(*ny, *nh), rows.spanf(*oy_, *oh));
            if !(ntop.is_finite() && otop.is_finite()) {
                return false;
            }
            if (nheight - oheight).abs() < 1e-3 && (ntop - otop - delta as f32).abs() < 1e-3 {
                return true;
            }
            // the same slop `op_band` allows a hairline's snap, plus the corners
            let margin = nrad * cw as f32 + 5.0;
            covers(ntop, ntop + nheight, margin, y0, y1)
                && covers(otop, otop + oheight, margin, src0, src1)
        }
        // bitboards and column spans ignore the scroll shift and are never translated
        _ => false,
    }
}

/// The pixel translations that could carry an old leaf onto the first translatable
/// leaf of the new strip `k`: for each old leaf equal to it up to its row, the
/// difference of their tops. The candidates a strip's blit is tried at.
pub(super) fn blit_deltas(new: &Leaves, old: &Leaves, k: usize, rows: &RowMap) -> Vec<isize> {
    let mut deltas = Vec::new();
    for &i in &new.per_strip[k] {
        let leaf = &new.leaves[i as usize];
        let Some(top) = leaf_top(leaf.op, leaf.dy, &leaf.rows(rows)) else {
            continue;
        };
        // a whole-pane rect anchors nothing (it matches anywhere by covering)
        if matches!(leaf.op, Op::Rect { h, .. } if *h > 1) {
            continue;
        }
        for old_leaf in &old.leaves {
            let Some(old_top) = leaf_top(old_leaf.op, old_leaf.dy, &old_leaf.rows(rows)) else {
                continue;
            };
            let same = match (leaf.op, old_leaf.op) {
                (
                    Op::Text {
                        col: a,
                        s: sa,
                        face: fa,
                        ..
                    },
                    Op::Text {
                        col: b,
                        s: sb,
                        face: fb,
                        ..
                    },
                ) => a == b && fa == fb && sa == sb,
                (
                    Op::Cursor {
                        col: a,
                        style: sa,
                        color: ca,
                        under: ua,
                        ..
                    },
                    Op::Cursor {
                        col: b,
                        style: sb,
                        color: cb,
                        under: ub,
                        ..
                    },
                ) => a == b && sa == sb && ca == cb && ua == ub,
                (
                    Op::Rect {
                        col: a,
                        w: wa,
                        h: ha,
                        face: fa,
                        radius: ra,
                        ..
                    },
                    Op::Rect {
                        col: b,
                        w: wb,
                        h: hb,
                        face: fb,
                        radius: rb,
                        ..
                    },
                ) => a == b && wa == wb && ha == hb && fa == fb && ra == rb,
                _ => false,
            };
            let delta = top - old_top;
            if same && delta != 0 && !deltas.contains(&delta) {
                deltas.push(delta);
            }
        }
        // one anchor is enough: every other leaf still has to agree
        if !deltas.is_empty() {
            break;
        }
    }
    deltas
}

/// Whether dirty strip `k` (pixel rows `[y0, y1)`) is the old frame's band
/// `[y0 - delta, y1 - delta)` translated: every new leaf covering the strip pairs, in
/// order, with an old leaf covering the source band under `leaf_translates`.
pub(super) fn strip_translates(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    delta: isize,
    (y0, y1): (usize, usize),
    fb_h: usize,
    rows: &RowMap,
    cw: usize,
) -> bool {
    let (src0, src1) = (y0 as isize - delta, y1 as isize - delta);
    if src0 < 0 || src1 > fb_h as isize {
        return false;
    }
    let new_leaves = &new.per_strip[k];
    // a caret whose blink phase flipped is not the pixels the canvas holds anywhere
    if new.caret_hidden != old.caret_hidden
        && new_leaves.iter().any(|&i| new.leaves[i as usize].caret)
    {
        return false;
    }
    let mut old_leaves = old.covering(src0 as usize, src1 as usize);
    for &i in new_leaves {
        let Some(j) = old_leaves.next() else {
            return false;
        };
        if !leaf_translates(
            &new.leaves[i as usize],
            &old.leaves[j],
            delta,
            (y0 as isize, y1 as isize),
            rows,
            cw,
        ) {
            return false;
        }
    }
    old_leaves.next().is_none()
}

/// For every dirty full-height cell-row strip that is a translation of pixels the
/// canvas already holds, `(k, source_y0)`: strip `k`'s rows are copied from the old
/// canvas rows starting at `source_y0` instead of rasterised. The translation that
/// carried the previous strip is tried first — a scroll moves a whole pane by one
/// delta — then the ones the strip's own anchor suggests.
#[allow(clippy::too_many_arguments)]
pub(super) fn strip_blits(
    new: &Leaves,
    old: &Leaves,
    dirty: &[bool],
    strips: &Strips,
    fb_h: usize,
    cw: usize,
) -> Vec<(usize, usize)> {
    let rows = &strips.rows;
    let ch = rows.ch;
    let mut blits = Vec::new();
    let mut last_delta: Option<isize> = None;
    for k in 1..strips.count {
        if !dirty[k] {
            continue;
        }
        let (y0, y1) = strips.rows(k);
        if y1 - y0 != ch {
            // a partial strip at the bottom, or the row above a seam with its gap: its rows
            // are its own
            continue;
        }
        let mut candidates = Vec::new();
        if let Some(d) = last_delta {
            candidates.push(d);
        }
        for d in blit_deltas(new, old, k, rows) {
            if !candidates.contains(&d) {
                candidates.push(d);
            }
        }
        for delta in candidates {
            if strip_translates(new, old, k, delta, (y0, y1), fb_h, rows, cw) {
                blits.push((k, (y0 as isize - delta) as usize));
                last_delta = Some(delta);
                break;
            }
        }
    }
    blits
}

/// Merge runs of dirty strips into damage rects — one raster pass each. Strip `k` spans
/// the pixel columns `columns[k]`; a run of adjacent dirty strips becomes one rect over
/// the union of their columns (re-rasterising more of a row than changed is exact too,
/// and one pass over the frame per run is what the row diff always paid).
pub(super) fn dirty_rects(
    dirty: &[bool],
    columns: &[(usize, usize)],
    strips: &Strips,
) -> Vec<DamageRect> {
    let mut rects: Vec<DamageRect> = Vec::new();
    for (k, &d) in dirty.iter().enumerate() {
        if !d {
            continue;
        }
        let (y0, y1) = strips.rows(k);
        let (x0, x1) = columns[k];
        if y1 <= y0 || x1 <= x0 {
            continue;
        }
        match rects.last_mut() {
            Some(last) if last.y1 == y0 => {
                last.y1 = y1;
                last.x0 = last.x0.min(x0);
                last.x1 = last.x1.max(x1);
            }
            _ => rects.push(DamageRect { x0, y0, x1, y1 }),
        }
    }
    rects
}

#[cfg(test)]
mod strip_diff_tests {
    use super::*;
    use crate::host::gui::{CursorStyle, Face, Op};

    fn text(row: u16, s: &str) -> Op {
        Op::Text {
            row,
            col: 1,
            s: s.into(),
            face: Face {
                fg: Some([200, 200, 200]),
                ..Face::default()
            },
        }
    }

    fn cursor(row: u16) -> Op {
        Op::Cursor {
            row,
            col: 2,
            style: CursorStyle::Block,
            color: None,
            blink: None,
            under: None,
        }
    }

    fn rect(row: u16, h: u16) -> Op {
        Op::Rect {
            row,
            col: 0,
            w: 6,
            h,
            face: Face {
                bg: Some([30, 40, 50]),
                ..Face::default()
            },
            radius: 0.0,
        }
    }

    fn editor_frame(lines: &[&str], cursor_row: u16) -> Vec<Op> {
        let mut ops = vec![Op::Clear, rect(cursor_row, 1)];
        for (i, l) in lines.iter().enumerate() {
            ops.push(text(i as u16, l));
        }
        ops.push(cursor(cursor_row));
        ops
    }

    /// Which strips (cell rows, 1-based; 0 is the top margin) a change dirties.
    fn dirty_rows(new: &[Op], old: &[Op], oy: usize, ch: usize, fb_h: usize) -> Vec<usize> {
        let strips = Strips::new(RowMap::uniform(oy, ch), fb_h);
        strip_diff(
            &Leaves::of(new, &strips, false),
            &Leaves::of(old, &strips, false),
            &strips,
        )
        .iter()
        .enumerate()
        .filter(|(_, d)| **d)
        .map(|(k, _)| k)
        .collect()
    }

    #[test]
    fn an_identical_frame_dirties_no_strip() {
        let a = editor_frame(&["one", "two", "three"], 1);
        let b = editor_frame(&["one", "two", "three"], 1);
        assert_eq!(dirty_rows(&a, &b, 2, 10, 60), Vec::<usize>::new());
    }

    #[test]
    fn editing_one_line_dirties_only_that_row() {
        let old = editor_frame(&["one", "two", "three"], 1);
        let new = editor_frame(&["one", "two!", "three"], 1);
        // strip 2 = cell row 1 (the edited line, which also carries the hl-line band
        // and the cursor)
        assert_eq!(dirty_rows(&new, &old, 2, 10, 60), vec![2]);
    }

    #[test]
    fn moving_the_cursor_dirties_the_rows_it_left_and_entered() {
        let old = editor_frame(&["one", "two", "three"], 1);
        let new = editor_frame(&["one", "two", "three"], 2);
        assert_eq!(dirty_rows(&new, &old, 2, 10, 60), vec![2, 3]);
    }

    #[test]
    fn a_taller_panel_dirties_every_row_it_spans_but_no_other() {
        let old = vec![Op::Clear, text(0, "a"), text(5, "f")];
        let new = vec![Op::Clear, rect(1, 3), text(0, "a"), text(5, "f")];
        assert_eq!(dirty_rows(&new, &old, 0, 10, 60), vec![2, 3, 4]);
    }

    #[test]
    fn a_scroll_shift_dirties_the_region_rows_but_leaves_the_rest() {
        let region = |dy: f32| Op::ScrollRegion {
            dy_frac: dy,
            ops: vec![text(1, "body"), text(2, "more")],
        };
        let old = vec![Op::Clear, region(0.0), text(5, "modeline")];
        let new = vec![Op::Clear, region(0.5), text(5, "modeline")];
        // the leaves shift up half a cell: rows 1–2 as they were plus the row above
        // each now-straddled band — never the mode line at row 5
        let rows = dirty_rows(&new, &old, 0, 10, 60);
        assert!(rows.contains(&2) && rows.contains(&3), "{rows:?}");
        assert!(!rows.contains(&6), "{rows:?}");
    }

    #[test]
    fn a_clear_added_or_removed_dirties_everything() {
        let old = vec![text(0, "a")];
        let new = vec![Op::Clear, text(0, "a")];
        let strips = Strips::new(RowMap::uniform(0, 10), 30);
        let all: Vec<bool> = strip_diff(
            &Leaves::of(&new, &strips, false),
            &Leaves::of(&old, &strips, false),
            &strips,
        );
        // every strip that has rows (the top margin is empty at oy = 0)
        let mut with_rows = (0..strips.count).filter(|&k| strips.rows(k).1 > strips.rows(k).0);
        assert!(
            with_rows.clone().count() == 3 && with_rows.all(|k| all[k]),
            "{all:?}"
        );
    }

    /// The property the whole scheme rests on: painting a sequence of frames
    /// incrementally (strip diff, retained canvas) ends on exactly the pixels a
    /// from-scratch raster of the last frame produces.
    #[test]
    fn an_incremental_raster_matches_a_full_one_pixel_for_pixel() {
        let frames = [
            editor_frame(&["alpha", "beta", "gamma"], 0),
            editor_frame(&["alpha", "beta!", "gamma"], 1),
            editor_frame(&["alpha", "beta!", "gamma", "delta"], 3),
            vec![
                Op::Clear,
                Op::ScrollRegion {
                    dy_frac: 0.3,
                    ops: vec![text(0, "alpha"), text(1, "beta!"), rect(2, 2)],
                },
                text(4, "mode line"),
                Op::FRect {
                    x: 0.5,
                    y: 1.25,
                    w: 0.05,
                    h: 2.0,
                    face: Face {
                        bg: Some([90, 90, 120]),
                        ..Face::default()
                    },
                    opacity: 1.0,
                    radius: 0.0,
                },
                cursor(1),
            ],
            editor_frame(&["alpha", "beta!", "gamma", "delta"], 3),
            editor_frame(&["alpha", "beta!", "gamma", "delta"], 3), // identical: no work
            editor_frame(&["", "beta!", "gamma", "delta"], 0),
        ];
        let (fb_w, fb_h) = (96usize, 80usize);
        let mut incremental = Renderer::new(1.0, default_families(), 14.0);
        for (i, frame) in frames.iter().enumerate() {
            let bands = raster_frame(&mut incremental, frame, fb_w, fb_h, false);
            if i > 0 && frames[i] == frames[i - 1] {
                assert!(bands.is_empty(), "an identical frame repainted {bands:?}");
            }
            let mut full = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut full, frame, fb_w, fb_h, true);
            assert!(
                incremental.retained.canvas == full.retained.canvas,
                "frame {i}: incremental raster differs from a full one (repainted {bands:?})"
            );
        }
    }

    /// An `frect` inside a scroll region rides with the text: a gutter's change bar
    /// painted half a cell up lands where a text op at the same row does, both in the
    /// raster and in the rows the diff marks dirty, so a bar never jumps a row while
    /// the lines glide.
    #[test]
    fn an_frect_inside_a_scroll_region_shifts_with_the_text() {
        let bar = Op::FRect {
            x: 0.0,
            y: 2.0,
            w: 1.0,
            h: 1.0,
            face: Face {
                bg: Some([0, 255, 0]),
                ..Face::default()
            },
            opacity: 1.0,
            radius: 0.0,
        };
        let region = |dy: f32| Op::ScrollRegion {
            dy_frac: dy,
            ops: vec![bar.clone()],
        };
        // raster: at a half-cell shift the bar's top pixel row moves up by half a cell
        let raster = |dy: f32| {
            let mut r = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut r, &[Op::Clear, region(dy)], 40, 80, true);
            let (fb_w, ch) = (40, r.cell_h);
            let first_green = r
                .retained
                .canvas
                .iter()
                .position(|&p| p == 0x00ff00)
                .map(|i| i / fb_w)
                .expect("the bar was painted");
            (first_green, ch)
        };
        let (at_rest, ch) = raster(0.0);
        let (shifted, _) = raster(0.5);
        assert_eq!(at_rest, 2 * ch, "unshifted, the bar starts at its cell row");
        assert_eq!(shifted, 2 * ch - ch / 2, "shifted, it rides up half a cell");
        // diff: the shift dirties the rows the bar left and entered, not the whole frame
        let old = vec![Op::Clear, region(0.0), text(6, "modeline")];
        let new = vec![Op::Clear, region(0.5), text(6, "modeline")];
        let rows = dirty_rows(&new, &old, 0, 10, 80);
        assert!(rows.contains(&2) && rows.contains(&3), "{rows:?}");
        assert!(!rows.contains(&7), "{rows:?}");
    }

    /// A hairline whose unsnapped geometry sits just below a strip boundary snaps onto
    /// the row above it; the incremental raster must still match the full one.
    #[test]
    fn a_snapped_hairline_across_a_strip_boundary_is_repainted_whole() {
        let hair = |y: f32| Op::FRect {
            x: 1.0,
            y,
            w: 6.0,
            h: 0.002,
            face: Face {
                bg: Some([255, 0, 0]),
                ..Face::default()
            },
            opacity: 1.0,
            radius: 0.0,
        };
        let (fb_w, fb_h) = (96usize, 80usize);
        let mut incremental = Renderer::new(1.0, default_families(), 14.0);
        let ch = incremental.cell_h as f32;
        let frames = [
            vec![Op::Clear, text(0, "aaaa"), text(1, "bbbb")],
            vec![
                Op::Clear,
                text(0, "aaaa"),
                text(1, "bbbb"),
                hair(1.0 + 0.0001 / ch),
            ],
            vec![Op::Clear, text(0, "aaaa"), text(1, "bbbb")],
        ];
        for (i, frame) in frames.iter().enumerate() {
            raster_frame(&mut incremental, frame, fb_w, fb_h, false);
            let mut full = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut full, frame, fb_w, fb_h, true);
            assert!(
                incremental.retained.canvas == full.retained.canvas,
                "frame {i} differs"
            );
        }
    }

    /// The frames of a scrolling pane: the gutter wash and the divider stay, the
    /// line-number/text rows shift by `top`, the mode line keeps its row and says where
    /// we are. What `strip_blits` exists for.
    fn scrolled_frame(top: usize, lines: &[&str], dy_frac: f32) -> Vec<Op> {
        let rows = 5usize;
        let mut body = vec![rect(0, rows as u16)]; // the gutter wash, taller than a strip
        for r in 0..rows {
            let line = top + r;
            if line < lines.len() {
                body.push(Op::Text {
                    row: r as u16,
                    col: 0,
                    s: format!("{line:2}"),
                    face: Face {
                        fg: Some([120, 120, 120]),
                        ..Face::default()
                    },
                });
                body.push(text(r as u16, lines[line]));
            }
        }
        vec![
            Op::Clear,
            Op::ScrollRegion { dy_frac, ops: body },
            Op::Rect {
                row: 0,
                col: 7,
                w: 1,
                h: 6,
                face: Face {
                    bg: Some([80, 80, 80]),
                    ..Face::default()
                },
                radius: 0.0,
            },
            text(5, &format!("L{top}")),
        ]
    }

    /// Paint `frames` in sequence incrementally, checking each against a from-scratch
    /// raster; returns the blitted row count of each frame.
    fn blit_run(frames: &[Vec<Op>], fb_w: usize, fb_h: usize) -> Vec<usize> {
        let mut incremental = Renderer::new(1.0, default_families(), 14.0);
        let mut blitted = Vec::new();
        for (i, frame) in frames.iter().enumerate() {
            raster_frame(&mut incremental, frame, fb_w, fb_h, false);
            blitted.push(incremental.retained.blit_rows);
            let mut full = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut full, frame, fb_w, fb_h, true);
            assert!(
                incremental.retained.canvas == full.retained.canvas,
                "frame {i}: a blitted raster differs from a full one"
            );
        }
        blitted
    }

    #[test]
    fn a_scroll_copies_the_rows_it_keeps_and_draws_only_the_new_ones() {
        let lines = [
            "alpha", "beta", "gamma", "delta", "eps", "zeta", "eta", "theta",
        ];
        let frames = [
            scrolled_frame(0, &lines, 0.0),
            scrolled_frame(1, &lines, 0.0), // one line: four strips copied, one drawn
            scrolled_frame(3, &lines, 0.0), // two lines at once: three rows survive
            scrolled_frame(2, &lines, 0.0), // and back down
        ];
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        // seven cell rows tall, so every row of the six-row frame is a full strip
        let blitted = blit_run(&frames, 96, 7 * ch);
        assert_eq!(blitted[0], 0, "the first frame has nothing to copy from");
        assert_eq!(blitted[1], 4 * ch, "{blitted:?}");
        assert_eq!(blitted[2], 3 * ch, "{blitted:?}");
        assert_eq!(blitted[3], 4 * ch, "{blitted:?}");
    }

    #[test]
    fn a_sub_cell_scroll_step_is_a_translation_too() {
        let lines = [
            "alpha", "beta", "gamma", "delta", "eps", "zeta", "eta", "theta",
        ];
        // the same top, gliding: every strip is the old canvas a few pixels up — except
        // the ones a partially shown line enters or leaves
        let frames = [
            scrolled_frame(1, &lines, 0.0),
            scrolled_frame(1, &lines, 0.25),
            scrolled_frame(1, &lines, 0.5),
            scrolled_frame(2, &lines, 0.0), // the snap: back to whole rows
        ];
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        let blitted = blit_run(&frames, 96, 7 * ch);
        assert!(blitted[1] >= 2 * ch, "{blitted:?}");
        assert!(blitted[2] >= 2 * ch, "{blitted:?}");
        assert!(blitted[3] >= 2 * ch, "{blitted:?}");
    }

    #[test]
    fn two_panes_scrolling_opposite_ways_copy_from_each_other_safely() {
        let lines = [
            "alpha", "beta", "gamma", "delta", "eps", "zeta", "eta", "theta",
        ];
        // pane A rows 0..4 scrolls down, pane B rows 4..8 scrolls up, in one frame:
        // a destination strip of one is a source strip of the other
        let pane = |top: usize, at: u16| -> Vec<Op> {
            (0..4)
                .filter(|r| top + r < lines.len())
                .map(|r| text(at + r as u16, lines[top + r]))
                .collect()
        };
        let frame = |a: usize, b: usize| {
            let mut ops = vec![Op::Clear];
            ops.extend(pane(a, 0));
            ops.extend(pane(b, 4));
            ops
        };
        let frames = [frame(1, 1), frame(2, 0), frame(0, 2)];
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        let blitted = blit_run(&frames, 96, 7 * ch);
        assert!(blitted[1] > 0 && blitted[2] > 0, "{blitted:?}");
    }

    #[test]
    fn a_strip_that_only_looks_shifted_is_drawn_not_copied() {
        // the text moves down a row but a cursor appears with it: the row it lands on
        // is not a translation of any old band (no old strip had text + cursor)
        let frames = [
            vec![Op::Clear, text(0, "alpha"), text(1, "beta")],
            vec![Op::Clear, text(1, "alpha"), text(2, "beta"), cursor(1)],
        ];
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        let blitted = blit_run(&frames, 96, 7 * ch);
        assert_eq!(
            blitted[1], ch,
            "only `beta`'s new row is a pure translation: {blitted:?}"
        );
    }

    /// A window whose width and height swap keeps its pixel count; the canvas must
    /// still be treated as fresh, or the diff would trust rows of the wrong shape.
    #[test]
    fn a_same_area_resize_rerasterises_everything() {
        let frame = editor_frame(&["alpha", "beta"], 0);
        let mut r = Renderer::new(1.0, default_families(), 14.0);
        raster_frame(&mut r, &frame, 96, 80, false);
        let bands = raster_frame(&mut r, &frame, 80, 96, false);
        assert_eq!(bands, vec![DamageRect::whole(80, 96)]);
        let mut full = Renderer::new(1.0, default_families(), 14.0);
        raster_frame(&mut full, &frame, 80, 96, true);
        assert!(r.retained.canvas == full.retained.canvas);
    }

    #[test]
    fn a_hairline_snaps_to_one_device_pixel_per_logical_pixel() {
        // a 0.45 px wide rule whose centre straddles a pixel boundary: 1 px, on the
        // pixel that holds the centre
        assert_eq!(
            snap_hairline(9.975, 0.45, 0.0, 30.0, 0.0, 1.0),
            (10.0, 1.0, 0.0, 30.0, 0.0)
        );
        assert_eq!(
            snap_hairline(4.275, 0.45, 0.0, 30.0, 0.0, 1.0),
            (4.0, 1.0, 0.0, 30.0, 0.0)
        );
        // at 2× the same logical hairline is 2 device px
        assert_eq!(
            snap_hairline(0.0, 30.0, 19.5, 0.9, 0.0, 2.0),
            (0.0, 30.0, 19.0, 2.0, 0.0)
        );
        // a real rect passes through untouched, radius included
        assert_eq!(
            snap_hairline(3.0, 8.0, 2.0, 5.0, 1.5, 1.0),
            (3.0, 8.0, 2.0, 5.0, 1.5)
        );
    }

    // ---- the blinking caret (ADR-469) and shared fragments (ADR-470) ----

    fn blinking(row: u16) -> Op {
        Op::Cursor {
            row,
            col: 2,
            style: CursorStyle::Block,
            color: None,
            blink: Some(crate::host::gui::CursorBlink {
                interval_ms: 530,
                blinks: 10,
            }),
            under: None,
        }
    }

    fn fragment(ops: Vec<Op>) -> Op {
        Op::Fragment(crate::host::gui::FragmentOps(ops.into()))
    }

    /// Paint `steps` — a frame and the blink phase it is painted in — incrementally,
    /// checking each against a from-scratch raster in the same phase; returns the rects
    /// each step repainted.
    fn blink_run(steps: &[(Vec<Op>, bool)], fb_w: usize, fb_h: usize) -> Vec<Vec<DamageRect>> {
        let mut incremental = Renderer::new(1.0, default_families(), 14.0);
        let mut repainted = Vec::new();
        for (i, (frame, hidden)) in steps.iter().enumerate() {
            incremental.caret_hidden = *hidden;
            repainted.push(raster_frame(&mut incremental, frame, fb_w, fb_h, false));
            let mut full = Renderer::new(1.0, default_families(), 14.0);
            full.caret_hidden = *hidden;
            raster_frame(&mut full, frame, fb_w, fb_h, true);
            assert!(
                incremental.retained.canvas == full.retained.canvas,
                "step {i}: an incremental raster differs from a full one"
            );
        }
        repainted
    }

    #[test]
    fn a_blink_phase_flip_repaints_only_the_caret_cell() {
        let mut frame = editor_frame(&["alpha", "beta", "gamma"], 1);
        frame.pop();
        frame.push(blinking(1));
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        let repainted = blink_run(
            &[
                (frame.clone(), false),
                (frame.clone(), true),
                (frame.clone(), true), // the same phase again: nothing to do
                (frame.clone(), false),
            ],
            96,
            80,
        );
        let renderer = Renderer::new(1.0, default_families(), 14.0);
        let ((ox, oy), cw) = (renderer.grid_origin(96, 80), renderer.cell_w);
        // the caret sits at column 2 of row 1, and its row holds nothing else that changed
        let caret_row = DamageRect {
            x0: ox + 2 * cw,
            y0: oy + ch,
            x1: ox + 3 * cw,
            y1: oy + 2 * ch,
        };
        assert_eq!(repainted[1], vec![caret_row]);
        assert!(repainted[2].is_empty(), "{:?}", repainted[2]);
        assert_eq!(repainted[3], vec![caret_row]);
        // and a hidden caret is exactly the frame without it
        let mut hidden = Renderer::new(1.0, default_families(), 14.0);
        hidden.caret_hidden = true;
        raster_frame(&mut hidden, &frame, 96, 80, true);
        let mut without = Renderer::new(1.0, default_families(), 14.0);
        let mut bare = frame.clone();
        bare.pop();
        raster_frame(&mut without, &bare, 96, 80, true);
        assert!(hidden.retained.canvas == without.retained.canvas);
    }

    #[test]
    fn a_blinking_caret_in_a_scrolling_pane_matches_a_full_raster_in_every_phase() {
        let lines = [
            "alpha", "beta", "gamma", "delta", "eps", "zeta", "eta", "theta",
        ];
        let with_caret = |top: usize, row: u16| {
            let mut frame = scrolled_frame(top, &lines, 0.0);
            if let Op::ScrollRegion { ops, .. } = &mut frame[1] {
                ops.push(blinking(row));
            }
            frame
        };
        let ch = Renderer::new(1.0, default_families(), 14.0).cell_h;
        blink_run(
            &[
                (with_caret(0, 2), false),
                (with_caret(1, 1), true), // scrolled one line, caret riding along, hidden
                (with_caret(1, 1), false),
                (with_caret(2, 0), false), // a blit carries a shown caret
                (with_caret(3, 3), true),
                (with_caret(2, 3), true), // a blit carries a hidden one
            ],
            96,
            7 * ch,
        );
    }

    #[test]
    fn a_fragment_paints_as_its_ops_spliced_in_and_matches_a_full_raster() {
        let rows = vec![
            text(0, "alpha"),
            text(1, "beta"),
            rect(2, 1),
            text(2, "gamma"),
        ];
        let shared = fragment(rows.clone());
        let spliced: Vec<Op> = [vec![Op::Clear], rows.clone(), vec![text(4, "mode")]].concat();
        let fragmented = vec![Op::Clear, shared.clone(), text(4, "mode")];
        let paint = |frame: &[Op]| {
            let mut r = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut r, frame, 96, 80, true);
            r.retained.canvas
        };
        assert!(paint(&spliced) == paint(&fragmented));
        // inside a scroll region the fragment takes the region's shift
        let scrolled = |ops: Vec<Op>| vec![Op::Clear, Op::ScrollRegion { dy_frac: 0.4, ops }];
        assert!(paint(&scrolled(rows.clone())) == paint(&scrolled(vec![shared.clone()])));
        // and a run of frames sharing it stays pixel-identical to full rasters
        blink_run(
            &[
                (fragmented.clone(), false),
                (vec![Op::Clear, shared.clone(), text(4, "mode!")], false),
                (vec![Op::Clear, fragment(rows.clone()), blinking(1)], true),
                (vec![Op::Clear, shared.clone(), blinking(1)], false),
            ],
            96,
            80,
        );
    }

    #[test]
    fn an_unchanged_fragment_dirties_nothing_and_its_leaves_compare_by_pointer() {
        let shared = fragment(vec![text(0, "alpha"), text(1, "beta"), text(2, "gamma")]);
        let old = vec![Op::Clear, shared.clone(), text(4, "L1")];
        let new = vec![Op::Clear, shared.clone(), text(4, "L2")];
        assert_eq!(dirty_rows(&new, &old, 0, 10, 60), vec![5]);
        // the frames' fragments are one allocation: the leaves are the same ops in memory
        let strips = Strips::new(RowMap::uniform(0, 10), 60);
        let (a, b) = (
            Leaves::of(&new, &strips, false),
            Leaves::of(&old, &strips, false),
        );
        assert!(std::ptr::eq(a.leaves[1].op, b.leaves[1].op));
        // a fragment rebuilt with the same ops is equal, just not by pointer
        let rebuilt = vec![
            Op::Clear,
            fragment(vec![text(0, "alpha"), text(1, "beta"), text(2, "gamma")]),
            text(4, "L2"),
        ];
        assert!(rebuilt == new);
        assert_eq!(dirty_rows(&rebuilt, &old, 0, 10, 60), vec![5]);
    }

    // ---- column damage (ADR-482) ----

    fn text_at(row: u16, col: u16, s: &str, fg: [u8; 3]) -> Op {
        Op::Text {
            row,
            col,
            s: s.into(),
            face: Face {
                fg: Some(fg),
                ..Face::default()
            },
        }
    }

    fn bar(row: u16, col: u16) -> Op {
        Op::Cursor {
            row,
            col,
            style: CursorStyle::Bar,
            color: None,
            blink: None,
            under: None,
        }
    }

    /// Two panes side by side — a line in each, a divider between — and a mode line.
    fn split_frame(left: &str, right: &str, caret: u16) -> Vec<Op> {
        vec![
            Op::Clear,
            text_at(0, 0, left, [200, 200, 200]),
            Op::Rect {
                row: 0,
                col: 15,
                w: 1,
                h: 3,
                face: Face {
                    bg: Some([60, 60, 60]),
                    ..Face::default()
                },
                radius: 0.0,
            },
            text_at(0, 16, right, [180, 220, 180]),
            bar(0, caret),
            text_at(2, 0, "mode line", [90, 90, 90]),
        ]
    }

    #[test]
    fn typing_a_character_repaints_only_the_cells_it_changed() {
        let renderer = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (renderer.cell_w, renderer.cell_h);
        let (fb_w, fb_h) = (32 * cw, 4 * ch);
        let (ox, oy) = renderer.grid_origin(fb_w, fb_h);
        let repainted = blink_run(
            &[
                (split_frame("let x", "other pane", 5), false),
                (split_frame("let x=", "other pane", 6), false),
                (split_frame("let x=", "other pane!", 6), false),
            ],
            fb_w,
            fb_h,
        );
        // the line's text grew a cell and the caret moved one: columns 0..7, not the
        // divider or the pane beside it
        assert_eq!(
            repainted[1],
            vec![DamageRect {
                x0: ox,
                y0: oy,
                x1: ox + 7 * cw,
                y1: oy + ch,
            }]
        );
        // a change in the other pane is that pane's cells alone
        assert_eq!(
            repainted[2],
            vec![DamageRect {
                x0: ox + 16 * cw,
                y0: oy,
                x1: ox + 27 * cw,
                y1: oy + ch,
            }]
        );
    }

    /// Every op kind that has columns, changed one at a time inside a row that keeps the
    /// rest, paints what a full raster paints — tabs, wide clusters, scaled text, a
    /// sub-cell rect, pixel text, a turned quad, bitboards and spans.
    #[test]
    fn column_damage_matches_a_full_raster_for_every_op_kind() {
        let renderer = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (renderer.cell_w, renderer.cell_h);
        let (fb_w, fb_h) = (40 * cw, 6 * ch);
        let (_, oy) = renderer.grid_origin(fb_w, fb_h);
        let band = Op::Rect {
            row: 1,
            col: 0,
            w: 40,
            h: 1,
            face: Face {
                bg: Some([30, 30, 50]),
                ..Face::default()
            },
            radius: 0.0,
        };
        let base = |variant: usize| {
            let pick = |n: usize| variant == n;
            let mut ops = vec![
                Op::Clear,
                band.clone(),
                text_at(
                    1,
                    2,
                    if pick(1) { "a\tbc" } else { "a\tb" },
                    [220, 220, 220],
                ),
                text_at(
                    1,
                    20,
                    if pick(2) { "日本語" } else { "日本" },
                    [200, 180, 160],
                ),
                Op::Text {
                    row: 2,
                    col: if pick(3) { 5 } else { 4 },
                    s: "Ab".into(),
                    face: Face {
                        fg: Some([250, 250, 250]),
                        bg: Some([10, 60, 10]),
                        scale: 2,
                        ..Face::default()
                    },
                },
                Op::FRect {
                    x: if pick(4) { 11.6 } else { 10.3 },
                    y: 1.2,
                    w: 3.5,
                    h: 0.6,
                    face: Face {
                        bg: Some([200, 90, 90]),
                        ..Face::default()
                    },
                    opacity: 0.5,
                    radius: 0.2,
                },
                Op::TextPx {
                    x: if pick(5) {
                        31.5 * cw as f32
                    } else {
                        30.0 * cw as f32
                    },
                    y: (oy + ch) as f32 + 1.0,
                    s: "px".into(),
                    face: Face {
                        fg: Some([90, 200, 250]),
                        ..Face::default()
                    },
                    align: crate::host::gui::Align::Center,
                },
                Op::Quad {
                    x: 36.0 * cw as f32,
                    y: (oy + ch) as f32,
                    w: cw as f32 * 2.0,
                    h: ch as f32 * 0.5,
                    color: [255, 255, 0, 200],
                    rot: if pick(6) { 0.7 } else { 0.3 },
                },
                Op::Cells {
                    row0: 4,
                    col0: 3,
                    w: 8,
                    aspect: 2,
                    bytes: vec![if pick(7) { 0b1010_0101 } else { 0b0101_1010 }],
                    color: Some([120, 250, 120]),
                },
                Op::VSpans {
                    row0: 4,
                    col0: 25,
                    cols: vec![vec![(1, Some([250, 120, 120]))], vec![(1, None)]]
                        .into_iter()
                        .map(|mut col| {
                            if pick(8) {
                                col[0].1 = Some([10, 10, 250]);
                            }
                            col
                        })
                        .collect(),
                },
                bar(1, if pick(9) { 9 } else { 8 }),
                text_at(5, 0, "mode line", [90, 90, 90]),
            ];
            if pick(10) {
                // an op added mid-row: the middles differ in length
                ops.insert(3, text_at(1, 12, "new", [255, 255, 255]));
            }
            ops
        };
        let mut steps = Vec::new();
        for variant in 0..=10 {
            steps.push((base(0), false));
            steps.push((base(variant), false));
        }
        let repainted = blink_run(&steps, fb_w, fb_h);
        // every change but the cell-wide ones stayed inside its row's columns
        let narrow = repainted
            .iter()
            .flatten()
            .filter(|rect| rect.x1 - rect.x0 < fb_w)
            .count();
        assert!(narrow > 0, "{repainted:?}");
    }

    /// Rasterising into a rect touches nothing outside it and paints, inside it, what a
    /// full raster paints — every primitive honours the column clip as it does the rows.
    #[test]
    fn a_raster_confined_to_a_rect_paints_only_inside_it_and_exactly_there() {
        let mut renderer = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (renderer.cell_w, renderer.cell_h);
        let (fb_w, fb_h) = (24 * cw, 6 * ch);
        let (ox, oy) = renderer.grid_origin(fb_w, fb_h);
        let frame = vec![
            Op::Clear,
            text_at(0, 0, "wide 日本 and text", [230, 230, 230]),
            Op::Text {
                row: 1,
                col: 1,
                s: "Big".into(),
                face: Face {
                    fg: Some([255, 255, 255]),
                    bg: Some([50, 20, 20]),
                    scale: 2,
                    underline: true,
                    ..Face::default()
                },
            },
            Op::Rect {
                row: 3,
                col: 2,
                w: 12,
                h: 2,
                face: Face {
                    bg: Some([20, 60, 20]),
                    ..Face::default()
                },
                radius: 0.4,
            },
            Op::FRect {
                x: 3.3,
                y: 0.4,
                w: 9.0,
                h: 4.2,
                face: Face {
                    bg: Some([90, 90, 200]),
                    ..Face::default()
                },
                opacity: 0.6,
                radius: 0.5,
            },
            Op::Quad {
                x: 5.0 * cw as f32,
                y: (oy + 2 * ch) as f32,
                w: 6.0 * cw as f32,
                h: ch as f32,
                color: [250, 200, 0, 180],
                rot: 0.5,
            },
            Op::Cursor {
                row: 4,
                col: 6,
                style: CursorStyle::Block,
                color: None,
                blink: None,
                under: None,
            },
            Op::Cursor {
                row: 2,
                col: 9,
                style: CursorStyle::Hollow,
                color: None,
                blink: None,
                under: None,
            },
            Op::Cursor {
                row: 5,
                col: 7,
                style: CursorStyle::Underline,
                color: None,
                blink: None,
                under: None,
            },
            Op::CellRegion {
                x: 14,
                y: 2,
                w: 8,
                h: 3,
                px: 20.0,
                ops: vec![text_at(0, 0, "region", [200, 200, 100])],
            },
        ];
        let mut full = vec![0u32; fb_w * fb_h];
        let bg0 = pack(renderer.bg());
        {
            let mut canvas = Canvas::full(&mut full, fb_w, fb_h);
            canvas.fill_rows(0, fb_h, bg0);
            render_ops(
                &frame,
                &mut canvas,
                &mut renderer,
                ox,
                &RowMap::uniform(oy, ch),
                cw,
                bg0,
                0,
            );
        }
        const GARBAGE: u32 = 0x00de_ad42;
        // rects that cut through glyphs, a scaled cell, rounded corners, the turned quad,
        // each caret's rim and the cell region
        for (x0, y0, x1, y1) in [
            (ox + cw / 2, oy, ox + 9 * cw + 3, oy + ch),
            (ox + 2 * cw + 1, oy + ch, ox + 5 * cw - 2, oy + 3 * ch),
            (ox + 3 * cw, oy + 3 * ch, ox + 13 * cw + 5, oy + 5 * ch),
            (ox + 6 * cw + 2, oy + 2 * ch, ox + 9 * cw + 7, oy + 3 * ch),
            (ox + 15 * cw + 3, oy + 2 * ch, fb_w, oy + 4 * ch),
            (0, 0, fb_w, fb_h),
        ] {
            let mut pixels = vec![GARBAGE; fb_w * fb_h];
            {
                let mut canvas = Canvas::rect(&mut pixels, fb_w, fb_h, (x0, y0), (x1, y1));
                canvas.fill_rows(y0, y1 - y0, bg0);
                render_ops(
                    &frame,
                    &mut canvas,
                    &mut renderer,
                    ox,
                    &RowMap::uniform(oy, ch),
                    cw,
                    bg0,
                    0,
                );
            }
            for y in 0..fb_h {
                for x in 0..fb_w {
                    let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
                    let expected = if inside { full[y * fb_w + x] } else { GARBAGE };
                    assert_eq!(
                        pixels[y * fb_w + x],
                        expected,
                        "pixel ({x}, {y}) for the rect {x0}..{x1} x {y0}..{y1}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_strip_whose_ops_have_no_known_columns_is_repainted_whole() {
        let strips = Strips::new(RowMap::uniform(0, 10), 40);
        let columns = |new: &[Op], old: &[Op]| {
            let (a, b) = (
                Leaves::of(new, &strips, false),
                Leaves::of(old, &strips, false),
            );
            strip_columns(&a, &b, 1, 200, 0, 8, false)
        };
        let row = |s: &str| vec![Op::Clear, text_at(0, 2, s, [200, 200, 200])];
        assert_eq!(columns(&row("abcd"), &row("abc")), (16, 48));
        // a clear that came or went reaches every column
        let mut cleared = row("abcd");
        cleared.remove(0);
        assert_eq!(columns(&cleared, &row("abcd")), (0, 200));
        // so does a cell region, whose width the painter does not clip
        let region = |s: &str| {
            vec![
                Op::Clear,
                Op::CellRegion {
                    x: 2,
                    y: 0,
                    w: 4,
                    h: 1,
                    px: 14.0,
                    ops: vec![text_at(0, 0, s, [200, 200, 200])],
                },
            ]
        };
        assert_eq!(columns(&region("b"), &region("a")), (0, 200));
        // a change off the right edge is the row whole rather than an empty rect
        let far = |s: &str| vec![Op::Clear, text_at(0, 400, s, [200, 200, 200])];
        assert_eq!(columns(&far("b"), &far("a")), (0, 200));
    }
}
