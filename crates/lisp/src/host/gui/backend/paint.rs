//! Painting a frame: turn a window's render ops into pixels — incrementally. The
//! renderer retains the last frame (its ops and the canvas they rasterised to); a new
//! frame is diffed against it per cell row (`strip_diff`), only the rows whose op
//! sequence changed are re-rasterised, and only those rows are copied to the window.
//! A keystroke re-rasterises the line it touched and the mode line, not the screen.
//! Runs on the GUI thread, where a panic takes the event loop with it — so
//! `render_ops` is fuzzed against wild inputs here.

use super::*;

// Paint-breakdown diagnostics (BROOD_STALL_MS): single GUI thread, so Relaxed is
// fine. Reset at each `paint` entry; when the paint runs >= the threshold, the
// breakdown line tells us whether the time is glyph re-rasterization (cache
// misses — e.g. an animated `:scale`) or just blitting a lot of pixels.
pub(super) static PAINT_CLUSTERS: AtomicU64 = AtomicU64::new(0);

pub(super) static PAINT_MISSES: AtomicU64 = AtomicU64::new(0);

pub(super) static PAINT_BUILD_NS: AtomicU64 = AtomicU64::new(0);

pub(super) fn paint_stall_ms() -> Option<u128> {
    static MS: OnceLock<Option<u128>> = OnceLock::new();
    *MS.get_or_init(|| {
        // `BROOD_GUI_TRACE=1` traces EVERY paint (threshold 0) without the runtime's
        // per-native-call stall tracing that a `BROOD_STALL_MS=0` would flood stderr with.
        if std::env::var("BROOD_GUI_TRACE")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            return Some(0);
        }
        std::env::var("BROOD_STALL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

/// A half-open damage rectangle in physical pixels: `[x0,x1) × [y0,y1)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct DamageRect {
    pub(super) x0: usize,
    pub(super) y0: usize,
    pub(super) x1: usize,
    pub(super) y1: usize,
}

impl DamageRect {
    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// Incremental (row-diffed) painting is **on by default**; `BROOD_GUI_DAMAGE=0` opts
/// back to re-rasterising and presenting the whole buffer every frame (the safe
/// fallback) — a one-line escape hatch if a diff ever misses a change. Read once.
pub(super) fn gui_damage_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BROOD_GUI_DAMAGE")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

/// The scroll blit (`strip_blits`) is on by default; `BROOD_GUI_BLIT=0` rasterises
/// every dirty strip instead — the escape hatch if a translated strip ever differs
/// from a drawn one. Read once.
pub(super) fn gui_blit_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BROOD_GUI_BLIT")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

/// A region's `dy-frac` (in cell units) as a pixel offset that the coordinate math
/// can safely subtract. A non-finite `dy-frac` casts to 0 — no scroll — which is the
/// only defensible reading of "shift by NaN".
pub(super) fn scroll_px(dy_frac: f32, ch: usize) -> isize {
    ((dy_frac * ch as f32).round() as isize).clamp(-MAX_SCROLL_PX, MAX_SCROLL_PX)
}

/// `BROOD_GUI_DUMP=<path.ppm>`: write the retained canvas after every paint as a binary
/// PPM — the way to LOOK at what the rasteriser produced (text AA, hairlines) from a
/// script, without a screenshot of the compositor's output. Read once; off by default.
fn dump_canvas(canvas: &[u32], w: usize, h: usize) {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    let Some(path) = PATH.get_or_init(|| std::env::var("BROOD_GUI_DUMP").ok()) else {
        return;
    };
    let mut out = Vec::with_capacity(w * h * 3 + 32);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    for px in canvas.iter().take(w * h) {
        out.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8]);
    }
    let _ = std::fs::write(path, out);
}

// ---- the strip diff ----------------------------------------------------------

/// The signed pixel-row band `[top, bottom)` an op paints, before clipping, given the
/// grid origin `oy`, cell height `ch` and the scroll shift `dy` in effect where it
/// sits; `None` for an op that paints nothing (a cursor zone). Conservative by
/// construction — a band may be wider than the pixels actually touched, never
/// narrower — since it decides which rows a change re-rasterises. A `Clear` is the
/// whole framebuffer; a `ScrollRegion` is not an entry (its children are, each with
/// its shift), so it never gets here.
fn op_band(op: &Op, dy: isize, oy: usize, ch: usize) -> Option<(isize, isize)> {
    let ch_i = ch as isize;
    let cell_top = |row: u16| oy as isize + row as isize * ch_i - dy;
    // A height in cells as pixels, saturating: the op vocabulary allows u16::MAX rows.
    let px_h = |cells: usize| (cells as isize).saturating_mul(ch_i);
    let band = |top: isize, h: isize| Some((top, top.saturating_add(h)));
    match op {
        Op::Clear => Some((isize::MIN, isize::MAX)),
        Op::CursorZone { .. } => None,
        Op::Text { row, face, .. } => band(cell_top(*row), px_h(face.scale.max(1) as usize)),
        Op::Cursor { row, .. } => band(cell_top(*row), ch_i),
        Op::Rect { row, h, .. } => band(cell_top(*row), px_h(*h as usize)),
        Op::FRect { y, h, .. } => {
            // shifted like a cell (`cell_top`): an frect inside a scroll region moves
            let top = oy as f32 + y * ch as f32 - dy as f32;
            let bottom = top + h * ch as f32;
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
            band(cell_top(*row0) + dy, px_h(tallest)) // VSpans ignore the scroll shift
        }
        Op::Cells { row0, w, bytes, .. } | Op::CellsRgb { row0, w, bytes, .. } => {
            let bits = bytes.len().saturating_mul(8);
            let rows = bits.div_ceil((*w).max(1) as usize);
            band(cell_top(*row0) + dy, px_h(rows)) // as do the bitboards
        }
        Op::ScrollRegion { .. } => Some((isize::MIN, isize::MAX)),
        // A region paints only inside its own rect, whose height is in PARENT cells — so
        // this band is exact even though the ops inside it are on a different grid.
        Op::CellRegion { y, h, .. } => band(cell_top(*y), px_h(*h as usize)),
    }
}

/// One flattened op: a leaf of the frame's op tree with the scroll shift in effect
/// where it sits. Two frames are compared as sequences of these.
struct Entry<'a> {
    op: &'a Op,
    dy: isize,
}

/// Flatten a frame's op tree into leaves, each carrying its region's shift.
fn flatten<'a>(ops: &'a [Op], ch: usize, dy: isize, out: &mut Vec<Entry<'a>>) {
    for op in ops {
        match op {
            Op::ScrollRegion { dy_frac, ops } => flatten(ops, ch, scroll_px(*dy_frac, ch), out),
            // A cell region is ONE leaf: its children are on the region's grid, not this one,
            // so their bands cannot be computed with `ch` and their positions mean nothing
            // here. The whole region is compared (and repainted) as a unit.
            _ => out.push(Entry { op, dy }),
        }
    }
}

/// The cell-row strips of a `fb_h`-tall framebuffer whose grid starts at `oy`:
/// strip 0 is the top margin `[0, oy)`, strip `k ≥ 1` the cell row `k-1`, the last
/// one whatever remainder is left below the grid. The unit of the frame diff: a strip
/// is repainted whole or not at all, so the diff never has to reason about pixels.
struct Strips {
    oy: usize,
    ch: usize,
    fb_h: usize,
    count: usize,
}

impl Strips {
    fn new(oy: usize, ch: usize, fb_h: usize) -> Strips {
        let ch = ch.max(1);
        let oy = oy.min(fb_h);
        Strips {
            oy,
            ch,
            fb_h,
            count: 1 + (fb_h - oy).div_ceil(ch),
        }
    }

    /// Strip `k`'s pixel rows, clipped to the framebuffer.
    fn rows(&self, k: usize) -> (usize, usize) {
        if k == 0 {
            (0, self.oy)
        } else {
            let top = self.oy + (k - 1) * self.ch;
            (top.min(self.fb_h), (top + self.ch).min(self.fb_h))
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
            let y = y as usize;
            if y < self.oy {
                0
            } else {
                (1 + (y - self.oy) / self.ch).min(self.count - 1)
            }
        };
        strip_of(top)..strip_of(bottom - 1) + 1
    }
}

/// A frame flattened for the diff: its leaves, each leaf's signed pixel band (`None`
/// for one that paints nothing), and per strip the leaves whose band covers it.
struct Leaves<'a> {
    leaves: Vec<Entry<'a>>,
    bands: Vec<Option<(isize, isize)>>,
    per_strip: Vec<Vec<u32>>,
}

impl<'a> Leaves<'a> {
    fn of(ops: &'a [Op], strips: &Strips, oy: usize, ch: usize) -> Leaves<'a> {
        let mut leaves = Vec::new();
        flatten(ops, ch, 0, &mut leaves);
        let bands: Vec<Option<(isize, isize)>> =
            leaves.iter().map(|e| op_band(e.op, e.dy, oy, ch)).collect();
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
/// identical frame dirties nothing and a one-line edit dirties one strip. Returns a
/// bool per strip.
fn strip_diff(new: &Leaves, old: &Leaves, strips: &Strips) -> Vec<bool> {
    (0..strips.count)
        .map(|k| {
            let (a, b) = (&new.per_strip[k], &old.per_strip[k]);
            a.len() != b.len()
                || a.iter().zip(b).any(|(&i, &j)| {
                    let (x, y) = (&new.leaves[i as usize], &old.leaves[j as usize]);
                    x.dy != y.dy || x.op != y.op
                })
        })
        .collect()
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
fn leaf_top(op: &Op, dy: isize, oy: usize, ch: usize) -> Option<isize> {
    match op {
        Op::Text { row, .. } | Op::Cursor { row, .. } | Op::Rect { row, .. } => {
            Some(oy as isize + *row as isize * ch as isize - dy)
        }
        _ => None,
    }
}

/// Whether painting `new` into the rows `[y0, y1)` produces the pixels `old` painted
/// into `[y0 - delta, y1 - delta)`: the same op translated by `delta` pixels, or a solid
/// fill that covers both bands with its straight part (rounded corners kept `radius`
/// clear of the bands, and a hairline's snap slop on top).
fn leaf_translates(
    new: &Entry,
    old: &Entry,
    delta: isize,
    (y0, y1): (isize, isize),
    oy: usize,
    cw: usize,
    ch: usize,
) -> bool {
    let (src0, src1) = (y0 - delta, y1 - delta);
    let ch_i = ch as isize;
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
                && oy as isize + *nr as isize * ch_i - new.dy
                    == oy as isize + *or as isize * ch_i - old.dy + delta
        }
        (
            Op::Cursor {
                row: nr,
                col: nc,
                style: nst,
            },
            Op::Cursor {
                row: or,
                col: oc,
                style: ost,
            },
        ) => {
            nc == oc
                && nst == ost
                && oy as isize + *nr as isize * ch_i - new.dy
                    == oy as isize + *or as isize * ch_i - old.dy + delta
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
            let ntop = oy as isize + *nr as isize * ch_i - new.dy;
            let otop = oy as isize + *or as isize * ch_i - old.dy;
            if nh == oh && ntop == otop + delta {
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
                (ntop + *nh as isize * ch_i) as f32,
                margin,
                y0,
                y1,
            ) && covers(
                otop as f32,
                (otop + *oh as isize * ch_i) as f32,
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
            let ntop = oy as f32 + ny * ch as f32 - new.dy as f32;
            let otop = oy as f32 + oy_ * ch as f32 - old.dy as f32;
            if !(ntop.is_finite() && otop.is_finite()) {
                return false;
            }
            if nh == oh && (ntop - otop - delta as f32).abs() < 1e-3 {
                return true;
            }
            // the same slop `op_band` allows a hairline's snap, plus the corners
            let margin = nrad * cw as f32 + 5.0;
            covers(ntop, ntop + nh * ch as f32, margin, y0, y1)
                && covers(otop, otop + oh * ch as f32, margin, src0, src1)
        }
        // bitboards and column spans ignore the scroll shift and are never translated
        _ => false,
    }
}

/// The pixel translations that could carry an old leaf onto the first translatable
/// leaf of the new strip `k`: for each old leaf equal to it up to its row, the
/// difference of their tops. The candidates a strip's blit is tried at.
fn blit_deltas(new: &Leaves, old: &Leaves, k: usize, oy: usize, ch: usize) -> Vec<isize> {
    let mut deltas = Vec::new();
    for &i in &new.per_strip[k] {
        let leaf = &new.leaves[i as usize];
        let Some(top) = leaf_top(leaf.op, leaf.dy, oy, ch) else {
            continue;
        };
        // a whole-pane rect anchors nothing (it matches anywhere by covering)
        if matches!(leaf.op, Op::Rect { h, .. } if *h > 1) {
            continue;
        }
        for old_leaf in &old.leaves {
            let Some(old_top) = leaf_top(old_leaf.op, old_leaf.dy, oy, ch) else {
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
                        col: a, style: sa, ..
                    },
                    Op::Cursor {
                        col: b, style: sb, ..
                    },
                ) => a == b && sa == sb,
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
fn strip_translates(
    new: &Leaves,
    old: &Leaves,
    k: usize,
    delta: isize,
    (y0, y1): (usize, usize),
    fb_h: usize,
    oy: usize,
    cw: usize,
    ch: usize,
) -> bool {
    let (src0, src1) = (y0 as isize - delta, y1 as isize - delta);
    if src0 < 0 || src1 > fb_h as isize {
        return false;
    }
    let new_leaves = &new.per_strip[k];
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
            oy,
            cw,
            ch,
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
fn strip_blits(
    new: &Leaves,
    old: &Leaves,
    dirty: &[bool],
    strips: &Strips,
    fb_h: usize,
    oy: usize,
    cw: usize,
    ch: usize,
) -> Vec<(usize, usize)> {
    let mut blits = Vec::new();
    let mut last_delta: Option<isize> = None;
    for k in 1..strips.count {
        if !dirty[k] {
            continue;
        }
        let (y0, y1) = strips.rows(k);
        if y1 - y0 != ch {
            continue; // a partial strip at the bottom: its rows are its own
        }
        let mut candidates = Vec::new();
        if let Some(d) = last_delta {
            candidates.push(d);
        }
        for d in blit_deltas(new, old, k, oy, ch) {
            if !candidates.contains(&d) {
                candidates.push(d);
            }
        }
        for delta in candidates {
            if strip_translates(new, old, k, delta, (y0, y1), fb_h, oy, cw, ch) {
                blits.push((k, (y0 as isize - delta) as usize));
                last_delta = Some(delta);
                break;
            }
        }
    }
    blits
}

/// Merge runs of dirty strips into pixel bands `[y0, y1)` — one raster pass each.
fn dirty_bands(dirty: &[bool], strips: &Strips) -> Vec<(usize, usize)> {
    let mut bands: Vec<(usize, usize)> = Vec::new();
    for (k, &d) in dirty.iter().enumerate() {
        if !d {
            continue;
        }
        let (y0, y1) = strips.rows(k);
        if y1 <= y0 {
            continue;
        }
        match bands.last_mut() {
            Some(last) if last.1 == y0 => last.1 = y1,
            _ => bands.push((y0, y1)),
        }
    }
    bands
}

// ---- the frame's cursor hot-zones --------------------------------------------------

/// A cursor hot-zone from the last drawn frame, flattened to a PIXEL rect relative to the
/// grid origin — `Op::CursorZone`'s cell rect resolved through whatever regions enclose it.
///
/// Cells alone cannot say where a zone is: inside an `Op::CellRegion` a cell is not the
/// window's cell (that is the whole point of the op), and inside an `Op::ScrollRegion` a
/// row has slid by a fraction of one. Pixels carry the answer out to the hit-test, which
/// has the pointer's pixel position in hand anyway.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Zone {
    pub(super) x: isize,
    pub(super) y: isize,
    pub(super) w: isize,
    pub(super) h: isize,
    pub(super) shape: CursorShape,
}

impl Zone {
    /// Is the pointer — grid-relative physical px — inside this zone?
    pub(super) fn contains(&self, x: isize, y: isize) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Every `Op::CursorZone` in a frame as a pixel rect, walking INTO the regions with the
/// same coordinate math `render_ops` paints them with — so a zone lands exactly on what
/// the eye sees under it. Collected once per frame (at `Draw`), never per pointer move.
pub(super) fn cursor_zones(ops: &[Op], r: &Renderer) -> Vec<Zone> {
    let mut zones = Vec::new();
    collect_zones(
        ops,
        r,
        0,
        0,
        r.cell_w.max(1),
        r.cell_h.max(1),
        0,
        (isize::MIN, isize::MAX),
        &mut zones,
    );
    zones
}

/// `cursor_zones`' recursion: `ox`/`oy` are the enclosing region's origin in grid pixels,
/// `cw`/`ch` the cell metrics in force there, `scroll_dy` the pixel shift applied to its
/// ops, and `clip` the row band the region confines them to — each the mirror of what
/// `render_ops` does with the same op.
#[allow(clippy::too_many_arguments)]
fn collect_zones(
    ops: &[Op],
    r: &Renderer,
    ox: isize,
    oy: isize,
    cw: usize,
    ch: usize,
    scroll_dy: isize,
    clip: (isize, isize),
    zones: &mut Vec<Zone>,
) {
    for op in ops {
        match op {
            Op::CursorZone { x, y, w, h, shape } => {
                let left = ox.saturating_add((*x as isize).saturating_mul(cw as isize));
                let top = oy
                    .saturating_add((*y as isize).saturating_mul(ch as isize))
                    .saturating_sub(scroll_dy);
                let bottom = top.saturating_add((*h as isize).saturating_mul(ch as isize));
                // Clipped to the enclosing region's band, like the pixels are: a zone the
                // region cuts off is not hoverable where it was never painted.
                let (top, bottom) = (top.max(clip.0), bottom.min(clip.1));
                if bottom > top {
                    zones.push(Zone {
                        x: left,
                        y: top,
                        w: (*w as isize).saturating_mul(cw as isize),
                        h: bottom - top,
                        shape: *shape,
                    });
                }
            }
            // The region's own offset replaces the parent's for the ops inside it.
            Op::ScrollRegion { dy_frac, ops } => {
                collect_zones(ops, r, ox, oy, cw, ch, scroll_px(*dy_frac, ch), clip, zones);
            }
            Op::CellRegion {
                x, y, h, px, ops, ..
            } => {
                let region = r.metrics_at(*px);
                if region.cell_w == 0 || region.cell_h == 0 {
                    continue;
                }
                let top = oy
                    .saturating_add((*y as isize).saturating_mul(ch as isize))
                    .saturating_sub(scroll_dy);
                let bottom = top.saturating_add((*h as isize).saturating_mul(ch as isize));
                let band = (top.max(clip.0), bottom.min(clip.1));
                if band.1 <= band.0 {
                    continue;
                }
                collect_zones(
                    ops,
                    r,
                    ox.saturating_add((*x as isize).saturating_mul(cw as isize)),
                    top,
                    region.cell_w,
                    region.cell_h,
                    0,
                    band,
                    zones,
                );
            }
            _ => {}
        }
    }
}

// ---- rasterising the ops ----------------------------------------------------------

/// Render `ops` into `canvas` (its band is the clip: rows outside it are untouched)
/// with the given `scroll_dy` pixel shift (positive = content shifted upward). Called
/// recursively for `ScrollRegion` — each region overrides the parent's `scroll_dy`
/// with its own, then automatically restores on return. An op whose band misses the
/// canvas band is skipped outright, so a strip repaint walks the whole frame but
/// rasterises only what lands in the strip.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_ops(
    ops: &[Op],
    canvas: &mut Canvas,
    r: &mut Renderer,
    ox: usize,
    oy: usize,
    cw: usize,
    ch: usize,
    bg0: u32,
    scroll_dy: isize,
) {
    let fb_h = canvas.h;
    for op in ops {
        if let Some((top, bottom)) = op_band(op, scroll_dy, oy, ch) {
            if bottom <= canvas.y0 as isize || top >= canvas.y1 as isize {
                continue;
            }
        }
        match op {
            Op::Clear => canvas.fill_rows(0, fb_h, bg0),
            Op::ScrollRegion { dy_frac, ops } => {
                let inner_dy = scroll_px(*dy_frac, ch);
                render_ops(ops, canvas, r, ox, oy, cw, ch, bg0, inner_dy);
            }
            Op::CellRegion {
                x, y, h, px, ops, ..
            } => {
                // The rect is in the PARENT's cells (and moves with an enclosing scroll, like
                // any other op); everything inside it is on the region's own grid, starting at
                // its top-left. The canvas band is narrowed to the rect, so an op that
                // overruns the region vertically is clipped rather than painted over its
                // neighbour — the region is a window, not a hint.
                // The rect's WIDTH is not clipped here (the canvas bands rows, not columns): it is
                // the width the region was laid out to, which its own ops already respect.
                let region = r.metrics_at(*px);
                if region.cell_w == 0 || region.cell_h == 0 {
                    continue;
                }
                let top = oy as isize + *y as isize * ch as isize - scroll_dy;
                let bottom = top.saturating_add((*h as isize).saturating_mul(ch as isize));
                let (y0, y1) = (
                    top.max(canvas.y0 as isize).max(0) as usize,
                    bottom.min(canvas.y1 as isize).max(0) as usize,
                );
                if y1 <= y0 {
                    continue;
                }
                let region_ox = ox + *x as usize * cw;
                let region_oy = top.max(0) as usize;
                let saved = r.metrics();
                r.set_metrics(region);
                let (canvas_w, canvas_h) = (canvas.w, canvas.h);
                let mut inner = Canvas::band(canvas.buf, canvas_w, canvas_h, y0, y1);
                render_ops(
                    ops,
                    &mut inner,
                    r,
                    region_ox,
                    region_oy,
                    region.cell_w,
                    region.cell_h,
                    bg0,
                    0,
                );
                r.set_metrics(saved);
            }
            Op::Text { row, col, s, face } => {
                let (mut fg, mut bg) =
                    (face.fg.unwrap_or(DEFAULT_FG), face.bg.unwrap_or(DEFAULT_BG));
                // Only paint a cell background when the face specifies one (or is
                // reversed). A face with no `:bg` is TRANSPARENT — the glyph composites
                // over whatever's already there (the frame clear, or an hl-line /
                // selection `rect` band drawn under the text), so the current-line
                // highlight shows behind the text too, exactly like Emacs. (Before,
                // every glyph filled its cell with DEFAULT_BG, painting over the band so
                // hl-line only showed in the line's trailing empty space.) For an
                // un-banded line the pixel beneath is already DEFAULT_BG, so this is
                // visually identical there — only banded lines change.
                let mut paint_bg = face.bg.is_some();
                if face.reverse {
                    std::mem::swap(&mut fg, &mut bg);
                    paint_bg = true;
                }
                // `:scale n` draws each glyph n× larger, occupying an n×n block
                // of base cells anchored at this op's (row, col); positions stay
                // in base-cell units, so a scaled cell advances `scale` columns.
                // We walk *grapheme clusters* (not codepoints), so a ZWJ emoji /
                // flag / accented char is one unit, advancing its `string/display-width`
                // cells — a wide glyph (emoji, CJK) takes two.
                let scale = face.scale.max(1) as usize;
                let ch_s = ch * scale;
                // Compute the effective top with scroll offset; clip to the grid origin.
                let top_signed = oy as isize + *row as isize * ch as isize - scroll_dy;
                let clip_skip = (oy as isize - top_signed).max(0) as usize;
                if clip_skip >= ch_s {
                    continue; // entirely above the grid origin
                }
                let visible_h = ch_s - clip_skip;
                let render_top = top_signed.max(oy as isize) as usize;
                let mut cx = *col as usize;
                let bg_packed = pack(bg);
                for g in s.graphemes(true) {
                    // A raw tab — the frontend has no line column to measure from, so
                    // it advances to the next SCREEN stop, as a terminal would (an app
                    // that wants the line's stops expands first: `string/expand-tabs`).
                    // Background only: a tab has no glyph.
                    if g == "\t" {
                        let cells = cluster_cells_at(g, cx, TAB_WIDTH); // base cells to the stop
                        if paint_bg {
                            let left = ox + cx * cw;
                            fill_cell(canvas, left, render_top, cells * cw, visible_h, bg_packed);
                        }
                        cx += cells;
                        continue;
                    }
                    let cells = cluster_cells(g);
                    if cells == 0 {
                        // zero-width (a lone combining mark): nothing to advance.
                        continue;
                    }
                    let block_w = cells * cw * scale; // the cluster's pixel span
                    let left = ox + cx * cw;
                    if paint_bg {
                        fill_cell(canvas, left, render_top, block_w, visible_h, bg_packed);
                    }
                    r.draw_cluster(
                        canvas,
                        left,
                        render_top,
                        g,
                        face.family,
                        face.bold,
                        face.italic,
                        face.scale,
                        fg,
                        clip_skip,
                    );
                    if face.underline {
                        // a rule near the block bottom, in the text colour
                        // (scaled with the glyph so it stays proportional).
                        let uy_signed = top_signed + ch_s as isize - 2 * scale as isize;
                        if uy_signed >= oy as isize {
                            fill_cell(canvas, left, uy_signed as usize, block_w, scale, pack(fg));
                        }
                    }
                    cx += cells * scale;
                }
            }
            Op::Rect {
                row,
                col,
                w,
                h,
                face,
                radius,
            } => {
                // A solid panel: fill the w×h cell block with the face background
                // (reverse swaps in the fg). No background → nothing to paint.
                let bg = if face.reverse { face.fg } else { face.bg };
                if let Some(bg) = bg {
                    let top_signed = oy as isize + *row as isize * ch as isize - scroll_dy;
                    let h_px = *h as isize * ch as isize;
                    let clip_skip = (oy as isize - top_signed).max(0);
                    let visible_h = (h_px - clip_skip).max(0) as usize;
                    if visible_h > 0 {
                        let render_top = top_signed.max(oy as isize) as usize;
                        if *radius > 0.0 {
                            // Rounded: the same AA filler `FRect` uses, over the
                            // cell-aligned box. Clipping is shared with the square
                            // path above, so a rounded panel scrolls identically.
                            fill_rrect(
                                canvas,
                                (ox + *col as usize * cw) as f32,
                                render_top as f32,
                                (*w as usize * cw) as f32,
                                visible_h as f32,
                                *radius * cw as f32,
                                bg,
                                1.0,
                                r.scale() as f32,
                            );
                        } else {
                            fill_cell(
                                canvas,
                                ox + *col as usize * cw,
                                render_top,
                                *w as usize * cw,
                                visible_h,
                                pack(bg),
                            );
                        }
                    }
                }
            }
            Op::FRect {
                x,
                y,
                w,
                h,
                face,
                opacity,
                radius,
            } => {
                // Sub-cell rounded rect: cell-unit floats → px via the same origin +
                // cell metrics every op shares, then an AA, alpha-blended fill.
                let bg = if face.reverse { face.fg } else { face.bg };
                if let Some(bg) = bg {
                    // Inside a scroll region the rect rides with the text: a gutter's
                    // change bar or a link's hover band is drawn as an `frect` in the
                    // pane body, and one that ignored the shift stayed put while the
                    // lines glided, snapping a row each time the top advanced.
                    fill_rrect(
                        canvas,
                        ox as f32 + *x * cw as f32,
                        oy as f32 + *y * ch as f32 - scroll_dy as f32,
                        *w * cw as f32,
                        *h * ch as f32,
                        *radius * cw as f32,
                        bg,
                        *opacity,
                        r.scale() as f32,
                    );
                }
            }
            Op::Cursor { row, col, style } => {
                // Compute the logical cell bounds in framebuffer pixels. `cell_top`
                // may be < oy (partially scrolled off the viewport top) — clip the
                // draw range to [oy, fb_h) rather than suppressing the cursor entirely,
                // so it tracks the text through the full smooth-scroll animation.
                let cell_top = oy as isize + *row as isize * ch as isize - scroll_dy;
                let cell_bottom = cell_top + ch as isize;
                let draw_top = cell_top.max(oy as isize);
                let draw_bottom = cell_bottom.min(fb_h as isize);
                if draw_top < draw_bottom {
                    cursor_cell(
                        canvas,
                        ox + *col as usize * cw,
                        draw_top as usize,
                        draw_bottom as usize,
                        cw,
                        ch,
                        cell_top,
                        *style,
                    );
                }
            }
            // Not painted — a cursor zone is hover metadata, hit-tested on
            // pointer-move in the window event handler (ADR-080).
            Op::CursorZone { .. } => {}
            Op::VSpans { row0, col0, cols } => {
                let top0 = oy + *row0 as usize * ch;
                for (i, segs) in cols.iter().enumerate() {
                    let left = ox + (*col0 as usize + i) * cw;
                    let mut y = top0;
                    for (h, color) in segs {
                        let span_h = *h as usize * ch;
                        if let Some(rgb) = color {
                            fill_cell(canvas, left, y, cw, span_h, pack(*rgb));
                        }
                        y += span_h;
                    }
                }
            }
            Op::Cells {
                row0,
                col0,
                w,
                aspect,
                bytes,
                color,
            } => {
                // Enumerate set bits by a single byte scan — O(bytes + live), and the
                // same code whether the board came in as a bignum or a byte string.
                // Each cell is an `aspect`-wide × 1-tall block of screen cells.
                if let Some(rgb) = color {
                    let packed = pack(*rgb);
                    let asp = (*aspect).max(1) as usize;
                    let cell_w = asp * cw; // a board cell spans `aspect` screen cells
                    let wmod = (*w).max(1) as usize;
                    for (bi, &byte) in bytes.iter().enumerate() {
                        let mut b = byte;
                        let base = bi * 8;
                        while b != 0 {
                            let bit = base + b.trailing_zeros() as usize;
                            let x = bit % wmod;
                            let y = bit / wmod;
                            let left = ox + (*col0 as usize + x * asp) * cw;
                            let top = oy + (*row0 as usize + y) * ch;
                            fill_cell(canvas, left, top, cell_w, ch, packed);
                            b &= b - 1;
                        }
                    }
                }
            }
            Op::CellsRgb {
                row0,
                col0,
                w,
                aspect,
                bytes,
                colors,
                default,
            } => {
                let asp = (*aspect).max(1) as usize;
                let cell_w = asp * cw;
                let wmod = (*w).max(1) as usize;
                for (bi, &byte) in bytes.iter().enumerate() {
                    let mut b = byte;
                    let base = bi * 8;
                    while b != 0 {
                        let bit = base + b.trailing_zeros() as usize;
                        let rgb = colors.get(&(bit as u64)).copied().unwrap_or(*default);
                        let x = bit % wmod;
                        let y = bit / wmod;
                        let left = ox + (*col0 as usize + x * asp) * cw;
                        let top = oy + (*row0 as usize + y) * ch;
                        fill_cell(canvas, left, top, cell_w, ch, pack(rgb));
                        b &= b - 1;
                    }
                }
            }
        }
    }
}

/// Rasterise `frame` into the renderer's retained canvas (sized `fb_w`×`fb_h`),
/// re-rendering only the strips the diff against the previous frame marks dirty —
/// or everything, when the canvas is fresh/resized, the diff is disabled, or
/// `force` (the caller wants a clean full render). A dirty strip that is a translation
/// of pixels the canvas already holds (a scroll, `strip_blits`) is copied, not drawn.
/// Records the frame as the new previous one and returns the pixel bands `[y0, y1)`
/// whose pixels changed — drawn or copied — for the present. `Renderer::blit_rows` counts
/// the copied rows for the trace.
pub(super) fn raster_frame(
    r: &mut Renderer,
    frame: &[Op],
    fb_w: usize,
    fb_h: usize,
    force: bool,
) -> Vec<(usize, usize)> {
    let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
    let (ox, oy) = r.grid_origin(fb_w, fb_h);
    let strips = Strips::new(oy, ch, fb_h);
    let fresh = r.canvas_size != (fb_w, fb_h);
    if fresh {
        r.canvas = vec![0u32; fb_w * fb_h];
        r.canvas_size = (fb_w, fb_h);
    }
    let same = frame == r.prev_ops.as_slice();
    // `changed`: every band whose pixels differ after this raster (what the present
    // ships); `raster`: the subset drawn; `blits`: the strips copied from old rows.
    let (changed, raster, blits) =
        if fresh || force || r.prev_ops.is_empty() || !gui_damage_enabled() {
            (vec![(0, fb_h)], vec![(0, fb_h)], Vec::new())
        } else if same {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let new = Leaves::of(frame, &strips, oy, ch);
            let old = Leaves::of(&r.prev_ops, &strips, oy, ch);
            let mut dirty = strip_diff(&new, &old, &strips);
            let changed = dirty_bands(&dirty, &strips);
            let blits = if gui_blit_enabled() {
                strip_blits(&new, &old, &dirty, &strips, fb_h, oy, cw, ch)
            } else {
                Vec::new()
            };
            for &(k, _) in &blits {
                dirty[k] = false;
            }
            (changed, dirty_bands(&dirty, &strips), blits)
        };
    let bg0 = pack(r.bg());
    // The canvas leaves the renderer for the duration of the raster (the primitives
    // borrow both), and comes back untouched in shape.
    let mut pixels = std::mem::take(&mut r.canvas);
    r.blit_rows = blits.len() * ch;
    // The blits first, all sources read before any destination is written: two
    // panes can scroll opposite ways, so a destination may be another's source.
    if !blits.is_empty() {
        let mut sources = Vec::with_capacity(blits.len() * ch * fb_w);
        for &(_, src_y0) in &blits {
            sources.extend_from_slice(&pixels[src_y0 * fb_w..(src_y0 + ch) * fb_w]);
        }
        for (n, &(k, _)) in blits.iter().enumerate() {
            let (y0, _) = strips.rows(k);
            pixels[y0 * fb_w..(y0 + ch) * fb_w]
                .copy_from_slice(&sources[n * ch * fb_w..(n + 1) * ch * fb_w]);
        }
    }
    for &(y0, y1) in &raster {
        let mut canvas = Canvas::band(&mut pixels, fb_w, fb_h, y0, y1);
        // The conventional frame opens with a full `:clear`; a frame that doesn't
        // still expects a clean background, and a strip repaint needs one either
        // way (the strip's old pixels are exactly what changed).
        canvas.fill_rows(y0, y1 - y0, bg0);
        render_ops(frame, &mut canvas, r, ox, oy, cw, ch, bg0, 0);
    }
    r.canvas = pixels;
    if !same {
        r.prev_ops = frame.to_vec();
    }
    changed
}

pub(super) fn paint(
    surface: &mut softbuffer::Surface<Rc<winit::window::Window>, Rc<winit::window::Window>>,
    window: &winit::window::Window,
    r: &mut Renderer,
    frame: &[Op],
) {
    // Paint-breakdown timing (BROOD_STALL_MS): reset the per-paint counters; the
    // tail of this fn prints clusters/misses/build-time when the paint is slow.
    let paint_t0 = paint_stall_ms().map(|ms| {
        PAINT_CLUSTERS.store(0, Ordering::Relaxed);
        PAINT_MISSES.store(0, Ordering::Relaxed);
        PAINT_BUILD_NS.store(0, Ordering::Relaxed);
        (ms, Instant::now())
    });
    let op_count = frame.len();
    let sz = window.inner_size();
    let (w, h) = (sz.width.max(1), sz.height.max(1));
    if surface
        .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
        .is_err()
    {
        return;
    }
    let (fb_w, fb_h) = (w as usize, h as usize);
    // Coordinate contract: `r.cell_w`/`cell_h` are PHYSICAL (post-scale) pixels;
    // `Op` row/col are BASE cells (top-left pixel = col*cell_w, row*cell_h); a
    // face `:scale n` multiplies into that same physical grid (n×n base cells).
    let t_setup = Instant::now();
    let resized = r.canvas_size != (fb_w, fb_h);
    let bands = raster_frame(r, frame, fb_w, fb_h, false);
    let t_body = Instant::now();
    // Present. The window buffer softbuffer hands us may hold content from
    // `buf.age()` frames ago, so the rows to copy from the canvas — and to declare as
    // damage — are the union of the last `age` frames' repainted bands. A resize
    // resets the history and copies everything; so does an unknown age (0) or one
    // older than the history we keep. Only the copied rows are presented, so a
    // one-line edit ships one line to the compositor.
    let mut buf = match surface.buffer_mut() {
        Ok(b) => b,
        Err(_) => {
            // The rows just rasterised never reached the window; forget the retained
            // frame so the next paint rasterises and presents everything.
            r.invalidate();
            return;
        }
    };
    let full = DamageRect {
        x0: 0,
        y0: 0,
        x1: fb_w,
        y1: fb_h,
    };
    let this: Vec<DamageRect> = bands
        .iter()
        .map(|&(y0, y1)| DamageRect {
            x0: 0,
            y0,
            x1: fb_w,
            y1,
        })
        .collect();
    if resized || !gui_damage_enabled() {
        r.damage_ring.clear();
    }
    r.damage_ring.push(this);
    if r.damage_ring.len() > DAMAGE_HISTORY {
        let drop = r.damage_ring.len() - DAMAGE_HISTORY;
        r.damage_ring.drain(0..drop);
    }
    let age = buf.age() as usize;
    let rects: Vec<DamageRect> =
        if resized || !gui_damage_enabled() || age == 0 || age > r.damage_ring.len() {
            vec![full]
        } else {
            let mut acc: Vec<DamageRect> = r
                .damage_ring
                .iter()
                .rev()
                .take(age)
                .flatten()
                .copied()
                .filter(|d| !d.is_empty())
                .collect();
            // Sort + merge overlapping bands so each row is copied once.
            acc.sort_by_key(|d| d.y0);
            let mut merged: Vec<DamageRect> = Vec::new();
            for d in acc {
                match merged.last_mut() {
                    Some(last) if d.y0 <= last.y1 => last.y1 = last.y1.max(d.y1),
                    _ => merged.push(d),
                }
            }
            merged
        };
    for d in &rects {
        let (a, b) = (d.y0 * fb_w, d.y1 * fb_w);
        buf[a..b].copy_from_slice(&r.canvas[a..b]);
    }
    dump_canvas(&r.canvas, fb_w, fb_h);
    let whole = rects.len() == 1 && rects[0] == full;
    if rects.is_empty() {
        // Nothing changed and the buffer is current — an expose with no new frame.
        // A zero-area damage is invalid, so re-present as is.
        let _ = buf.present();
    } else if whole {
        let _ = buf.present();
    } else {
        let sb: Vec<softbuffer::Rect> = rects
            .iter()
            .map(|d| softbuffer::Rect {
                x: d.x0 as u32,
                y: d.y0 as u32,
                width: NonZeroU32::new((d.x1 - d.x0) as u32).unwrap(),
                height: NonZeroU32::new((d.y1 - d.y0) as u32).unwrap(),
            })
            .collect();
        let _ = buf.present_with_damage(&sb);
    }
    // Slow-paint breakdown: when this paint took >= BROOD_STALL_MS, attribute the
    // time across phases. `present` dominating = the softbuffer→window blit (a
    // platform/software-render cost); `body` dominating with build~0 = the raster
    // of the repainted rows (see `rows=` for how many).
    if let Some((ms, t0)) = paint_t0 {
        let el = t0.elapsed();
        if el.as_millis() >= ms {
            let us = |d: std::time::Duration| d.as_micros();
            let clusters = PAINT_CLUSTERS.load(Ordering::Relaxed);
            let misses = PAINT_MISSES.load(Ordering::Relaxed);
            let build_us = PAINT_BUILD_NS.load(Ordering::Relaxed) / 1_000;
            let setup_us = us(t_setup.duration_since(t0));
            let body_us = us(t_body.duration_since(t_setup));
            let present_us = us(t_body.elapsed());
            let blit = r.blit_rows;
            // `rows` is what was drawn: the changed rows less the ones a blit copied
            let rows: usize = bands
                .iter()
                .map(|(a, b)| b - a)
                .sum::<usize>()
                .saturating_sub(blit);
            let shipped: usize = rects.iter().map(|d| d.y1 - d.y0).sum();
            let aa = if r.subpixel_text() {
                "subpixel"
            } else {
                "gray"
            };
            eprintln!(
                "[gui-paint] {}us: fb={fb_w}x{fb_h} ops={op_count} rows={rows}/{fb_h} blit={blit} \
                 shipped={shipped} clusters={clusters} misses={misses} build={build_us}us \
                 aa={aa} | setup={setup_us}us body={body_us}us present={present_us}us",
                us(el)
            );
        }
    }
}

#[cfg(test)]
mod render_robustness {
    use super::*;
    use crate::host::gui::{CursorStyle, Face, Op};

    /// Render `ops` into a small framebuffer. Any panic — an arithmetic overflow
    /// under debug-assertions, an out-of-bounds framebuffer write — fails the test.
    fn render(ops: &[Op]) {
        let mut r = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
        let (fb_w, fb_h) = (64usize, 48usize);
        let mut buf = vec![0u32; fb_w * fb_h];
        let mut canvas = Canvas::full(&mut buf, fb_w, fb_h);
        render_ops(ops, &mut canvas, &mut r, 2, 2, cw, ch, 0, 0);
    }

    fn face_bg() -> Face {
        Face {
            bg: Some([10, 20, 30]),
            ..Face::default()
        }
    }

    /// `[:scroll-region dy …]` takes an unvalidated float, and the renderer turns it
    /// into a pixel offset it SUBTRACTS from every inner op's top. A hugely negative
    /// `dy` saturates the cast to `isize::MIN`, and `oy + row*ch - isize::MIN`
    /// overflows — a debug-assertions panic on the GUI thread, wrapped garbage
    /// coordinates without. Non-finite values reach the same cast.
    #[test]
    fn a_wild_scroll_offset_does_not_overflow_the_coordinate_math() {
        for dy in [
            -1e30f32,
            1e30,
            f32::MIN,
            f32::MAX,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.5,
            3.25,
        ] {
            render(&[Op::ScrollRegion {
                dy_frac: dy,
                ops: vec![
                    Op::Rect {
                        row: 1,
                        col: 1,
                        w: 4,
                        h: 3,
                        face: face_bg(),
                        radius: 0.0,
                    },
                    Op::Rect {
                        row: 1,
                        col: 1,
                        w: 4,
                        h: 3,
                        face: face_bg(),
                        radius: 1.5,
                    },
                    Op::Cursor {
                        row: 2,
                        col: 2,
                        style: CursorStyle::Block,
                    },
                    Op::Text {
                        row: 1,
                        col: 0,
                        s: "hi".into(),
                        face: face_bg(),
                    },
                ],
            }]);
        }
    }

    /// Nested regions re-enter `render_ops`; each level's own `dy` must be as safe
    /// as the outer one, and the offsets must not accumulate into an overflow.
    #[test]
    fn nested_scroll_regions_stay_in_range() {
        let inner = Op::Rect {
            row: 0,
            col: 0,
            w: 8,
            h: 8,
            face: face_bg(),
            radius: 0.0,
        };
        let mut op = Op::ScrollRegion {
            dy_frac: -1e30,
            ops: vec![inner],
        };
        for _ in 0..16 {
            op = Op::ScrollRegion {
                dy_frac: 1e30,
                ops: vec![op],
            };
        }
        render(&[op]);
    }

    /// The largest extents the op parser can hand over (`clamp_u16` tops out at
    /// `u16::MAX - 1`) at the largest position, so `left + w` / `top + h` are as big
    /// as they can get. Both fills clip to the framebuffer, so this must be cheap
    /// and in-bounds rather than a 4-billion-cell loop or an OOB write.
    #[test]
    fn a_maximal_rect_clips_instead_of_running_away() {
        let big = u16::MAX - 1;
        render(&[
            Op::Rect {
                row: big,
                col: big,
                w: big,
                h: big,
                face: face_bg(),
                radius: 0.0,
            },
            Op::Rect {
                row: 0,
                col: 0,
                w: big,
                h: big,
                face: face_bg(),
                radius: 4.0,
            },
            Op::Cursor {
                row: big,
                col: big,
                style: CursorStyle::Bar,
            },
        ]);
    }

    /// `FRect` is the sub-cell op: its geometry is floats all the way down, so NaN
    /// and infinity reach the anti-aliased filler's loop bounds directly.
    #[test]
    fn a_non_finite_frect_paints_nothing_rather_than_hanging() {
        let wild = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -1e30,
            1e30,
            0.0,
            -3.5,
        ];
        for v in wild {
            render(&[Op::FRect {
                x: v,
                y: v,
                w: v,
                h: v,
                face: face_bg(),
                opacity: v,
                radius: v,
            }]);
            render(&[Op::FRect {
                x: 1.0,
                y: 1.0,
                w: 6.0,
                h: 4.0,
                face: face_bg(),
                opacity: v,
                radius: v,
            }]);
        }
    }

    /// The batch ops index a caller-supplied bit/byte buffer with a caller-supplied
    /// stride. A zero stride, a zero aspect and an all-ones buffer are the three
    /// degenerate inputs (`% 0` is a panic, not a wrong pixel).
    #[test]
    fn the_batch_ops_survive_degenerate_strides() {
        for w in [0u32, 1, 3, u32::MAX] {
            for aspect in [0u16, 1, u16::MAX] {
                render(&[
                    Op::Cells {
                        row0: 0,
                        col0: 0,
                        w,
                        aspect,
                        bytes: vec![0xff; 64],
                        color: Some([1, 2, 3]),
                    },
                    Op::CellsRgb {
                        row0: 0,
                        col0: 0,
                        w,
                        aspect,
                        bytes: vec![0xff; 64],
                        colors: std::collections::HashMap::new(),
                        default: [4, 5, 6],
                    },
                ]);
            }
        }
        // Vertical spans: many maximal segments, so the running `y` climbs as far
        // as the op vocabulary allows.
        let segs: Vec<(u16, Option<[u8; 3]>)> =
            (0..64).map(|_| (u16::MAX - 1, Some([7, 8, 9]))).collect();
        render(&[Op::VSpans {
            row0: u16::MAX - 1,
            col0: u16::MAX - 1,
            cols: vec![segs; 4],
        }]);
    }
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
        let strips = Strips::new(oy, ch, fb_h);
        strip_diff(
            &Leaves::of(new, &strips, oy, ch),
            &Leaves::of(old, &strips, oy, ch),
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
        let strips = Strips::new(0, 10, 30);
        let all: Vec<bool> = strip_diff(
            &Leaves::of(&new, &strips, 0, 10),
            &Leaves::of(&old, &strips, 0, 10),
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
                incremental.canvas == full.canvas,
                "frame {i}: incremental raster differs from a full one (repainted {bands:?})"
            );
        }
    }

    /// An `frect` inside a scroll region rides with the text: a gutter's change bar
    /// painted half a cell up lands where a text op at the same row does, both in the
    /// raster and in the rows the diff marks dirty. It used to stay put while the
    /// lines glided — the bar jumped a row each time the top advanced.
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
            assert!(incremental.canvas == full.canvas, "frame {i} differs");
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
            blitted.push(incremental.blit_rows);
            let mut full = Renderer::new(1.0, default_families(), 14.0);
            raster_frame(&mut full, frame, fb_w, fb_h, true);
            assert!(
                incremental.canvas == full.canvas,
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
        assert_eq!(bands, vec![(0, 96)]);
        let mut full = Renderer::new(1.0, default_families(), 14.0);
        raster_frame(&mut full, &frame, 80, 96, true);
        assert!(r.canvas == full.canvas);
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
}

#[cfg(test)]
mod cell_region_tests {
    use super::*;
    use crate::host::gui::{Face, Op};

    const FB: (usize, usize) = (160, 160);

    /// Paint `ops` into a fresh framebuffer with the window's own metrics at 15 px and
    /// the grid flush at the origin, and hand back the pixels.
    fn paint(ops: &[Op]) -> Vec<u32> {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
        let (fb_w, fb_h) = FB;
        let mut buf = vec![0u32; fb_w * fb_h];
        {
            let mut canvas = Canvas::full(&mut buf, fb_w, fb_h);
            render_ops(ops, &mut canvas, &mut r, 0, 0, cw, ch, 0, 0);
        }
        buf
    }

    /// The window's cell height at 15 px — the unit the test's rects are in.
    fn window_cell() -> (usize, usize) {
        let r = Renderer::new(1.0, default_families(), 15.0);
        (r.cell_w.max(1), r.cell_h.max(1))
    }

    fn white() -> Face {
        Face {
            fg: Some([255, 255, 255]),
            ..Face::default()
        }
    }

    fn text(row: u16, col: u16) -> Op {
        Op::Text {
            row,
            col,
            s: "M".into(),
            face: white(),
        }
    }

    /// The painted pixels' bounding box, as (max_x, max_y, count).
    fn extent(buf: &[u32]) -> (usize, usize, usize) {
        let (fb_w, _) = FB;
        buf.iter()
            .enumerate()
            .filter(|(_, p)| **p != 0)
            .fold((0, 0, 0), |(mx, my, n), (i, _)| {
                (mx.max(i % fb_w), my.max(i / fb_w), n + 1)
            })
    }

    fn region(px: f32, h: u16, ops: Vec<Op>) -> Op {
        Op::CellRegion {
            x: 0,
            y: 0,
            w: 40,
            h,
            px,
            ops,
        }
    }

    /// The point of the op: the same glyph inside a region at twice the size covers more
    /// pixels and reaches further down and right than the window's own cell would allow.
    #[test]
    fn a_region_paints_its_ops_at_its_own_size() {
        let (plain_x, plain_y, plain_n) = extent(&paint(&[text(0, 0)]));
        let (big_x, big_y, big_n) = extent(&paint(&[region(30.0, 6, vec![text(0, 0)])]));
        assert!(plain_n > 0, "the plain glyph painted nothing to compare to");
        assert!(
            big_n > plain_n && big_x > plain_x && big_y > plain_y,
            "a 30 px region painted no bigger than the 15 px window: \
             {big_n} px vs {plain_n}, to ({big_x}, {big_y}) vs ({plain_x}, {plain_y})"
        );
    }

    /// Scoped: the ops AFTER a region are painted with the window's metrics again, exactly
    /// as if the region were not there. A size that leaked would move every later op.
    #[test]
    fn the_window_metrics_come_back_after_a_region() {
        let (_, ch) = window_cell();
        let after = text(5, 0);
        let alone = paint(std::slice::from_ref(&after));
        let following = paint(&[region(30.0, 2, vec![text(0, 0)]), after]);
        // below the region's 2 cell rows, the two frames must agree pixel for pixel
        let from = 2 * ch * FB.0;
        assert_eq!(
            alone[from..],
            following[from..],
            "the region's metrics leaked into the op that followed it"
        );
    }

    /// A region is a window, not a hint: an op that overruns it vertically is clipped to
    /// the rect, so a zoomed buffer cannot paint over its neighbour.
    #[test]
    fn a_region_clips_its_ops_to_its_rect() {
        let (_, ch) = window_cell();
        // one window-cell tall, holding text far past its bottom at a big size
        let buf = paint(&[region(30.0, 1, vec![text(0, 0), text(3, 0), text(6, 0)])]);
        let spill = buf[ch * FB.0..].iter().filter(|p| **p != 0).count();
        assert_eq!(spill, 0, "{spill} pixels painted below a 1-row region");
    }

    /// The rect is in the PARENT's cells, so a region moves by whole window cells — and
    /// its ops move with it.
    #[test]
    fn a_region_is_placed_in_the_parent_grid() {
        let (cw, ch) = window_cell();
        let at = |x: u16, y: u16| {
            extent(&paint(&[Op::CellRegion {
                x,
                y,
                w: 20,
                h: 8,
                px: 15.0,
                ops: vec![text(0, 0)],
            }]))
        };
        let (x0, y0, n0) = at(0, 0);
        let (x1, y1, n1) = at(2, 3);
        assert!(n0 > 0 && n1 > 0, "a placed region painted nothing");
        assert_eq!(
            (x1 - x0, y1 - y0),
            (2 * cw, 3 * ch),
            "the region did not move by whole parent cells"
        );
    }

    // ---- the frame's cursor hot-zones ----

    /// The zones of a frame, as the `Draw` handler collects them.
    fn zones(ops: &[Op]) -> Vec<Zone> {
        let r = Renderer::new(1.0, default_families(), 15.0);
        cursor_zones(ops, &r)
    }

    fn zone(x: u16, y: u16, w: u16, h: u16) -> Op {
        Op::CursorZone {
            x,
            y,
            w,
            h,
            shape: CursorShape::Pointer,
        }
    }

    /// At the top level a zone is its cell rect in the window's own cells — the pixels
    /// the same rect would be painted at.
    #[test]
    fn a_top_level_zone_is_its_cell_rect_in_window_pixels() {
        let (cw, ch) = window_cell();
        let found = zones(&[zone(2, 3, 4, 1)]);
        assert_eq!(
            found,
            vec![Zone {
                x: 2 * cw as isize,
                y: 3 * ch as isize,
                w: 4 * cw as isize,
                h: ch as isize,
                shape: CursorShape::Pointer,
            }]
        );
    }

    /// The gap this closes: inside a `CellRegion` a zone used to be dropped entirely, so
    /// a link in a zoomed pane showed no hand. Now it is placed at the region's origin
    /// (parent cells) and sized in the REGION's cells — bigger at a bigger px.
    #[test]
    fn a_zone_inside_a_region_is_sized_in_that_regions_cells() {
        let (cw, ch) = window_cell();
        let region_at = |px: f32| {
            let found = zones(&[Op::CellRegion {
                x: 1,
                y: 2,
                w: 40,
                h: 20,
                px,
                ops: vec![zone(0, 1, 3, 1)],
            }]);
            assert_eq!(found.len(), 1, "a zone inside a region went missing");
            found[0]
        };
        let plain = region_at(15.0);
        assert_eq!(
            (plain.x, plain.y),
            (cw as isize, (2 * ch + ch) as isize),
            "a same-size region's zone did not land on the parent grid"
        );
        let big = region_at(30.0);
        assert_eq!(big.x, cw as isize, "the region's origin moved with its px");
        assert!(
            big.h > plain.h && big.w > plain.w && big.y > plain.y,
            "a 30 px region's zone is no bigger than a 15 px one: {big:?} vs {plain:?}"
        );
    }

    /// A zone rides its region's scroll, like the pixels under it: the same op, in a
    /// region scrolled half a cell, is half a cell higher.
    #[test]
    fn a_zone_rides_the_scroll_of_its_region() {
        let (_, ch) = window_cell();
        let at = |dy_frac: f32| {
            zones(&[Op::ScrollRegion {
                dy_frac,
                ops: vec![zone(0, 4, 2, 1)],
            }])[0]
        };
        assert_eq!(
            at(0.0).y - at(0.5).y,
            scroll_px(0.5, ch),
            "the zone did not move with the region's offset"
        );
    }

    /// A region is a window for its zones too: one that falls outside the rect is
    /// clipped away, so a zoomed pane cannot claim the pointer over its neighbour.
    #[test]
    fn a_region_clips_the_zones_it_holds() {
        let inside = zones(&[Op::CellRegion {
            x: 0,
            y: 0,
            w: 40,
            h: 2,
            px: 15.0,
            ops: vec![zone(0, 0, 4, 1), zone(0, 9, 4, 1)],
        }]);
        assert_eq!(
            inside.len(),
            1,
            "a zone below a 2-row region survived: {inside:?}"
        );
    }

    /// Wild geometry and sizes come straight from an app's render op. None of it may
    /// panic (an overflow under debug-assertions, an out-of-bounds write) or hang.
    #[test]
    fn wild_zone_geometry_does_not_overflow_the_coordinate_math() {
        for px in [1.0f32, 3.5, 1e9, f32::MAX] {
            for (x, y, w, h) in [
                (0, 0, 0, 0),
                (u16::MAX, u16::MAX, u16::MAX, u16::MAX),
                (0, u16::MAX, 40, 4),
                (200, 0, 4, 200),
            ] {
                zones(&[Op::CellRegion {
                    x,
                    y,
                    w,
                    h,
                    px,
                    ops: vec![
                        zone(x, y, w, h),
                        Op::ScrollRegion {
                            dy_frac: f32::MAX,
                            ops: vec![zone(x, y, w, h)],
                        },
                    ],
                }]);
            }
        }
    }

    /// Wild geometry and sizes come straight from an app's render op. None of it may
    /// panic (an overflow under debug-assertions, an out-of-bounds write) or hang.
    #[test]
    fn wild_region_geometry_does_not_overflow_the_coordinate_math() {
        for px in [1.0f32, 3.5, 1e9, f32::MAX] {
            for (x, y, w, h) in [
                (0, 0, 0, 0),
                (u16::MAX, u16::MAX, u16::MAX, u16::MAX),
                (0, u16::MAX, 40, 4),
                (200, 0, 4, 200),
            ] {
                paint(&[Op::CellRegion {
                    x,
                    y,
                    w,
                    h,
                    px,
                    ops: vec![text(0, 0), text(9, 9)],
                }]);
            }
        }
    }
}
