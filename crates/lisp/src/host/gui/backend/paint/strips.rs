//! The incremental half of a paint: what changed between two frames. Per cell row,
//! `strip_differs` says whether its ops differ (`grid_damage` over a grid), `strip_pieces`
//! narrows a row to the columns its changed ops can paint, and `strip_blits` finds rows (or
//! columns of rows) that only moved, so their pixels are copied rather than drawn;
//! `grid_plan` puts them together for any grid, the window's or a zoomed pane's
//! (`region_plan`). Pure over the ops and the row map: nothing here touches a pixel.

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
        Op::Sound { .. } | Op::Music { .. } | Op::Ambience { .. } => None,
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
#[derive(Clone, Copy)]
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
    /// The strips `per_strip` is laid out on: what `covering` finds a band's leaves through.
    strips: Strips,
    caret_hidden: bool,
    /// Per leaf, the old frame's leaf it is diffed inside of rather than compared with
    /// (`NO_TWIN` for none): a cell region at the same rect and size in both frames, whose
    /// changed cells `region_plan` finds (`pair_regions`).
    twins: Vec<u32>,
    /// The twins (new leaf, old leaf) that are a zoomed pane moved or resized (`twin`), which
    /// the window's diff leaves to the pane's own plan (`differing_leaves`).
    moved_twins: Vec<(u32, u32)>,
    /// Per leaf, its columns (`leaf_columns`) once worked out, on the grid `measured_on`: a
    /// scroll asks each leaf's columns for every strip, span and delta it tries, and a text
    /// op's are a walk of its graphemes.
    columns: Vec<std::cell::OnceCell<Option<(isize, isize)>>>,
    measured_on: std::cell::Cell<Option<Across>>,
    /// The leaves an anchor of the next frame can pair with (`partners_of`), indexed once asked.
    partners: std::cell::OnceCell<Partners<'a>>,
}

/// A frame's leaves as `blit_offsets` looks them up: each text's by its string, and the carets
/// and one-row bands, the only other leaves an anchor pairs with.
struct Partners<'a> {
    texts: std::collections::HashMap<
        &'a str,
        Vec<u32>,
        std::hash::BuildHasherDefault<super::super::render::PaintHasher>,
    >,
    marks: Vec<u32>,
}

impl<'a> Partners<'a> {
    fn of(leaves: &[Entry<'a>]) -> Partners<'a> {
        let mut partners = Partners {
            texts: Default::default(),
            marks: Vec::new(),
        };
        for (index, leaf) in leaves.iter().enumerate() {
            match leaf.op {
                Op::Text { s, .. } => partners
                    .texts
                    .entry(s.as_str())
                    .or_default()
                    .push(index as u32),
                Op::Cursor { .. } => partners.marks.push(index as u32),
                Op::Rect { h, .. } if *h <= 1 => partners.marks.push(index as u32),
                _ => {}
            }
        }
        partners
    }
}

/// No twin: the leaf is compared with the old frame's as a value.
const NO_TWIN: u32 = u32::MAX;

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
        let twins = vec![NO_TWIN; leaves.len()];
        let columns = vec![std::cell::OnceCell::new(); leaves.len()];
        Leaves {
            leaves,
            bands,
            per_strip,
            strips: *strips,
            caret_hidden,
            twins,
            moved_twins: Vec::new(),
            columns,
            measured_on: std::cell::Cell::new(None),
            partners: std::cell::OnceCell::new(),
        }
    }

    /// Leaf `i`'s pixel columns on the grid `across` (`leaf_columns`), worked out once. The
    /// leaves of a frame are measured on one grid; asked on another, they are worked out afresh.
    fn columns(&self, i: usize, across: Across) -> Option<(isize, isize)> {
        let measure = || leaf_columns(self.leaves[i].op, across.ox, across.cw, across.ligatures);
        match self.measured_on.get() {
            None => self.measured_on.set(Some(across)),
            Some(grid) if grid != across => return measure(),
            Some(_) => {}
        }
        *self.columns[i].get_or_init(measure)
    }

    /// Leaf `i`'s op.
    pub(super) fn op(&self, i: usize) -> &'a Op {
        self.leaves[i].op
    }

    /// Leaf `i`'s pixel band, `None` for a leaf that paints nothing.
    pub(super) fn band(&self, i: usize) -> Option<(isize, isize)> {
        self.bands[i]
    }

    /// Diff leaf `i` inside the old frame's leaf `j` rather than compare the two (`same_leaf`):
    /// a zoomed pane moved or resized between the frames, whose own grid plans its pixels
    /// (`region_plan`).
    pub(super) fn twin(&mut self, i: usize, j: usize) {
        self.twins[i] = j as u32;
        self.moved_twins.push((i as u32, j as u32));
    }

    /// Say leaf `i`'s columns on the grid `across` where `leaf_columns` cannot: a zoomed pane
    /// whose pixels keep to its rect (`region_extent`), whose ops are on another grid. Kept
    /// only on the grid the leaves are measured on.
    pub(super) fn know_columns(&self, i: usize, across: Across, columns: (isize, isize)) {
        if self.measured_on.get().is_none() {
            self.measured_on.set(Some(across));
        }
        if self.measured_on.get() == Some(across) {
            let _ = self.columns[i].set(Some(columns));
        }
    }

    /// The leaves of this (old) frame `blit_offsets` can pair `op`, an anchor whose pixel top is
    /// `top`, with, in frame order: for a text, the texts of the same string anywhere and every
    /// leaf on the rows `moved_by` above it (where a line cut otherwise lies: scrolled sideways,
    /// or down and sideways at once); for a caret or a one-row band, every caret and one-row
    /// band. Found through an index built once a frame is asked (`Partners`), not by a walk of
    /// every leaf per anchor: a strip where nothing moved asks for each of its leaves.
    fn partners_of(&self, op: &Op, top: isize, moved_by: &[isize]) -> Vec<usize> {
        let partners = self.partners.get_or_init(|| Partners::of(&self.leaves));
        let widen = |indices: &[u32]| indices.iter().map(|&j| j as usize).collect::<Vec<_>>();
        let mut found = match op {
            Op::Text { s, .. } => {
                let mut found = partners
                    .texts
                    .get(s.as_str())
                    .map_or(Vec::new(), |v| widen(v));
                for &dy in moved_by {
                    let row = top - dy;
                    if row >= 0 {
                        found.extend(self.covering(row as usize, row as usize + 1));
                    }
                }
                found
            }
            Op::Cursor { .. } | Op::Rect { .. } => widen(&partners.marks),
            _ => Vec::new(),
        };
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The leaves whose band overlaps the pixel rows `[y0, y1)`, in frame order — what
    /// a strip's `per_strip` list is, for an arbitrary band. Found through the strips the band
    /// meets (a leaf meeting the band meets one of them), not by a walk of every leaf: a
    /// scroll asks it for every strip it tries to copy.
    fn covering(&self, y0: usize, y1: usize) -> Vec<usize> {
        let (y0, y1) = (y0 as isize, y1 as isize);
        let overlaps =
            |&i: &usize| matches!(self.bands[i], Some((top, bottom)) if top < y1 && bottom > y0);
        let met = self.strips.covering(y0, y1);
        if met.len() == 1 {
            return self.per_strip[met.start]
                .iter()
                .map(|&i| i as usize)
                .filter(overlaps)
                .collect();
        }
        let mut leaves: Vec<usize> = self.per_strip[met]
            .iter()
            .flatten()
            .map(|&i| i as usize)
            .collect();
        leaves.sort_unstable();
        leaves.dedup();
        leaves.retain(overlaps);
        leaves
    }
}

/// Whether strip `k` must be re-rasterised to turn the frame `old` painted into the frame
/// `new` would paint: whether its sequence of covering leaf ops differs — an op added,
/// removed, changed, reordered, or shifted by a scroll region. The ops are compared as values
/// (`Op: PartialEq`), so an identical frame dirties nothing and a one-line edit dirties one
/// strip. A leaf of the same memoised fragment in both frames is the same op in memory
/// (ADR-470), so the pointer settles it before any field is compared; a blinking caret's strip
/// is dirty when the phase flipped (ADR-469). `strip_damage` and `grid_damage` diff a grid by
/// it.
pub(super) fn strip_differs(new: &Leaves, old: &Leaves, k: usize) -> bool {
    let (a, b) = (&new.per_strip[k], &old.per_strip[k]);
    a.len() != b.len()
        || a.iter()
            .zip(b)
            .any(|(&i, &j)| !same_leaf(new, i as usize, old, j as usize))
}

/// The damage of one strip: `None` when its leaves are the old ones, else the pixel columns
/// the change can reach — narrowed by `strip_columns` when `narrow`, the whole row `[0, fb_w)`
/// otherwise. THE per-strip diff: the window's grid (`grid_damage`), a zoomed pane's own grid
/// (`region_plan`) and a resized window's kept strips (`paint::resize_damage`) all ask it.
pub(super) fn strip_damage(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    fb_w: usize,
    across: Across,
    narrow: bool,
) -> Option<(usize, usize)> {
    // only a moved zoomed pane differs: its own plan draws it (`region_plan`)
    if !strip_differs(new, old, k) || differing_leaves(new, old, k).is_empty() {
        return None;
    }
    Some(if narrow {
        strip_columns(new, old, k, fb_w, across)
    } else {
        (0, fb_w)
    })
}

/// A grid's diff, strip by strip: whether each strip is dirty, and the pixel columns it is
/// drawn (or copied) over — the whole row for a clean one — and, once its copies are taken
/// (`strip_blits`), the columns it draws apart from those, further right (`apart`: two panes of
/// a split scrolled sideways opposite ways each draw the columns they brought in, at either end
/// of the row).
pub(super) struct GridDamage {
    pub(super) dirty: Vec<bool>,
    pub(super) columns: Vec<(usize, usize)>,
    pub(super) apart: Vec<Vec<(usize, usize)>>,
}

impl GridDamage {
    /// The rects it draws: the dirty strips over their columns, and each strip's columns apart,
    /// all merged down the strips (`dirty_rects`).
    pub(super) fn rects(&self, strips: &Strips) -> Vec<DamageRect> {
        let mut rects = dirty_rects(&self.dirty, &self.columns, strips);
        let layers = self.apart.iter().map(Vec::len).max().unwrap_or(0);
        for layer in 0..layers {
            let dirty: Vec<bool> = self.apart.iter().map(|spans| spans.len() > layer).collect();
            let columns: Vec<(usize, usize)> = self
                .apart
                .iter()
                .map(|spans| spans.get(layer).copied().unwrap_or((0, 0)))
                .collect();
            rects.extend(dirty_rects(&dirty, &columns, strips));
        }
        rects
    }
}

/// Diff `new` against `old`, both laid out on `strips`, with `across` the grid's columns
/// (`strip_damage` per strip): what the window's diff (`paint::diff_plan`) and a zoomed pane's
/// (`region_plan`) start from.
pub(super) fn grid_damage(
    new: &Leaves,
    old: &Leaves,
    strips: &Strips,
    fb_w: usize,
    across: Across,
    narrow: bool,
) -> GridDamage {
    let mut damage = GridDamage {
        dirty: vec![false; strips.count],
        columns: vec![(0, fb_w); strips.count],
        apart: vec![Vec::new(); strips.count],
    };
    for k in 0..strips.count {
        if let Some(columns) = strip_damage(new, old, k, fb_w, across, narrow) {
            damage.dirty[k] = true;
            damage.columns[k] = columns;
        }
    }
    damage
}

/// Whether leaf `i` of `new` paints exactly what leaf `j` of `old` painted: the same op
/// (by pointer first, ADR-470) under the same shift on the same pixel rows, and not a
/// blinking caret whose phase flipped in between (ADR-469). The band settles a window that
/// was resized (its rows below the seam moved); within one size the same op under the same
/// shift always has the same band. A cell region diffed inside (its twin, `pair_regions`)
/// counts as the same here: its changed cells are damaged by `region_plan`.
pub(super) fn same_leaf(new: &Leaves, i: usize, old: &Leaves, j: usize) -> bool {
    if new.twins[i] == j as u32 {
        return true;
    }
    let (x, y) = (&new.leaves[i], &old.leaves[j]);
    x.dy == y.dy
        && x.above_seam == y.above_seam
        && !(x.caret && new.caret_hidden != old.caret_hidden)
        && new.bands[i] == old.bands[j]
        && (std::ptr::eq(x.op, y.op) || x.op == y.op)
}

// ---- a cell region diffed as cells ------------------------------------------------
//
// A cell region (a zoomed pane) is one leaf of the frame: its ops are on its own grid, so
// the window's strips cannot place them. Compared as a value, a keystroke in a zoomed pane
// changes the region and repaints every row of it. But a region in both frames is a small
// frame of its own, laid out on a uniform grid from its top: its ops diff the way the
// window's do, strip by strip and column by column, on that grid. Its pixels are its ops'
// over the ground the window's leaves before it laid, so where neither those leaves nor its
// own changed, nothing did. A region that moved or changed size is that grid with its old
// pixels carried to its new top-left first — the way a resized window keeps its canvas.

/// A cell region of `new` and the old frame's region it is diffed inside of (`pair_regions`):
/// in the same place, or `moved` — another rect at the same cell size.
#[derive(Clone, Copy)]
pub(super) struct RegionPair {
    pub(super) new: usize,
    pub(super) old: usize,
    pub(super) moved: bool,
}

/// Pair each cell region of `new` with the old frame's region of the same cell size and
/// seam group, taken in frame order, when the two differ. A pair in the same place — the same
/// rect and shift — is marked twins here (`same_leaf` then lets the window's diff pass them);
/// a moved one is left for the caller, which twins it only when both keep their pixels to
/// their rects (`region_extent`), the one thing `region_plan` needs to carry them across.
pub(super) fn pair_regions(new: &mut Leaves, old: &Leaves) -> Vec<RegionPair> {
    let regions = |leaves: &Leaves| -> Vec<usize> {
        (0..leaves.leaves.len())
            .filter(|&i| matches!(leaves.leaves[i].op, Op::CellRegion { .. }))
            .collect()
    };
    let (new_regions, old_regions) = (regions(new), regions(old));
    let mut pairs = Vec::new();
    for (&i, &j) in new_regions.iter().zip(&old_regions) {
        let (x, y) = (&new.leaves[i], &old.leaves[j]);
        let placed = |op: &Op| match op {
            Op::CellRegion { x, y, w, h, .. } => Some((*x, *y, *w, *h)),
            _ => None,
        };
        let size = |op: &Op| match op {
            Op::CellRegion { px, .. } => Some(px.to_bits()),
            _ => None,
        };
        if x.above_seam != y.above_seam || size(x.op) != size(y.op) || same_leaf(new, i, old, j) {
            continue;
        }
        let in_place = x.dy == y.dy && new.bands[i] == old.bands[j] && placed(x.op) == placed(y.op);
        if in_place {
            new.twins[i] = j as u32;
        }
        pairs.push(RegionPair {
            new: i,
            old: j,
            moved: !in_place,
        });
    }
    pairs
}

/// The pixel columns of cell region `op` on the window's grid `across` — its rect's, from
/// `across.ox + x * across.cw`, `w` window cells wide — when every pixel it paints keeps to
/// them; `None` when one may not. `render_ops` clips a region's rows to its rect, never its
/// columns, so this asks its ops, on its own grid (`metrics` cells from its left edge): each
/// a cell-grid op whose columns (`leaf_columns`) lie inside the rect. A pixel-space op
/// (pixel text, a quad, a sprite), a nested region or a group above a seam is placed by more
/// than the region's cells, and answers `None`.
pub(super) fn region_extent(
    op: &Op,
    across: Across,
    (cell_w, cell_h): (usize, usize),
) -> Option<(isize, isize)> {
    let Op::CellRegion { x, w, ops, .. } = op else {
        return None;
    };
    let left = across.ox as isize + *x as isize * across.cw as isize;
    let right = left + *w as isize * across.cw as isize;
    let mut leaves = Vec::new();
    flatten(ops, cell_h, 0, false, &mut leaves);
    let keeps = leaves.iter().all(|leaf| match leaf.op {
        Op::CursorZone { .. }
        | Op::Sound { .. }
        | Op::Music { .. }
        | Op::Ambience { .. }
        | Op::Burst { .. } => true,
        Op::Text { .. }
        | Op::Cursor { .. }
        | Op::Rect { .. }
        | Op::FRect { .. }
        | Op::Image { .. }
        | Op::VSpans { .. }
        | Op::Cells { .. }
        | Op::CellsRgb { .. }
            if !leaf.above_seam =>
        {
            leaf_columns(leaf.op, left.max(0) as usize, cell_w, across.ligatures)
                .is_some_and(|(from, to)| from >= left && to <= right)
        }
        _ => false,
    });
    (keeps && left >= 0).then_some((left, right))
}

/// The window's grid across: where cell column 0 is (`ox`), the cell width, and whether
/// punctuation runs are one unit (ligatures) — what a leaf's columns are measured on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Across {
    pub(super) ox: usize,
    pub(super) cw: usize,
    pub(super) ligatures: bool,
}

/// Whether the window's leaves over the pixels `rows` × `columns` paint them as its leaves
/// over the pixels `offset` back painted them: what a zoomed pane's copy needs of the ground
/// under it and anything drawn over it. `leaves_translate` over the window's leaves on
/// `window`'s rows, the pane `twin` (new leaf, old leaf) standing for itself, its own ops
/// checked on its own grid; both ends on the `fb_w`×`fb_h` framebuffer.
#[allow(clippy::too_many_arguments)]
fn window_translates(
    new: &Leaves,
    old: &Leaves,
    twin: (usize, usize),
    window: &Strips,
    across: Across,
    (fb_w, fb_h): (usize, usize),
    offset: Offset,
    (y0, y1): (usize, usize),
    columns: (usize, usize),
) -> bool {
    let (src0, src1) = (y0 as isize - offset.dy, y1 as isize - offset.dy);
    let (left, right) = (
        columns.0 as isize - offset.dx,
        columns.1 as isize - offset.dx,
    );
    if src0 < 0 || src1 > fb_h as isize || left < 0 || right > fb_w as isize {
        return false;
    }
    let source_columns = (left as usize, right as usize);
    let new_leaves = new
        .covering(y0, y1)
        .into_iter()
        .filter(|&n| reaches(new, n, across, columns));
    let old_leaves = old
        .covering(src0 as usize, src1 as usize)
        .into_iter()
        .filter(|&o| reaches(old, o, across, source_columns));
    leaves_translate(
        new,
        old,
        new_leaves,
        old_leaves,
        offset,
        ((y0, y1), columns),
        &window.rows,
        across,
        Some(twin),
    )
}

/// The plan of twin regions `pair` (`pair_regions`): the region's own frame planned on its
/// grid — a uniform one from its top, `metrics` cells, its cell column 0 at its left edge,
/// `across.ox + x * across.cw`, as `render_ops` places it. In place, its strips diff and copy
/// as the window's do (`grid_plan`), within its band, a copy taken only where the window's own
/// leaves over those pixels translate with it (`window_translates`); the rects are cut to its
/// band. Moved or resized: `moved_region_plan`.
#[allow(clippy::too_many_arguments)]
pub(super) fn region_plan(
    new: &Leaves,
    old: &Leaves,
    pair: RegionPair,
    window: &Strips,
    across: Across,
    metrics: (usize, usize),
    (fb_w, fb_h): (usize, usize),
    switches: (bool, bool),
) -> Plan {
    let (i, j) = (pair.new, pair.old);
    if pair.moved {
        return moved_region_plan(
            new,
            old,
            pair,
            window,
            across,
            metrics,
            (fb_w, fb_h),
            switches,
        );
    }
    let (
        Op::CellRegion {
            x, ops: new_ops, ..
        },
        Op::CellRegion { ops: old_ops, .. },
    ) = (new.leaves[i].op, old.leaves[j].op)
    else {
        return Plan::nothing();
    };
    let Some((top, bottom)) = new.bands[i] else {
        return Plan::nothing();
    };
    let (cell_w, cell_h) = metrics;
    let inner_rows = RowMap::uniform(top.max(0) as usize, cell_h);
    let inner = Strips::new(inner_rows, fb_h);
    let new_inner = Leaves::of(new_ops, &inner, new.caret_hidden);
    let old_inner = Leaves::of(old_ops, &inner, old.caret_hidden);
    // the region's own grid across: its cell column 0 at its left edge, its cell width
    let region_across = Across {
        ox: across.ox + *x as usize * across.cw,
        cw: cell_w,
        ligatures: across.ligatures,
    };
    let (band_top, band_bottom) = (
        top.clamp(0, fb_h as isize) as usize,
        bottom.clamp(0, fb_h as isize) as usize,
    );
    let around = |offset: Offset, rows: (usize, usize), columns: (usize, usize)| {
        window_translates(
            new,
            old,
            (i, j),
            window,
            across,
            (fb_w, fb_h),
            offset,
            rows,
            columns,
        )
    };
    let bounds = BlitBounds {
        rows: (band_top, band_bottom),
        columns: (0, fb_w),
        around: Some(&around),
    };
    let plan = grid_plan(
        &new_inner,
        &old_inner,
        &inner,
        fb_w,
        region_across,
        &bounds,
        switches,
    );
    let band = DamageRect {
        x0: 0,
        y0: band_top,
        x1: fb_w,
        y1: band_bottom,
    };
    Plan {
        changed: cut_rects(plan.changed, band),
        raster: cut_rects(plan.raster, band),
        moves: plan.moves,
    }
}

/// `rects` cut to `within`, the ones left empty dropped.
pub(super) fn cut_rects(rects: Vec<DamageRect>, within: DamageRect) -> Vec<DamageRect> {
    rects
        .into_iter()
        .filter_map(|rect| {
            let cut = DamageRect {
                x0: rect.x0.max(within.x0),
                y0: rect.y0.max(within.y0),
                x1: rect.x1.min(within.x1),
                y1: rect.y1.min(within.y1),
            };
            (!cut.is_empty()).then_some(cut)
        })
        .collect()
}

/// The parts of `outer` outside `inner`: at most four rects — the rows above and below it, and
/// the columns left and right of it across its rows.
fn rect_less(outer: DamageRect, inner: DamageRect) -> Vec<DamageRect> {
    let overlap = cut_rects(vec![inner], outer);
    let Some(&inner) = overlap.first() else {
        return cut_rects(vec![outer], outer);
    };
    let parts = [
        DamageRect {
            y1: inner.y0,
            ..outer
        },
        DamageRect {
            y0: inner.y1,
            ..outer
        },
        DamageRect {
            x1: inner.x0,
            y0: inner.y0,
            y1: inner.y1,
            ..outer
        },
        DamageRect {
            x0: inner.x1,
            y0: inner.y0,
            y1: inner.y1,
            ..outer
        },
    ];
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

/// The plan of twin regions that moved, changed size, or both (`pair_regions`, each keeping
/// its pixels to its rect: `region_extent`, whose columns the leaves carry). The old pane's
/// pixels are anchored at the new pane's top-left, as a resized window's canvas is
/// (`anchored_damage`): every row of its grid inside the size both have is copied there from
/// the old pane's — a rectangle copy, offset by the pane's move — when the window's leaves over
/// it translate too (`window_translates`) and the blit is on (`blit`), and then drawn where its
/// ops changed (narrowed when `narrow`) and past the old width; any other row of the new pane is
/// drawn whole. The old
/// pane's rect outside the new one is drawn: what was under the pane shows there now.
#[allow(clippy::too_many_arguments)]
fn moved_region_plan(
    new: &Leaves,
    old: &Leaves,
    pair: RegionPair,
    window: &Strips,
    across: Across,
    (cell_w, cell_h): (usize, usize),
    (fb_w, fb_h): (usize, usize),
    (narrow, blit): (bool, bool),
) -> Plan {
    let (i, j) = (pair.new, pair.old);
    let rect = |leaves: &Leaves, leaf: usize| -> Option<(isize, isize, isize, isize)> {
        let (left, right) = leaves.columns(leaf, across)?;
        let (top, bottom) = leaves.bands[leaf]?;
        Some((left, top, right, bottom))
    };
    let on_screen = |(left, top, right, bottom): (isize, isize, isize, isize)| DamageRect {
        x0: left.clamp(0, fb_w as isize) as usize,
        y0: top.clamp(0, fb_h as isize) as usize,
        x1: right.clamp(0, fb_w as isize) as usize,
        y1: bottom.clamp(0, fb_h as isize) as usize,
    };
    let (
        Some(new_rect),
        Some(old_rect),
        Op::CellRegion { ops: new_ops, .. },
        Op::CellRegion { ops: old_ops, .. },
    ) = (
        rect(new, i),
        rect(old, j),
        new.leaves[i].op,
        old.leaves[j].op,
    )
    else {
        // not known where the pane keeps its pixels (`region_extent` twins only one that says):
        // the rows of both drawn across the window
        let rows = |leaves: &Leaves, leaf: usize| {
            leaves.bands[leaf].map(|(top, bottom)| on_screen((0, top, fb_w as isize, bottom)))
        };
        let rects: Vec<DamageRect> = [rows(new, i), rows(old, j)].into_iter().flatten().collect();
        return Plan {
            changed: rects.clone(),
            raster: rects,
            moves: Vec::new(),
        };
    };
    let (new_area, old_area) = (on_screen(new_rect), on_screen(old_rect));
    let uncovered = rect_less(old_area, new_area);
    let mut plan = Plan {
        changed: cut_rects(vec![new_area], new_area),
        raster: uncovered.clone(),
        moves: Vec::new(),
    };
    plan.changed.extend(uncovered);
    let (new_left, new_top, new_right, new_bottom) = new_rect;
    let (old_left, old_top, old_right, old_bottom) = old_rect;
    if new_top < 0 || old_top < 0 {
        // a pane above the window's top is drawn from its clipped top: not a translation
        plan.raster.extend(cut_rects(vec![new_area], new_area));
        return plan;
    }
    let offset = Offset {
        dx: new_left - old_left,
        dy: new_top - old_top,
    };
    // the old pane at the new one's top-left: the width and height both have, on screen at both
    let kept_bottom = new_top + (old_bottom - old_top);
    let copy_width = (new_right - new_left)
        .min(old_right - old_left)
        .min(fb_w as isize - new_left)
        .min(fb_w as isize - old_left)
        .max(0) as usize;
    let copied = (new_left as usize, new_left as usize + copy_width);
    let inner = Strips::new(RowMap::uniform(new_top as usize, cell_h), fb_h);
    let kept: Vec<bool> = (0..inner.count)
        .map(|k| {
            let (y0, y1) = inner.rows(k);
            k > 0
                && blit
                && copy_width > 0
                && y1 - y0 == cell_h
                && y1 as isize <= kept_bottom.min(new_bottom)
                && window_translates(
                    new,
                    old,
                    (i, j),
                    window,
                    across,
                    (fb_w, fb_h),
                    offset,
                    (y0, y1),
                    copied,
                )
        })
        .collect();
    plan.moves = (0..inner.count)
        .filter(|&k| kept[k])
        .map(|k| {
            let destination = inner.rows(k).0;
            StripMove {
                destination,
                source: (destination as isize - offset.dy) as usize,
                height: cell_h,
                columns: copied,
                source_left: old_left as usize,
            }
        })
        .collect();
    // the old ops laid out where the new ones are: what the copied pixels hold, diffed
    let new_inner = Leaves::of(new_ops, &inner, new.caret_hidden);
    let old_inner = Leaves::of(old_ops, &inner, old.caret_hidden);
    let region_across = Across {
        ox: new_left as usize,
        cw: cell_w,
        ligatures: across.ligatures,
    };
    plan.raster.extend(anchored_damage(
        &new_inner,
        &old_inner,
        &inner,
        &kept,
        Anchored {
            area: new_area,
            kept_right: copied.1,
        },
        fb_w,
        region_across,
        narrow,
    ));
    plan
}

/// Where a grid's old pixels are kept, anchored at its top-left: its `area` on the framebuffer,
/// and the column the kept pixels end at (past it, only the new size has pixels).
pub(super) struct Anchored {
    pub(super) area: DamageRect,
    pub(super) kept_right: usize,
}

/// What a grid draws over old pixels kept anchored at its top-left — a resized window's canvas
/// (`paint::resize_damage`), a moved or resized zoomed pane's rows copied to its new place
/// (`moved_region_plan`): the frame `new` diffed against `old` as the kept pixels lay it out,
/// both on `strips`. A strip whose pixels are kept (`kept[k]`) and whose leaves are the old ones
/// keeps them; one that changed is drawn in its changed columns (`strip_damage`, narrowed when
/// `narrow`); one not kept is drawn whole; and every kept strip past `kept_right` — the new
/// right edge, drawn apart, since a change at the left and the edge at the right are two
/// narrow rects, not one row. All cut to the grid's `area`.
#[allow(clippy::too_many_arguments)]
pub(super) fn anchored_damage(
    new: &Leaves,
    old: &Leaves,
    strips: &Strips,
    kept: &[bool],
    anchored: Anchored,
    fb_w: usize,
    across: Across,
    narrow: bool,
) -> Vec<DamageRect> {
    let area = anchored.area;
    let mut dirty = vec![false; strips.count];
    let mut columns = vec![(area.x0, area.x1); strips.count];
    let mut edge = vec![false; strips.count];
    let edge_columns = vec![(anchored.kept_right.max(area.x0), area.x1); strips.count];
    for k in 0..strips.count {
        let (y0, y1) = strips.rows(k);
        if y1.min(area.y1) <= y0.max(area.y0) {
            continue;
        }
        if !kept[k] {
            dirty[k] = true;
            continue;
        }
        edge[k] = area.x1 > anchored.kept_right;
        if let Some((x0, x1)) = strip_damage(new, old, k, fb_w, across, narrow) {
            let (x0, x1) = (x0.max(area.x0), x1.min(area.x1));
            if x1 > x0 {
                dirty[k] = true;
                columns[k] = (x0, x1);
            }
        }
    }
    let mut rects = dirty_rects(&dirty, &columns, strips);
    rects.extend(dirty_rects(&edge, &edge_columns, strips));
    cut_rects(rects, area)
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

/// The pixel columns of strip `k` that can differ between `old`'s raster and `new`'s, as pieces
/// `[x0, x1)` left to right: the columns of the differing leaves (`differing_columns`) merged
/// where they overlap, so two leaves side by side stay two — a gutter beside its text, a word
/// beside the next, two panes of a split. `None` when any of those leaves has no known columns:
/// then the whole row can differ. What `strip_blits` cuts a strip at when its parts moved by
/// different offsets.
pub(super) fn strip_pieces(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    fb_w: usize,
    across: Across,
) -> Option<Vec<(usize, usize)>> {
    let extents = differing_columns(new, old, k, fb_w, across)?;
    let mut pieces: Vec<(usize, usize)> = Vec::with_capacity(extents.len());
    for (x0, x1) in extents {
        match pieces.last_mut() {
            Some(last) if last.1 > x0 => last.1 = last.1.max(x1),
            _ => pieces.push((x0, x1)),
        }
    }
    Some(pieces)
}

/// The pixel columns of the leaves of strip `k` that differ between `old` and `new`
/// (`differing_leaves`), left edge first, each clamped to the `fb_w`-wide window (one wholly
/// off it dropped). `None` when any of those leaves has no known columns.
fn differing_columns(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    fb_w: usize,
    across: Across,
) -> Option<Vec<(usize, usize)>> {
    let differing = differing_leaves(new, old, k);
    let mut extents = Vec::with_capacity(differing.len());
    for (in_new, i) in differing {
        let columns = if in_new {
            new.columns(i, across)
        } else {
            old.columns(i, across)
        };
        let (from, to) = columns?;
        let (x0, x1) = (
            from.clamp(0, fb_w as isize) as usize,
            to.clamp(0, fb_w as isize) as usize,
        );
        if x1 > x0 {
            extents.push((x0, x1));
        }
    }
    extents.sort_unstable();
    Some(extents)
}

/// The leaves of strip `k` that differ between `old` and `new`, in either frame: outside the
/// strip's common prefix and suffix, and position by position when what is left is one length
/// in both. A zoomed pane that moved (`Leaves::twin`) is left out: its own plan draws its new
/// rect and what it uncovered (`region_plan`), so a strip it alone came to or left is clean.
/// Each leaf as `(in_new, index)`: whether it is `new`'s, and its index there.
fn differing_leaves(new: &Leaves, old: &Leaves, k: usize) -> Vec<(bool, usize)> {
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
    let mut differing: Vec<(bool, usize)> = Vec::new();
    if middle_new.len() == middle_old.len() {
        for (p, q) in middle_new.zip(middle_old) {
            if !same(p, q) {
                differing.push((true, a[p] as usize));
                differing.push((false, b[q] as usize));
            }
        }
    } else {
        differing.extend(middle_new.map(|p| (true, a[p] as usize)));
        differing.extend(middle_old.map(|q| (false, b[q] as usize)));
    }
    differing.retain(|&(in_new, i)| {
        if in_new {
            !new.moved_twins.iter().any(|&(twin, _)| twin as usize == i)
        } else {
            !new.moved_twins.iter().any(|&(_, twin)| twin as usize == i)
        }
    });
    differing
}

/// The pixel columns `[x0, x1)` of strip `k` that can differ between `old`'s raster and
/// `new`'s: the hull of its `strip_pieces`. The whole row when any differing leaf has no known
/// columns, or nothing the differing leaves paint is on screen (the row whole, rather than a
/// zero-width rect).
pub(super) fn strip_columns(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    fb_w: usize,
    across: Across,
) -> (usize, usize) {
    match strip_pieces(new, old, k, fb_w, across).as_deref() {
        Some([first, .., last]) => (first.0, last.1),
        Some([only]) => *only,
        _ => (0, fb_w),
    }
}

// ---- the scroll blit ----------------------------------------------------------
//
// A scroll changes every strip of a pane, so the strip diff alone re-rasterises the
// whole pane — 6 ms of glyph blitting at 1080p, four times that on a 4K display — for
// pixels the canvas already holds one line up or down, or a few columns aside. The blit
// finds them: a dirty strip whose ops are the previous frame's ops elsewhere, TRANSLATED by
// a whole number of pixels down and across (an `Offset`), paints exactly the pixels the
// retained canvas has there, so those pixels are copied instead of drawn. Only what really
// changed (the line scrolled in, the columns scrolled in, the mode line's `L12`, the
// scrollbar thumb) is rasterised.
//
// The rule is exact, not heuristic: a strip's pixels in some columns are a function of the
// leaves that reach them, each painted at a position; if the new strip's leaves over the
// destination are the old leaves over the source in the same order, each painting there
// what its partner painted there — the same op moved by the offset (a text line, the
// cursor, a one-row band), a text op whose glyphs over those columns are its partner's
// moved (a line scrolled sideways shows another slice of itself), or a solid fill that
// covers both on any axis it did not move along (the gutter wash, a divider, the frame clear
// — a fill has no position-dependent pixels away from its corners) — the pixels are
// identical. Bands and columns are conservative (`op_band`, `leaf_columns`), so a leaf that
// only nearly touches a strip can veto a blit but never fake one. One mechanism carries a
// scroll up or down, sideways, and both, and a zoomed pane moved to another place.

/// A translation of pixels, `dx` columns right and `dy` rows down: where a copy's pixels go
/// from where the canvas holds them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Offset {
    pub(super) dx: isize,
    pub(super) dy: isize,
}

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

/// The units a text op at cell `col` draws, each with its pixel left and width on a grid at
/// `across` — the `Text` arm of `render_ops`' walk: a raw tab to the next screen stop (its
/// background only), a zero-width unit not at all (left out), any other `unit_cells × scale`.
/// Only the units whose pixels meet the columns `[x0, x1)`.
fn text_units_over(
    s: &str,
    col: usize,
    scale: usize,
    across: Across,
    (x0, x1): (isize, isize),
) -> Vec<(&str, isize, isize)> {
    let mut units = Vec::new();
    let mut cell = col;
    for unit in text_units(s, across.ligatures) {
        let cells = if unit == "\t" {
            cluster_cells_at(unit, cell, TAB_WIDTH)
        } else {
            unit_cells(unit) * scale
        };
        let (left, width) = (
            (across.ox + cell * across.cw) as isize,
            (cells * across.cw) as isize,
        );
        cell += cells;
        if width > 0 && left < x1 && left + width > x0 {
            units.push((unit, left, width));
        }
        if left >= x1 {
            break;
        }
    }
    units
}

/// Whether text `new_s` at cell `new_col` paints the columns `columns` as text `old_s` at
/// `old_col`, in the same face, painted the columns `dx` to the left: the units over each pair
/// one to one — the same unit, as wide, `dx` further right. A unit's glyph, background and
/// underline are its cell block's alone (`Renderer::build_cluster`), so a unit cut by the
/// columns' edge is cut alike in both. What a pane scrolled sideways shows: another slice of
/// the same line.
#[allow(clippy::too_many_arguments)]
fn text_translates(
    (new_s, new_col): (&str, usize),
    (old_s, old_col): (&str, usize),
    scale: usize,
    dx: isize,
    (x0, x1): (usize, usize),
    across: Across,
) -> bool {
    let (x0, x1) = (x0 as isize, x1 as isize);
    let new_units = text_units_over(new_s, new_col, scale, across, (x0, x1));
    let old_units = text_units_over(old_s, old_col, scale, across, (x0 - dx, x1 - dx));
    new_units.len() == old_units.len()
        && new_units.iter().zip(&old_units).all(
            |((new_unit, new_left, new_width), (old_unit, old_left, old_width))| {
                new_unit == old_unit && new_width == old_width && *new_left == old_left + dx
            },
        )
}

/// The cell shifts that would carry text `old_s` at `old_col` onto text `new_s` at `new_col`
/// on one row: for each way the two line up — the first unit of one on a unit of the other
/// equal to it — the shift when every unit both have from there on agrees and they share at
/// least half the shorter. What a pane scrolled sideways draws: a line cut at another column,
/// its units moved. Nearest first, at most `SIDEWAYS_SHIFTS`.
fn text_shifts(
    (new_s, new_col): (&str, usize),
    (old_s, old_col): (&str, usize),
    scale: usize,
    across: Across,
) -> Vec<isize> {
    // the units line up only where the bytes do: one text starting inside the other and agreeing
    // to the end of either — checked first, since most pairs asked are two unrelated lines
    let starts_inside = |outer: &[u8], inner: &[u8]| {
        (0..outer.len()).any(|from| {
            let length = (outer.len() - from).min(inner.len());
            outer[from..from + length] == inner[..length]
        })
    };
    let (new_bytes, old_bytes) = (new_s.as_bytes(), old_s.as_bytes());
    if !starts_inside(old_bytes, new_bytes) && !starts_inside(new_bytes, old_bytes) {
        return Vec::new();
    }
    let everything = (isize::MIN, isize::MAX);
    let new_units = text_units_over(new_s, new_col, scale, across, everything);
    let old_units = text_units_over(old_s, old_col, scale, across, everything);
    let shortest = new_units.len().min(old_units.len());
    if shortest == 0 {
        return Vec::new();
    }
    // new unit `p` on old unit `p + lag`: every pair from there agrees, and as far apart
    let shift_at = |lag: isize| -> Option<isize> {
        let first = (-lag).max(0) as usize;
        let last = new_units
            .len()
            .min((old_units.len() as isize - lag).max(0) as usize);
        if last <= first || (last - first) * 2 < shortest {
            return None;
        }
        let shift = new_units[first].1 - old_units[(first as isize + lag) as usize].1;
        (first..last)
            .all(|p| {
                let (unit, left, width) = new_units[p];
                let (old_unit, old_left, old_width) = old_units[(p as isize + lag) as usize];
                unit == old_unit && width == old_width && left - old_left == shift
            })
            .then_some(shift)
    };
    let lags = (0..old_units.len())
        .filter(|&q| old_units[q].0 == new_units[0].0)
        .map(|q| q as isize)
        .chain(
            (1..new_units.len())
                .filter(|&p| new_units[p].0 == old_units[0].0)
                .map(|p| -(p as isize)),
        );
    let mut shifts: Vec<isize> = lags
        .filter_map(shift_at)
        .filter(|&shift| shift != 0)
        .collect();
    shifts.sort_unstable_by_key(|shift| shift.abs());
    shifts.dedup();
    shifts.truncate(SIDEWAYS_SHIFTS);
    shifts
}

/// How many sideways shifts `text_shifts` offers for one pair of texts.
const SIDEWAYS_SHIFTS: usize = 4;

/// Whether a fill whose pixel extent is `[new_from, new_to)` in the new frame and `[old_from,
/// old_to)` in the old paints the destination `[to0, to1)` as it painted the source `delta`
/// before it, along one axis: moved by `delta` with its extent (a band scrolled with its
/// line), or covering both by `margin` beyond (its edges and rounded corners away from them,
/// so the pixels between are the fill's wherever it is).
fn fill_translates(
    (new_from, new_to): (f32, f32),
    (old_from, old_to): (f32, f32),
    delta: isize,
    (to0, to1): (isize, isize),
    margin: f32,
) -> bool {
    let moved = ((new_to - new_from) - (old_to - old_from)).abs() < 1e-3
        && (new_from - old_from - delta as f32).abs() < 1e-3;
    let covers = |from: f32, to: f32, lo: isize, hi: isize| {
        from + margin <= lo as f32 && to - margin >= hi as f32
    };
    moved
        || (covers(new_from, new_to, to0, to1)
            && covers(old_from, old_to, to0 - delta, to1 - delta))
}

/// Whether painting `new` over the destination — the pixel rows `[y0, y1)` and columns
/// `[x0, x1)` — produces the pixels `old` painted over the source, `offset` back: the same op
/// moved by the offset, a text whose units over the columns are the old one's moved
/// (`text_translates`), or a fill that moved or covers both along each axis
/// (`fill_translates`: rounded corners kept `radius` clear, a hairline's snap slop on top).
pub(super) fn leaf_translates(
    new: &Entry,
    old: &Entry,
    offset: Offset,
    ((y0, y1), (x0, x1)): ((isize, isize), (usize, usize)),
    rows: &RowMap,
    across: Across,
) -> bool {
    // a leaf in a group above the seam is on other rows than one outside it (ADR-615)
    if new.above_seam != old.above_seam {
        return false;
    }
    let rows = &new.rows(rows);
    let (dx, dy) = (offset.dx, offset.dy);
    let cw = across.cw as isize;
    let top = |row: u16, shift: isize| rows.y(row as isize).saturating_sub(shift);
    let moved_by = |new_col: u16, old_col: u16| (new_col as isize - old_col as isize) * cw == dx;
    let left = |col: u16| (across.ox as isize + col as isize * cw) as f32;
    let columns = (x0 as isize, x1 as isize);
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
            let scale = nf.scale.max(1) as usize;
            if nf != of || top(*nr, new.dy) != top(*or, old.dy).saturating_add(dy) {
                return false;
            }
            // a wavy underline's phase is the screen column's (`underline_rects`)
            if dx != 0 && nf.underline && nf.underline_wave && dx % (4 * scale as isize) != 0 {
                return false;
            }
            // the op moved whole, unless a tab in it reaches another screen stop
            (ns == os && moved_by(*nc, *oc) && (dx == 0 || !ns.contains('\t')))
                || text_translates(
                    (ns, *nc as usize),
                    (os, *oc as usize),
                    scale,
                    dx,
                    (x0, x1),
                    across,
                )
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
            moved_by(*nc, *oc)
                && nst == ost
                && ncolor == ocolor
                && nblink == oblink
                && nunder == ounder
                && top(*nr, new.dy) == top(*or, old.dy).saturating_add(dy)
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
            if nf != of || nrad != orad {
                return false;
            }
            let (ntop, otop) = (top(*nr, new.dy), top(*or, old.dy));
            // heights through the map: the row above a seam is taller by its gap
            let (nheight, oheight) = (
                rows.span(*nr as isize, *nh as isize),
                rows.span(*or as isize, *oh as isize),
            );
            let margin = if *nrad > 0.0 {
                (nrad * cw as f32).ceil() + 1.0
            } else {
                0.0
            };
            let width = |w: u16| w as f32 * cw as f32;
            fill_translates(
                (ntop as f32, ntop.saturating_add(nheight) as f32),
                (otop as f32, otop.saturating_add(oheight) as f32),
                dy,
                (y0, y1),
                margin,
            ) && fill_translates(
                (left(*nc), left(*nc) + width(*nw)),
                (left(*oc), left(*oc) + width(*ow)),
                dx,
                columns,
                margin,
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
            if nf != of || nop != oop || nrad != orad {
                return false;
            }
            let ntop = rows.yf(*ny) - new.dy as f32;
            let otop = rows.yf(*oy_) - old.dy as f32;
            let (nheight, oheight) = (rows.spanf(*ny, *nh), rows.spanf(*oy_, *oh));
            let (ox, cw) = (across.ox as f32, cw as f32);
            let (nleft, oleft) = (ox + nx * cw, ox + ox_ * cw);
            if ![ntop, otop, nheight, oheight, nleft, oleft]
                .iter()
                .all(|value| value.is_finite())
            {
                return false;
            }
            // the same slop `op_band` allows a hairline's snap, plus the corners
            let margin = nrad * cw + 5.0;
            fill_translates(
                (ntop, ntop + nheight),
                (otop, otop + oheight),
                dy,
                (y0, y1),
                margin,
            ) && fill_translates(
                (nleft, nleft + nw * cw),
                (oleft, oleft + ow * cw),
                dx,
                columns,
                margin,
            )
        }
        // bitboards and column spans ignore the scroll shift and are never translated
        _ => false,
    }
}

/// Whether leaf `i` of `leaves` can paint a pixel in the columns `[x0, x1)`: its columns
/// (`leaf_columns`) meet them, or are not known.
fn reaches(leaves: &Leaves, i: usize, across: Across, (x0, x1): (usize, usize)) -> bool {
    leaves
        .columns(i, across)
        .is_none_or(|(left, right)| left < x1 as isize && right > x0 as isize)
}

/// How many of the new strip's leaves `blit_offsets` takes its candidates from.
const ANCHORS: usize = 3;

/// How many of the new strip's leaves `blit_offsets` looks at for its `ANCHORS`: a strip that
/// moved shows them among its first few, and one where nothing did (another buffer shown) has
/// none to find.
const ANCHOR_LEAVES: usize = 8;

/// How many of the offsets its anchors suggest `blit_offsets` answers, the likeliest first: a
/// text repeated across the old frame suggests an offset for each copy, which one anchor alone
/// backs.
const ANCHOR_OFFSETS: usize = 6;

/// The translations that could carry old pixels onto dirty strip `k` over the columns
/// `window`, from its first few translatable leaves that reach them (`ANCHORS` of the first
/// `ANCHOR_LEAVES`): for each old leaf equal to one up to its place — a text, a caret or a
/// band, anywhere — the offset between them, and for a text cut otherwise on the same row (a
/// line scrolled sideways) or on a row an earlier anchor moved by (scrolled down and sideways
/// at once) each shift that lines the two up (`text_shifts`); the old leaves found through
/// `Leaves::partners_of`. The ones more anchors agree on come first, and of those the moves
/// straight up or down: the candidates a strip's blit is tried at, in order. With them, the
/// column the anchors were looked for up to: the right edge of the last leaf looked at when the
/// looking stopped short, else the window's.
pub(super) fn blit_offsets(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    rows: &RowMap,
    across: Across,
    window: (usize, usize),
) -> (Vec<Offset>, usize) {
    let cw = across.cw as isize;
    // each offset found with how many anchors suggest it
    let mut offsets: Vec<(Offset, usize)> = Vec::new();
    let (mut anchors, mut looked) = (0, 0);
    let mut reach = window.1;
    for &i in &new.per_strip[k] {
        let leaf = &new.leaves[i as usize];
        let Some(top) = leaf_top(leaf.op, leaf.dy, &leaf.rows(rows)) else {
            continue;
        };
        // a whole-pane rect anchors nothing (it matches anywhere by covering)
        if matches!(leaf.op, Op::Rect { h, .. } if *h > 1) {
            continue;
        }
        // nor does a leaf beside the columns that changed (the other pane of a split)
        if !reaches(new, i as usize, across, window) {
            continue;
        }
        looked += 1;
        let mut suggested: Vec<Offset> = Vec::new();
        let mut moved_by: Vec<isize> = vec![0];
        moved_by.extend(offsets.iter().map(|(found, _)| found.dy));
        for j in old.partners_of(leaf.op, top, &moved_by) {
            let old_leaf = &old.leaves[j];
            let Some(old_top) = leaf_top(old_leaf.op, old_leaf.dy, &old_leaf.rows(rows)) else {
                continue;
            };
            let dy = top - old_top;
            let columns = match (leaf.op, old_leaf.op) {
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
                ) if fa == fb => {
                    if sa == sb {
                        vec![(*a as isize - *b as isize) * cw]
                    } else if dy == 0 || offsets.iter().any(|(found, _)| found.dy == dy) {
                        // a line cut otherwise on its own row (scrolled sideways), or on a row
                        // another anchor moved by (scrolled down and sideways at once)
                        text_shifts(
                            (sa, *a as usize),
                            (sb, *b as usize),
                            fa.scale.max(1) as usize,
                            across,
                        )
                    } else {
                        Vec::new()
                    }
                }
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
                ) if sa == sb && ca == cb && ua == ub => vec![(*a as isize - *b as isize) * cw],
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
                ) if wa == wb && ha == hb && fa == fb && ra == rb => {
                    vec![(*a as isize - *b as isize) * cw]
                }
                _ => Vec::new(),
            };
            for dx in columns {
                let offset = Offset { dx, dy };
                if offset != (Offset { dx: 0, dy: 0 }) && !suggested.contains(&offset) {
                    suggested.push(offset);
                }
            }
        }
        if !suggested.is_empty() {
            for offset in suggested {
                match offsets.iter_mut().find(|(found, _)| *found == offset) {
                    Some((_, votes)) => *votes += 1,
                    None => offsets.push((offset, 1)),
                }
            }
            anchors += 1;
        }
        if anchors == ANCHORS || looked == ANCHOR_LEAVES {
            reach = new
                .columns(i as usize, across)
                .map_or(window.1, |(_, right)| {
                    right.clamp(window.0 as isize, window.1 as isize) as usize
                });
            break;
        }
    }
    // the ones more anchors agree on first, and of those straight up or down: a scroll's, the
    // common move (a line of text repeated elsewhere suggests its own, which one anchor alone
    // backs)
    offsets.sort_by_key(|&(offset, votes)| (std::cmp::Reverse(votes), offset.dx != 0));
    offsets.truncate(ANCHOR_OFFSETS);
    (
        offsets.into_iter().map(|(offset, _)| offset).collect(),
        reach,
    )
}

/// Whether the new leaves `new_leaves`, painting the destination — pixel rows `[y0, y1)`,
/// columns `[x0, x1)` — are the old leaves `old_leaves` painting the source `offset` back,
/// translated: they pair one to one, in order, each under `leaf_translates` — except `twin`, a
/// zoomed pane's leaf in both frames, whose own ops are checked on its own grid
/// (`region_plan`), and which pairs only with itself. A caret whose blink phase flipped is not
/// the pixels the canvas holds anywhere.
#[allow(clippy::too_many_arguments)]
fn leaves_translate(
    new: &Leaves,
    old: &Leaves,
    new_leaves: impl Iterator<Item = usize>,
    mut old_leaves: impl Iterator<Item = usize>,
    offset: Offset,
    ((y0, y1), columns): ((usize, usize), (usize, usize)),
    rows: &RowMap,
    across: Across,
    twin: Option<(usize, usize)>,
) -> bool {
    let flipped = new.caret_hidden != old.caret_hidden;
    for i in new_leaves {
        let Some(j) = old_leaves.next() else {
            return false;
        };
        if flipped && new.leaves[i].caret {
            return false;
        }
        let translates = match twin {
            Some((twin_new, twin_old)) if i == twin_new || j == twin_old => {
                i == twin_new && j == twin_old
            }
            _ => leaf_translates(
                &new.leaves[i],
                &old.leaves[j],
                offset,
                ((y0 as isize, y1 as isize), columns),
                rows,
                across,
            ),
        };
        if !translates {
            return false;
        }
    }
    old_leaves.next().is_none()
}

/// Whether the columns `columns` of dirty strip `k` (pixel rows `[y0, y1)`) are the old
/// frame's pixels `offset` back translated: every new leaf covering the strip that reaches
/// those columns pairs, in order, with an old leaf covering the source rows that reaches the
/// source columns (`leaves_translate`). A leaf that cannot reach them paints none of their
/// pixels, in either frame, so the other pane of a side-by-side split has no say in whether
/// this one scrolled. The source must lie within `bounds`.
#[allow(clippy::too_many_arguments)]
pub(super) fn strip_translates(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    offset: Offset,
    (y0, y1): (usize, usize),
    columns: (usize, usize),
    bounds: &BlitBounds,
    rows: &RowMap,
    across: Across,
) -> bool {
    let (src0, src1) = (y0 as isize - offset.dy, y1 as isize - offset.dy);
    let (left, right) = (
        columns.0 as isize - offset.dx,
        columns.1 as isize - offset.dx,
    );
    if src0 < bounds.rows.0 as isize
        || src1 > bounds.rows.1 as isize
        || left < bounds.columns.0 as isize
        || right > bounds.columns.1 as isize
    {
        return false;
    }
    let source_columns = (left as usize, right as usize);
    let new_leaves = new.per_strip[k]
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| reaches(new, i, across, columns));
    let old_leaves = old
        .covering(src0 as usize, src1 as usize)
        .into_iter()
        .filter(|&j| reaches(old, j, across, source_columns));
    leaves_translate(
        new,
        old,
        new_leaves,
        old_leaves,
        offset,
        ((y0, y1), columns),
        rows,
        across,
        None,
    )
}

/// What `rows` × `columns` paints besides a grid's own leaves, checked by whoever placed the
/// grid: given a move's offset, its destination rows and its destination columns, whether those
/// paint the same pixels translated (`region_plan`'s window leaves around a zoomed pane).
pub(super) type Around<'a> = &'a dyn Fn(Offset, (usize, usize), (usize, usize)) -> bool;

/// Where a grid's strips may be copied: both ends of a move within the pixel rows `rows` and
/// columns `columns`, and `around` (`None`: nothing) agreeing that what else paints there
/// moved with it.
pub(super) struct BlitBounds<'a> {
    pub(super) rows: (usize, usize),
    pub(super) columns: (usize, usize),
    pub(super) around: Option<Around<'a>>,
}

impl BlitBounds<'_> {
    /// The window's own grid: every pixel of an `fb_w`×`fb_h` framebuffer, nothing else
    /// painting.
    pub(super) fn whole((fb_w, fb_h): (usize, usize)) -> BlitBounds<'static> {
        BlitBounds {
            rows: (0, fb_h),
            columns: (0, fb_w),
            around: None,
        }
    }
}

/// The columns of `window` a move by `dx` can copy within `bounds`: the ones whose source,
/// `dx` to the left, is in the window and the bounds too — the rest of the window is what a
/// sideways scroll brought in, drawn.
fn copied_columns(window: (usize, usize), dx: isize, bounds: (usize, usize)) -> (usize, usize) {
    let shifted = |(x0, x1): (usize, usize)| (x0 as isize + dx, x1 as isize + dx);
    let (window_from, window_to) = shifted(window);
    let (bounds_from, bounds_to) = shifted(bounds);
    let from = (window.0 as isize)
        .max(window_from)
        .max(bounds.0 as isize)
        .max(bounds_from);
    let to = (window.1 as isize)
        .min(window_to)
        .min(bounds.1 as isize)
        .min(bounds_to);
    if to <= from {
        (0, 0)
    } else {
        (from as usize, to as usize)
    }
}

/// The columns of `window` a move by `offset` copies onto dirty strip `k`, when it carries them:
/// all of them, or all but the ones a sideways move brought in (`copied_columns`) — at least as
/// many copied as brought in, or the window is more likely two things that moved apart than one
/// that moved far — every leaf over them translating (`strip_translates`) and what else paints
/// there agreeing (`bounds.around`).
#[allow(clippy::too_many_arguments)]
fn carried(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    offset: Offset,
    window: (usize, usize),
    strips: &Strips,
    bounds: &BlitBounds,
    across: Across,
) -> Option<(usize, usize)> {
    let destination = strips.rows(k);
    let columns = copied_columns(window, offset.dx, bounds.columns);
    ((columns.1 - columns.0) * 2 >= window.1 - window.0
        && columns.1 > columns.0
        && strip_translates(
            new,
            old,
            k,
            offset,
            destination,
            columns,
            bounds,
            &strips.rows,
            across,
        )
        && bounds
            .around
            .is_none_or(|around| around(offset, destination, columns)))
    .then_some(columns)
}

/// No move: columns a strip's diff counts as changed whose leaves paint what they painted where
/// they painted it — a divider or a gutter the diff could not line up beside text that changed
/// its count of leaves. Carried by it, they are neither copied nor drawn.
const STILL: Offset = Offset { dx: 0, dy: 0 };

/// The first of `offsets` that carries the columns `columns` of strip `k` (`carried`), with the
/// columns it copies, each one tried counted in `tries`.
#[allow(clippy::too_many_arguments)]
fn first_carrying(
    offsets: &[Offset],
    new: &Leaves,
    old: &Leaves,
    k: usize,
    columns: (usize, usize),
    strips: &Strips,
    bounds: &BlitBounds,
    across: Across,
    tries: &mut usize,
) -> Option<(Offset, (usize, usize))> {
    offsets.iter().find_map(|&offset| {
        *tries += 1;
        carried(new, old, k, offset, columns, strips, bounds, across).map(|copied| (offset, copied))
    })
}

/// The offsets a dirty strip's columns are tried at, in order: the ones that carried strips
/// before (`recent`) — a scroll moves a whole pane by one offset, two panes by two — then the
/// ones the strip's own anchors suggest (`blit_offsets`), which walk the old frame and so are
/// worked out only when no offset known carries what is tried: over the strip's changed
/// columns `window` from the columns tried on, and again for columns past the ones the anchors
/// so far were found in (`anchored_to`) when those carried something (`anchors_paid`) — the
/// other pane of a split, scrolled its own way, and not a strip where nothing moved. It
/// counts the offsets it tried (`tries`), the work a strip spent looking for its copies.
struct Candidates {
    recent: Vec<Offset>,
    window: (usize, usize),
    anchored: Vec<Offset>,
    anchored_to: Option<usize>,
    anchors_paid: bool,
    tries: usize,
}

impl Candidates {
    fn new(recent: &[Offset], window: (usize, usize)) -> Candidates {
        Candidates {
            recent: recent.to_vec(),
            window,
            anchored: Vec::new(),
            anchored_to: None,
            anchors_paid: false,
            tries: 0,
        }
    }

    /// The first offset known so far — a recent one, no move at all (`STILL`), or one the
    /// anchors found — that carries the columns `columns` of strip `k` (`carried`), with the
    /// columns it copies.
    #[allow(clippy::too_many_arguments)]
    fn known(
        &mut self,
        new: &Leaves,
        old: &Leaves,
        k: usize,
        columns: (usize, usize),
        strips: &Strips,
        bounds: &BlitBounds,
        across: Across,
    ) -> Option<(Offset, (usize, usize))> {
        let tries = &mut self.tries;
        first_carrying(
            &self.recent,
            new,
            old,
            k,
            columns,
            strips,
            bounds,
            across,
            tries,
        )
        .or_else(|| {
            first_carrying(
                &[STILL],
                new,
                old,
                k,
                columns,
                strips,
                bounds,
                across,
                tries,
            )
        })
        .or_else(|| {
            let found = first_carrying(
                &self.anchored,
                new,
                old,
                k,
                columns,
                strips,
                bounds,
                across,
                tries,
            );
            self.anchors_paid |= found.is_some();
            found
        })
    }

    /// The first candidate that carries the columns `columns` of strip `k` (`carried`), with
    /// the columns it copies: a known one (`known`), else one the anchors from those columns
    /// on suggest, looked for once they lie past the ones looked at.
    #[allow(clippy::too_many_arguments)]
    fn carrying(
        &mut self,
        new: &Leaves,
        old: &Leaves,
        k: usize,
        columns: (usize, usize),
        strips: &Strips,
        bounds: &BlitBounds,
        across: Across,
    ) -> Option<(Offset, (usize, usize))> {
        self.known(new, old, k, columns, strips, bounds, across)
            .or_else(|| self.anchoring(new, old, k, columns, strips, bounds, across))
    }

    /// The first offset the anchors from the columns `columns` on suggest (`blit_offsets`) that
    /// carries them, the anchors looked for only when those columns lie past the ones looked
    /// at, and only offsets not known before tried.
    #[allow(clippy::too_many_arguments)]
    fn anchoring(
        &mut self,
        new: &Leaves,
        old: &Leaves,
        k: usize,
        columns: (usize, usize),
        strips: &Strips,
        bounds: &BlitBounds,
        across: Across,
    ) -> Option<(Offset, (usize, usize))> {
        if self
            .anchored_to
            .is_some_and(|reach| columns.0 < reach || !self.anchors_paid)
        {
            return None;
        }
        let from = self.anchored.len();
        let (offsets, reach) = blit_offsets(
            new,
            old,
            k,
            &strips.rows,
            across,
            (columns.0, self.window.1),
        );
        self.anchored_to = Some(reach);
        for offset in offsets {
            if !self.recent.contains(&offset) && !self.anchored.contains(&offset) {
                self.anchored.push(offset);
            }
        }
        let found = first_carrying(
            &self.anchored[from..],
            new,
            old,
            k,
            columns,
            strips,
            bounds,
            across,
            &mut self.tries,
        );
        self.anchors_paid = found.is_some();
        found
    }
}

/// How many offsets one strip's pieces are tried at in vain (`piece_runs`): what a piece no
/// offset carries cost — a strip where nothing moved (another buffer shown) stops looking after
/// these, while the tries that find a run (one for a whole-pane scroll, a few to stretch one)
/// do not count.
const STRIP_MISSES: usize = 24;

/// Strip `k`'s `pieces` (left to right) cut into runs that are each one copy. From the leftmost
/// piece not yet taken: all the rest when an offset known so far carries them (a whole-pane
/// scroll, one check: `Candidates::known`), else the first candidate that carries that piece
/// (known, then anchored: `Candidates::anchoring`), stretched over as many of the pieces after
/// it as it carries too — to the end, to the last piece touching it, then as far as halving
/// finds; a piece no candidate carries is left to draw, and so is every piece once the strip
/// has tried `STRIP_MISSES` offsets in vain. Each run's offset and the columns it copies. A pane
/// scrolled down and sideways at once is two runs — its gutter moved down, its text down and
/// across — and two panes of a split scrolled their own ways are a run each.
#[allow(clippy::too_many_arguments)]
fn piece_runs(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    pieces: &[(usize, usize)],
    candidates: &mut Candidates,
    strips: &Strips,
    bounds: &BlitBounds,
    across: Across,
) -> Vec<(Offset, (usize, usize))> {
    let mut runs = Vec::new();
    let mut first = 0;
    // whether the piece before `first` ended a run: only then is the rest tried whole
    let mut after_run = true;
    // the tries spent on pieces no offset carried
    let mut misses = 0;
    while first < pieces.len() && misses < STRIP_MISSES {
        let tries_before = candidates.tries;
        // the rest of the strip in one copy by an offset already known (a whole-pane scroll),
        // else its first piece by a known offset, else by one its anchors suggest
        let end = pieces.len() - 1;
        let rest = (pieces[first].0, pieces[end].1);
        let piece = pieces[first];
        let whole = if after_run {
            candidates.known(new, old, k, rest, strips, bounds, across)
        } else {
            None
        };
        // `last` is carried, `beyond` is not (or is past the pieces): the rest whole was just
        // tried at every known offset
        let found = match whole {
            Some((offset, columns)) => Some((offset, columns, end, pieces.len())),
            None => {
                let tried = if after_run { end } else { pieces.len() };
                match candidates.known(new, old, k, piece, strips, bounds, across) {
                    Some((offset, columns)) => Some((offset, columns, first, tried)),
                    None => candidates
                        .anchoring(new, old, k, piece, strips, bounds, across)
                        .map(|(offset, columns)| (offset, columns, first, pieces.len())),
                }
            }
        };
        let Some((offset, columns, last, beyond)) = found else {
            misses += candidates.tries - tries_before;
            first += 1;
            after_run = false;
            continue;
        };
        let stretched = |last: usize| {
            carried(
                new,
                old,
                k,
                offset,
                (pieces[first].0, pieces[last].1),
                strips,
                bounds,
                across,
            )
        };
        // stretched to the end, then to the last of the pieces touching this one (a pane's
        // gutter and text), then as far as halving finds
        let touching = (first..end)
            .find(|&index| pieces[index].1 != pieces[index + 1].0)
            .unwrap_or(end);
        let mut planned = vec![touching, end];
        let (mut last, mut copied, mut beyond) = (last, columns, beyond);
        while last + 1 < beyond {
            let probe = match planned.pop() {
                Some(probe) if probe > last && probe < beyond => probe,
                Some(_) => continue,
                None => (last + 1 + beyond) / 2,
            };
            candidates.tries += 1;
            match stretched(probe) {
                Some(columns) => (last, copied) = (probe, columns),
                None => beyond = probe,
            }
        }
        runs.push((offset, copied));
        first = last + 1;
        after_run = true;
    }
    runs
}

/// The parts of `columns` (spans left to right) outside every one of `copied`, on the pixel rows
/// `rows`: what a strip still draws once its copies are taken, left to right, the ones near
/// enough merged into one (`worth_merging`), so that the two ends of a row are two narrow
/// rects; none when the copies cover all of it.
fn left_to_draw(
    columns: &[(usize, usize)],
    copied: &[(usize, usize)],
    (y0, y1): (usize, usize),
) -> Vec<(usize, usize)> {
    let mut left: Vec<(usize, usize)> = columns.to_vec();
    for &(from, to) in copied {
        left = left
            .into_iter()
            .flat_map(|(x0, x1)| [(x0, x1.min(from)), (x0.max(to), x1)])
            .filter(|(x0, x1)| x1 > x0)
            .collect();
    }
    let mut groups: Vec<DamageRect> = Vec::new();
    for (x0, x1) in left {
        let part = DamageRect { x0, y0, x1, y1 };
        match groups.last_mut() {
            Some(last) if worth_merging(last, &part) => last.x1 = last.x1.max(x1),
            _ => groups.push(part),
        }
    }
    groups.iter().map(|group| (group.x0, group.x1)).collect()
}

/// How many of the offsets that carried strips `strip_blits` tries first.
const RECENT_OFFSETS: usize = 4;

/// Copy, instead of drawing, every dirty full-height cell-row strip of `damage` (on `strips`,
/// within `bounds`) whose changed pixels are a translation of pixels the canvas already
/// holds: answer the moves, and take what they cover out of `damage`. With narrowing on
/// (`narrow`), a strip whose changed columns are several pieces (`strip_pieces`) is cut into
/// runs, each its own offset (`piece_runs`): one run when a whole pane scrolled, one a pane
/// when two scrolled their own ways, the gutter apart from the text when a pane moved down and
/// sideways at once, none where nothing moved; any other strip is tried over its changed
/// columns (`damage.columns`) as one window. What no move copies — a piece that is no
/// translation, the columns a sideways scroll brought in — is left to draw, the parts far
/// apart as rects apart (`left_to_draw`, `GridDamage::apart`). Copying only the changed
/// columns is what lets one pane of a side-by-side split scroll by copying: the other pane's
/// leaves stay out of the comparison, and its pixels out of the copy.
#[allow(clippy::too_many_arguments)]
pub(super) fn strip_blits(
    new: &Leaves,
    old: &Leaves,
    damage: &mut GridDamage,
    strips: &Strips,
    fb_w: usize,
    bounds: &BlitBounds,
    across: Across,
    narrow: bool,
) -> Vec<StripMove> {
    let ch = strips.rows.ch;
    let mut moves = Vec::new();
    let mut recent: Vec<Offset> = Vec::new();
    for k in 1..strips.count {
        if !damage.dirty[k] {
            continue;
        }
        let (y0, y1) = strips.rows(k);
        if y1 - y0 != ch || y0 < bounds.rows.0 || y1 > bounds.rows.1 {
            // a partial strip at the bottom, the row above a seam with its gap, a row
            // reaching out of the bounds: its rows are its own
            continue;
        }
        let window = damage.columns[k];
        let mut candidates = Candidates::new(&recent, window);
        // the copies taken, and the columns they were taken over (what is left is drawn): a
        // strip of pieces side by side in runs, the first stretched over all of them when one
        // offset carries the lot (a whole-pane scroll); any other strip over its window
        let pieces = if narrow {
            strip_pieces(new, old, k, fb_w, across).filter(|pieces| pieces.len() > 1)
        } else {
            None
        };
        let (runs, changed) = match pieces {
            Some(pieces) => (
                piece_runs(
                    new,
                    old,
                    k,
                    &pieces,
                    &mut candidates,
                    strips,
                    bounds,
                    across,
                ),
                pieces,
            ),
            None => match candidates.carrying(new, old, k, window, strips, bounds, across) {
                Some(run) => (vec![run], vec![window]),
                None => continue,
            },
        };
        if runs.is_empty() {
            continue;
        }
        for &(offset, columns) in runs.iter().filter(|(offset, _)| *offset != STILL) {
            moves.push(StripMove {
                destination: y0,
                source: (y0 as isize - offset.dy) as usize,
                height: ch,
                columns,
                source_left: (columns.0 as isize - offset.dx) as usize,
            });
            recent.retain(|&other| other != offset);
            recent.insert(0, offset);
            recent.truncate(RECENT_OFFSETS);
        }
        let copied: Vec<(usize, usize)> = runs.iter().map(|&(_, columns)| columns).collect();
        match left_to_draw(&changed, &copied, (y0, y1)).split_first() {
            Some((&first, apart)) => {
                damage.columns[k] = first;
                damage.apart[k] = apart.to_vec();
            }
            None => damage.dirty[k] = false,
        }
    }
    moves
}

/// One grid's plan: its strips diffed (`grid_damage`, narrowed to their changed columns when
/// `narrow`), the dirty ones that are a translation copied (`strip_blits`, when `blit`), and
/// the rest drawn. THE plan of a grid: the window's (`paint::diff_plan`) and a zoomed pane's
/// own (`region_plan`).
#[allow(clippy::too_many_arguments)]
pub(super) fn grid_plan(
    new: &Leaves,
    old: &Leaves,
    strips: &Strips,
    fb_w: usize,
    across: Across,
    bounds: &BlitBounds,
    (narrow, blit): (bool, bool),
) -> Plan {
    let mut damage = grid_damage(new, old, strips, fb_w, across, narrow);
    let changed = dirty_rects(&damage.dirty, &damage.columns, strips);
    let moves = if blit {
        strip_blits(new, old, &mut damage, strips, fb_w, bounds, across, narrow)
    } else {
        Vec::new()
    };
    let raster = damage.rects(strips);
    Plan {
        changed,
        raster,
        moves,
    }
}

/// Merge runs of dirty strips into damage rects — one raster pass each. Strip `k` spans
/// the pixel columns `columns[k]`; a run of adjacent dirty strips becomes one rect over
/// the union of their columns (re-rasterising more of a row than changed is exact too,
/// and one pass over the frame per run is what the row diff always paid) — unless the union
/// would draw much more than the two apart (`worth_merging`).
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
        let strip = DamageRect { x0, y0, x1, y1 };
        match rects.last_mut() {
            Some(last) if last.y1 == y0 && worth_merging(last, &strip) => {
                last.y1 = y1;
                last.x0 = last.x0.min(x0);
                last.x1 = last.x1.max(x1);
            }
            _ => rects.push(strip),
        }
    }
    rects
}

/// How much more than its two parts a merged rect may draw, as `(numerator, denominator)`: a
/// quarter more. Past that the two are drawn apart — a resized window's new right edge beside
/// its new bottom rows is two narrow rects, not the window.
const MERGE_GROWTH_MAX: (usize, usize) = (5, 4);

/// Whether `above` and the strip right under it are drawn as one rect: their union draws at
/// most [`MERGE_GROWTH_MAX`] times the pixels the two do apart.
fn worth_merging(above: &DamageRect, below: &DamageRect) -> bool {
    let union = DamageRect {
        x0: above.x0.min(below.x0),
        x1: above.x1.max(below.x1),
        y0: above.y0,
        y1: below.y1,
    };
    let (numerator, denominator) = MERGE_GROWTH_MAX;
    union.area() * denominator <= (above.area() + below.area()) * numerator
}

#[cfg(test)]
mod strip_diff_tests {
    use super::*;
    use crate::host::gui::{CursorStyle, Face, Op};

    /// Every strip's `strip_differs`, as a bool per strip.
    fn strip_diff(new: &Leaves, old: &Leaves, strips: &Strips) -> Vec<bool> {
        (0..strips.count)
            .map(|k| strip_differs(new, old, k))
            .collect()
    }

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

    /// A grid's damage marks only the strips whose leaves changed, each over the columns its
    /// change reaches when narrowed and over the whole row when not; a clean strip keeps the
    /// whole row, so a copy over it would take everything.
    #[test]
    fn grid_damage_marks_the_changed_strips_over_their_columns() {
        let strips = Strips::new(RowMap::uniform(0, 10), 40);
        let across = Across {
            ox: 0,
            cw: 8,
            ligatures: false,
        };
        let frame = |second: &str| {
            vec![
                text_at(0, 2, "same", [200, 200, 200]),
                text_at(1, 2, second, [200, 200, 200]),
            ]
        };
        let (new_ops, old_ops) = (frame("abcd"), frame("abc"));
        let (new, old) = (
            Leaves::of(&new_ops, &strips, false),
            Leaves::of(&old_ops, &strips, false),
        );
        let narrow = grid_damage(&new, &old, &strips, 200, across, true);
        // strip 0 is the top margin, row 0 strip 1, row 1 strip 2
        assert_eq!(narrow.dirty, vec![false, false, true, false, false]);
        assert_eq!(narrow.columns[2], (16, 48));
        assert_eq!(narrow.columns[1], (0, 200));
        let whole = grid_damage(&new, &old, &strips, 200, across, false);
        assert_eq!(whole.dirty, narrow.dirty);
        assert_eq!(whole.columns[2], (0, 200));
        assert_eq!(strip_damage(&new, &old, 1, 200, across, true), None);
    }
    #[test]
    fn a_strip_whose_ops_have_no_known_columns_is_repainted_whole() {
        let strips = Strips::new(RowMap::uniform(0, 10), 40);
        let columns = |new: &[Op], old: &[Op]| {
            let (a, b) = (
                Leaves::of(new, &strips, false),
                Leaves::of(old, &strips, false),
            );
            let across = Across {
                ox: 0,
                cw: 8,
                ligatures: false,
            };
            strip_columns(&a, &b, 1, 200, across)
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
