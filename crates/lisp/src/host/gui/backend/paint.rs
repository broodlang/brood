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
            let top = oy as f32 + y * ch as f32;
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

/// Which strips of the framebuffer must be re-rasterised to turn the frame `old`
/// painted into the frame `new` would paint: those whose sequence of covering leaf
/// ops differs — an op added, removed, changed, reordered, or shifted by a scroll
/// region. Per strip, the ops are compared as values (`Op: PartialEq`), so an
/// identical frame dirties nothing and a one-line edit dirties one strip. Returns a
/// bool per strip.
fn strip_diff(new: &[Op], old: &[Op], strips: &Strips, oy: usize, ch: usize) -> Vec<bool> {
    // The leaf sequences covering each strip, as indices into the flattened lists.
    fn by_strip<'a>(
        ops: &'a [Op],
        strips: &Strips,
        oy: usize,
        ch: usize,
    ) -> (Vec<Entry<'a>>, Vec<Vec<u32>>) {
        let mut leaves = Vec::new();
        flatten(ops, ch, 0, &mut leaves);
        let mut per = vec![Vec::new(); strips.count];
        for (i, e) in leaves.iter().enumerate() {
            if let Some((top, bottom)) = op_band(e.op, e.dy, oy, ch) {
                for k in strips.covering(top, bottom) {
                    per[k].push(i as u32);
                }
            }
        }
        (leaves, per)
    }
    let (new_leaves, new_per) = by_strip(new, strips, oy, ch);
    let (old_leaves, old_per) = by_strip(old, strips, oy, ch);
    (0..strips.count)
        .map(|k| {
            let (a, b) = (&new_per[k], &old_per[k]);
            a.len() != b.len()
                || a.iter().zip(b).any(|(&i, &j)| {
                    let (x, y) = (&new_leaves[i as usize], &old_leaves[j as usize]);
                    x.dy != y.dy || x.op != y.op
                })
        })
        .collect()
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
                    fill_rrect(
                        canvas,
                        ox as f32 + *x * cw as f32,
                        oy as f32 + *y * ch as f32,
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
/// `force` (the caller wants a clean full render). Records the frame as the new
/// previous one and returns the pixel bands `[y0, y1)` that were repainted.
pub(super) fn raster_frame(
    r: &mut Renderer,
    frame: &[Op],
    fb_w: usize,
    fb_h: usize,
    force: bool,
) -> Vec<(usize, usize)> {
    let n = fb_w * fb_h;
    let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
    let (ox, oy) = r.grid_origin(fb_w, fb_h);
    let strips = Strips::new(oy, ch, fb_h);
    let fresh = r.canvas.len() != n;
    if fresh {
        r.canvas = vec![0u32; n];
    }
    let bands = if fresh || force || r.prev_ops.is_empty() || !gui_damage_enabled() {
        vec![(0, fb_h)]
    } else if frame == r.prev_ops.as_slice() {
        Vec::new()
    } else {
        dirty_bands(&strip_diff(frame, &r.prev_ops, &strips, oy, ch), &strips)
    };
    let bg0 = pack(r.bg());
    // The canvas leaves the renderer for the duration of the raster (the primitives
    // borrow both), and comes back untouched in shape.
    let mut pixels = std::mem::take(&mut r.canvas);
    for &(y0, y1) in &bands {
        let mut canvas = Canvas::band(&mut pixels, fb_w, fb_h, y0, y1);
        // The conventional frame opens with a full `:clear`; a frame that doesn't
        // still expects a clean background, and a strip repaint needs one either
        // way (the strip's old pixels are exactly what changed).
        canvas.fill_rows(y0, y1 - y0, bg0);
        render_ops(frame, &mut canvas, r, ox, oy, cw, ch, bg0, 0);
    }
    r.canvas = pixels;
    if !bands.is_empty() || r.prev_ops.is_empty() {
        r.prev_ops = frame.to_vec();
    }
    bands
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
    let resized = r.canvas.len() != fb_w * fb_h;
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
        Err(_) => return,
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
            let rows: usize = bands.iter().map(|(a, b)| b - a).sum();
            let shipped: usize = rects.iter().map(|d| d.y1 - d.y0).sum();
            let aa = if r.subpixel_text() {
                "subpixel"
            } else {
                "gray"
            };
            eprintln!(
                "[gui-paint] {}us: fb={fb_w}x{fb_h} ops={op_count} rows={rows}/{fb_h} \
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
        strip_diff(new, old, &strips, oy, ch)
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
        let all: Vec<bool> = strip_diff(&new, &old, &strips, 0, 10);
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
