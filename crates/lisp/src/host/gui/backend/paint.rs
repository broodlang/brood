//! Painting a frame: turn a window's render ops into pixels — incrementally. The
//! renderer retains the last frame (its ops and the canvas they rasterised to); a new
//! frame is diffed against it per cell row (`strip_diff`), only the rows whose op
//! sequence changed are re-rasterised — within a row, only the columns its changed ops can
//! paint (`strip_columns`) — and only those rects are copied to the window. A keystroke
//! re-rasterises the cells it touched and the mode line's, not the screen.
//! Runs on the GUI thread, where a panic takes the event loop with it — so
//! `render_ops` is fuzzed against wild inputs here.

mod strips;

use super::render::CpuTexture;
use super::rows::RowMap;
use super::*;
#[cfg(feature = "gui-gpu")]
pub(crate) use strips::holds_blinking_caret;
use strips::*;

// Paint-breakdown diagnostics (BROOD_STALL_MS): single GUI thread, so Relaxed is
// fine. Reset at each `paint` entry; when the paint runs >= the threshold, the
// breakdown line tells us whether the time is glyph re-rasterization (cache
// misses — e.g. an animated `:scale`) or just blitting a lot of pixels.
pub(super) static PAINT_CLUSTERS: AtomicU64 = AtomicU64::new(0);

pub(super) static PAINT_MISSES: AtomicU64 = AtomicU64::new(0);

pub(super) static PAINT_BUILD_NS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn paint_stall_ms() -> Option<u128> {
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

/// The retained frame: what the last paint left, which the next one is diffed against.
/// `canvas` holds the pixels of the last frame rasterised and `prev_ops` the ops that produced it.
/// A new frame is compared with `prev_ops` per cell row (`strips`), and only the rows that changed
/// are re-rasterised into `canvas`.
#[derive(Default)]
pub(super) struct RetainedFrame {
    pub(super) canvas: Vec<u32>,
    /// `canvas`'s `(width, height)` — compared as a pair, not a length: a rotation of
    /// the window (800×600 → 600×800) keeps the pixel count and changes every row.
    pub(super) canvas_size: (usize, usize),
    pub(super) prev_ops: Vec<Op>,
    /// Whether the blinking caret was in its hidden half for the paint that left `canvas` as
    /// it is: the CPU painter re-rasterises only the caret's rows when that changed.
    pub(super) prev_caret_hidden: bool,
    /// The changed pixel rects of recent frames, oldest first, so a present can cover the
    /// last `buffer.age()` frames.
    pub(super) damage_ring: Vec<Vec<DamageRect>>,
    /// The pixel rows the last `raster_frame` copied from the old canvas instead of drawing
    /// (the scroll blit): a per-paint stat for the trace and the tests.
    pub(super) blit_rows: usize,
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

    /// The whole `w`×`h` framebuffer.
    pub(super) fn whole(w: usize, h: usize) -> DamageRect {
        DamageRect {
            x0: 0,
            y0: 0,
            x1: w,
            y1: h,
        }
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

/// Column damage (`strip_columns`) is on by default; `BROOD_GUI_COLUMNS=0` re-rasterises a
/// changed strip across the whole row, as before — the escape hatch if a leaf ever paints
/// outside the columns `leaf_columns` gives it. Read once.
pub(super) fn gui_columns_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BROOD_GUI_COLUMNS")
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
            Op::Fragment(fragment) => {
                collect_zones(&fragment.0, r, ox, oy, cw, ch, scroll_dy, clip, zones);
            }
            // Zones are in the grid's uniform space, where a group above the seam is laid out
            // already (ADR-615); cut off at the seam's row, whose uniform pixels are the
            // bottom-anchored row's zones'. A zone on the partial line is not hoverable: a
            // pointer in the strip takes the zones of the row above (`RowMap::uniform_y`).
            Op::AboveSeam { ops } => {
                // (on the window's grid only: inside a cell region there is no seam)
                let on_window_grid = oy == 0 && ch == r.cell_h.max(1);
                let cut = match r.seam() {
                    Some(seam) if seam > 0 && on_window_grid => (seam as isize) * ch as isize,
                    _ => isize::MAX,
                };
                let band = (clip.0, clip.1.min(cut));
                collect_zones(ops, r, ox, oy, cw, ch, scroll_dy, band, zones);
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

/// The pixel rows a caret on grid `row` paints: its cell's top (`RowMap::y`), which is above
/// the grid's top while the row scrolls off the viewport's top, and the span drawn, clipped to
/// `[top, fb_h)` rather
/// than dropped, so the caret tracks its text through a whole smooth scroll. `None` when
/// none of it shows.
fn caret_rows(
    row: u16,
    rows: &RowMap,
    scroll_dy: isize,
    fb_h: usize,
) -> Option<(isize, usize, usize)> {
    let cell_top = rows.y(row as isize).saturating_sub(scroll_dy);
    let draw_top = cell_top.max(rows.top as isize);
    // a caret is a cell tall, also on the row above a seam
    let draw_bottom = cell_top.saturating_add(rows.ch as isize).min(fb_h as isize);
    (draw_top < draw_bottom).then_some((cell_top, draw_top as usize, draw_bottom as usize))
}

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
    rows: &RowMap,
    cw: usize,
    bg0: u32,
    scroll_dy: isize,
) {
    let fb_h = canvas.h;
    // `oy` is the clip every shifted op stops at (nothing is painted above row 0); every
    // row's pixel top comes from the map (`rows.y`), never from `oy + row * ch`
    let (oy, ch) = (rows.top, rows.ch);
    for op in ops {
        if let Some((top, bottom)) = op_band(op, scroll_dy, rows)
            && (bottom <= canvas.y0 as isize || top >= canvas.y1 as isize)
        {
            continue;
        }
        match op {
            Op::Clear => canvas.fill_rows(0, fb_h, bg0),
            Op::ScrollRegion { dy_frac, ops } => {
                let inner_dy = scroll_px(*dy_frac, ch);
                render_ops(ops, canvas, r, ox, rows, cw, bg0, inner_dy);
            }
            Op::Fragment(fragment) => {
                render_ops(&fragment.0, canvas, r, ox, rows, cw, bg0, scroll_dy);
            }
            Op::AboveSeam { ops } => {
                // ADR-615: the rows above the seam carried on with no gap, the band cut off
                // where the seam's row begins — so what the group puts on the seam's row shows
                // its top in the leftover strip, and nothing of it reaches the rows below.
                // Under half a row of strip the window's own rows: phase 1 (`above_seam_rows`).
                let cut = rows.seam_top().unwrap_or(isize::MAX).max(0) as usize;
                let mut inner = canvas.narrowed(0, cut);
                if inner.y1 > inner.y0 {
                    let uniform = rows.above_seam_rows();
                    render_ops(ops, &mut inner, r, ox, &uniform, cw, bg0, scroll_dy);
                }
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
                let top = rows.y(*y as isize).saturating_sub(scroll_dy);
                let bottom = top.saturating_add(rows.span(*y as isize, *h as isize));
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
                let mut inner = canvas.narrowed(y0, y1);
                render_ops(
                    ops,
                    &mut inner,
                    r,
                    region_ox,
                    &RowMap::uniform(region_oy, region.cell_h),
                    region.cell_w,
                    bg0,
                    0,
                );
                r.set_metrics(saved);
            }
            Op::Text { row, col, s, face } => {
                let (mut fg, mut bg) = (face.fg.unwrap_or(r.fg()), face.bg.unwrap_or(r.bg()));
                // Only paint a cell background when the face specifies one (or is
                // reversed). A face with no `:bg` is TRANSPARENT — the glyph composites
                // over whatever's already there (the frame clear, or an hl-line /
                // selection `rect` band drawn under the text), so the current-line
                // highlight shows behind the text too, exactly like Emacs. (Before,
                // every glyph filled its cell with DEFAULT_BG, painting over the band so
                // hl-line only showed in the line's trailing empty space.) For an
                // un-banded line the pixel beneath is already the window background, so this is
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
                let top_signed = rows.y(*row as isize).saturating_sub(scroll_dy);
                let clip_skip = (oy as isize - top_signed).max(0) as usize;
                if clip_skip >= ch_s {
                    continue; // entirely above the grid origin
                }
                let visible_h = ch_s - clip_skip;
                let render_top = top_signed.max(oy as isize) as usize;
                let mut cx = *col as usize;
                let bg_packed = pack(bg);
                // With `gui-ligatures!` on, a punctuation run is ONE unit — shaped together
                // so the font's ligatures form, one canvas spanning its cells (ADR-442).
                for g in text_units(s, r.ligatures()) {
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
                    let cells = unit_cells(g);
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
                        // a rule (or a wave) near the block bottom, in its own colour or the
                        // text's, scaled with the glyph so it stays proportional
                        let uy_signed = top_signed + ch_s as isize - 2 * scale as isize;
                        let colour = pack(face.underline_rgb.unwrap_or(fg));
                        for (x, y, w, h) in crate::host::gui::underline_rects(
                            left as isize,
                            uy_signed,
                            block_w,
                            scale,
                            face.underline_wave,
                        ) {
                            if y >= oy as isize && x >= 0 {
                                fill_cell(canvas, x as usize, y as usize, w, h, colour);
                            }
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
                    let top_signed = rows.y(*row as isize).saturating_sub(scroll_dy);
                    // through the map: a panel whose bottom is the seam covers the gap
                    let h_px = rows.span(*row as isize, *h as isize);
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
                        rows.yf(*y) - scroll_dy as f32,
                        *w * cw as f32,
                        rows.spanf(*y, *h),
                        *radius * cw as f32,
                        bg,
                        *opacity,
                        r.scale() as f32,
                    );
                }
            }
            // A blinking caret in its hidden half (ADR-469) paints nothing.
            Op::Cursor { blink: Some(_), .. } if r.caret_hidden => {}
            // A block with the text under it, Emacs's box (ADR-515): the cell solid in the
            // caret's colour, the text over it in its own face, coloured the window background.
            Op::Cursor {
                row,
                col,
                style: crate::host::gui::CursorStyle::Block,
                color,
                under: Some(under),
                ..
            } => {
                if let Some((_, draw_top, draw_bottom)) = caret_rows(*row, rows, scroll_dy, fb_h) {
                    fill_cell(
                        canvas,
                        ox + *col as usize * cw,
                        draw_top,
                        caret_cells(&under.0) * cw,
                        draw_bottom - draw_top,
                        pack(color.unwrap_or(CURSOR_FG)),
                    );
                    let glyph = caret_glyph(*row, *col, under, r.bg());
                    render_ops(
                        std::slice::from_ref(&glyph),
                        canvas,
                        r,
                        ox,
                        rows,
                        cw,
                        bg0,
                        scroll_dy,
                    );
                }
            }
            Op::Cursor {
                row,
                col,
                style,
                color,
                ..
            } => {
                if let Some((cell_top, draw_top, draw_bottom)) =
                    caret_rows(*row, rows, scroll_dy, fb_h)
                {
                    cursor_cell(
                        canvas,
                        ox + *col as usize * cw,
                        draw_top,
                        draw_bottom,
                        cw,
                        ch,
                        cell_top,
                        *style,
                        *color,
                    );
                }
            }
            // Not painted — a cursor zone is hover metadata, hit-tested on
            // pointer-move in the window event handler (ADR-080).
            Op::CursorZone { .. } => {}
            // A sound is played by the window, not painted.
            Op::Sound { .. } | Op::Music { .. } => {}
            // The frame's effects are passes `raster_frame` runs around these ops.
            Op::Ambient { .. }
            | Op::Light { .. }
            | Op::Overlay
            | Op::Bloom { .. }
            | Op::Crt { .. }
            | Op::Burst { .. } => {}
            // Placed the rows this walk paints with (`Renderer::rows`); paints nothing itself.
            Op::Seam { .. } => {}
            Op::Emitter { id, .. } => paint_particles(canvas, r, *id),
            // The pixel-space ops, in software: the GPU target draws them instanced; here
            // each pixel of the quad's bounding box is turned back into the quad's own
            // frame and filled or sampled if it lands inside. Slow next to the GPU, but
            // a game on a build without one (or under `BROOD_GUI_GPU=0`) draws instead of
            // showing its text alone.
            Op::Quad {
                x,
                y,
                w,
                h,
                color,
                rot,
            } => paint_quad(canvas, *x, *y, *w, *h, *color, *rot),
            Op::Sprite {
                tex,
                x,
                y,
                w,
                h,
                uv,
                tint,
                rot,
            } => {
                if let Some(t) = r.textures.get(tex) {
                    paint_sprite(canvas, t, *x, *y, *w, *h, *uv, *tint, *rot);
                }
            }
            // Pixel-space text: the `Text` walk with the run's top-left given in
            // pixels (after the alignment's shift by the run's shaped width) rather
            // than derived from a cell; unaffected by grid origin and scroll.
            Op::TextPx {
                x,
                y,
                s,
                face,
                align,
            } => {
                let (mut fg, mut bg) = (face.fg.unwrap_or(r.fg()), face.bg.unwrap_or(r.bg()));
                let mut paint_bg = face.bg.is_some();
                if face.reverse {
                    std::mem::swap(&mut fg, &mut bg);
                    paint_bg = true;
                }
                let scale = face.scale.max(1) as usize;
                let ch_s = ch * scale;
                let width = text_px_width(s, cw, scale) as f32;
                let left0 = match align {
                    crate::host::gui::Align::Left => *x,
                    crate::host::gui::Align::Center => *x - width / 2.0,
                    crate::host::gui::Align::Right => *x - width,
                };
                let top_signed = y.round() as isize;
                let clip_skip = (-top_signed).max(0) as usize;
                if clip_skip >= ch_s {
                    continue;
                }
                let render_top = top_signed.max(0) as usize;
                let visible_h = ch_s - clip_skip;
                let mut cx = 0usize;
                let bg_packed = pack(bg);
                for g in s.graphemes(true) {
                    let cells = cluster_cells(g);
                    if cells == 0 {
                        continue;
                    }
                    let block_w = cells * cw * scale;
                    let left_signed = left0.round() as isize + cx as isize;
                    if left_signed >= 0 {
                        let left = left_signed as usize;
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
                            let uy = top_signed + ch_s as isize - 2 * scale as isize;
                            let colour = pack(face.underline_rgb.unwrap_or(fg));
                            for (x, y, w, h) in crate::host::gui::underline_rects(
                                left as isize,
                                uy,
                                block_w,
                                scale,
                                face.underline_wave,
                            ) {
                                if y >= 0 && x >= 0 {
                                    fill_cell(canvas, x as usize, y as usize, w, h, colour);
                                }
                            }
                        }
                    }
                    cx += block_w;
                }
            }
            Op::VSpans { row0, col0, cols } => {
                for (i, segs) in cols.iter().enumerate() {
                    let left = ox + (*col0 as usize + i) * cw;
                    let mut row = *row0 as isize;
                    for (h, color) in segs {
                        let (top, span_h) = (rows.y(row), rows.span(row, *h as isize));
                        if let Some(rgb) = color {
                            fill_cell(
                                canvas,
                                left,
                                top.max(0) as usize,
                                cw,
                                span_h.max(0) as usize,
                                pack(*rgb),
                            );
                        }
                        row = row.saturating_add(*h as isize);
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
                            let row = (*row0 as usize + y) as isize;
                            let top = rows.y(row).max(0) as usize;
                            let span_h = rows.span(row, 1).max(0) as usize;
                            fill_cell(canvas, left, top, cell_w, span_h, packed);
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
                        let row = (*row0 as usize + y) as isize;
                        let top = rows.y(row).max(0) as usize;
                        let span_h = rows.span(row, 1).max(0) as usize;
                        fill_cell(canvas, left, top, cell_w, span_h, pack(rgb));
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
// ---- the pixel-space ops, in software ----------------------------------------------

/// The vertical centre and the half-diagonal of a `w`×`h` quad at `(x, y)` — the extent
/// its rotated bounding box can reach in y, whatever the angle.
fn quad_extent(y: f32, w: f32, h: f32) -> (f32, f32) {
    (y + h / 2.0, (w * w + h * h).sqrt() / 2.0)
}

/// `src` over `dst` (both 0x00RRGGBB) with straight alpha `a` (0..=255).
fn over(dst: u32, src: [u8; 3], a: u32) -> u32 {
    if a >= 255 {
        return pack(src);
    }
    if a == 0 {
        return dst;
    }
    let inv = 255 - a;
    let ch = |d: u32, s: u8| (d * inv + s as u32 * a + 127) / 255;
    let dr = (dst >> 16) & 0xff;
    let dg = (dst >> 8) & 0xff;
    let db = dst & 0xff;
    (ch(dr, src[0]) << 16) | (ch(dg, src[1]) << 8) | ch(db, src[2])
}

/// Walk every pixel of the `w`×`h` quad at `(x, y)` turned `rot` radians about its
/// centre, clipped to the canvas band: `f(px, py, u, v)` gets the pixel and where in the
/// quad it lies (`u`, `v` in 0..1 from the top-left corner). A non-finite or empty quad
/// walks nothing, however wild the geometry a frame carries.
fn each_quad_pixel(
    canvas: &Canvas,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rot: f32,
    mut f: impl FnMut(usize, usize, f32, f32),
) {
    if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite() && rot.is_finite())
        || w <= 0.0
        || h <= 0.0
    {
        return;
    }
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let half = (w * w + h * h).sqrt() / 2.0;
    let x0 = ((cx - half).floor().max(0.0) as usize).max(canvas.x0);
    let x1 = ((cx + half).ceil().max(0.0) as usize)
        .min(canvas.w)
        .min(canvas.x1);
    let y0 = (cy - half).floor().max(0.0) as usize;
    let y1 = ((cy + half).ceil().max(0.0) as usize).min(canvas.h);
    let (s, c) = if rot == 0.0 {
        (0.0, 1.0)
    } else {
        rot.sin_cos()
    };
    for py in canvas.rows(y0, y1.saturating_sub(y0)) {
        for px in x0..x1 {
            // the pixel centre, in the quad's own frame: undo the rotation about the centre
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let lx = dx * c + dy * s;
            let ly = -dx * s + dy * c;
            let u = lx / w + 0.5;
            let v = ly / h + 0.5;
            if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                f(px, py, u, v);
            }
        }
    }
}

/// A solid `[:quad …]`: the colour blended over the canvas by its alpha.
fn paint_quad(canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32, color: [u8; 4], rot: f32) {
    let rgb = [color[0], color[1], color[2]];
    let a = color[3] as u32;
    let fb_w = canvas.w;
    let mut hits: Vec<usize> = Vec::new();
    each_quad_pixel(canvas, x, y, w, h, rot, |px, py, _, _| {
        hits.push(py * fb_w + px)
    });
    for i in hits {
        canvas.buf[i] = over(canvas.buf[i], rgb, a);
    }
}

/// A `[:sprite …]`: the texture's `uv` rect (`[u0 v0 du dv]`, a negative extent mirrors)
/// sampled nearest-neighbour across the quad, multiplied by `tint`, blended by the
/// texel's alpha times the tint's.
#[allow(clippy::too_many_arguments)]
fn paint_sprite(
    canvas: &mut Canvas,
    t: &CpuTexture,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv: [f32; 4],
    tint: [u8; 4],
    rot: f32,
) {
    if t.w == 0 || t.h == 0 || t.rgba.len() < (t.w as usize * t.h as usize * 4) {
        return;
    }
    let fb_w = canvas.w;
    let (tw, th) = (t.w as f32, t.h as f32);
    let mut hits: Vec<(usize, u32)> = Vec::new();
    each_quad_pixel(canvas, x, y, w, h, rot, |px, py, u, v| {
        let tu = uv[0] + u * uv[2];
        let tv = uv[1] + v * uv[3];
        let sx = ((tu * tw).floor().clamp(0.0, tw - 1.0)) as usize;
        let sy = ((tv * th).floor().clamp(0.0, th - 1.0)) as usize;
        let i = (sy * t.w as usize + sx) * 4;
        let texel = [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2], t.rgba[i + 3]];
        let mul = |c: u8, k: u8| ((c as u32 * k as u32 + 127) / 255) as u8;
        let rgb = [
            mul(texel[0], tint[0]),
            mul(texel[1], tint[1]),
            mul(texel[2], tint[2]),
        ];
        let a = mul(texel[3], tint[3]) as u32;
        hits.push((py * fb_w + px, (a << 24) | pack(rgb)));
    });
    for (i, packed) in hits {
        let a = packed >> 24;
        let rgb = [
            ((packed >> 16) & 0xff) as u8,
            ((packed >> 8) & 0xff) as u8,
            (packed & 0xff) as u8,
        ];
        canvas.buf[i] = over(canvas.buf[i], rgb, a);
    }
}

/// Emitter `id`'s live particles, each a `size`-wide disc (or square) of its colour —
/// blended over the canvas, or added onto it for an additive emitter. The GPU's
/// `fs_particle`, per pixel.
fn paint_particles(canvas: &mut Canvas, r: &Renderer, id: u32) {
    use crate::host::gui::effects::particle_coverage;
    let Some(spec) = r.cpu_particles.spec(id).copied() else {
        return;
    };
    let fb_w = canvas.w;
    for (x, y, size, rgba) in r.cpu_particles.particles(id) {
        if !(x.is_finite() && y.is_finite()) || size <= 0.0 {
            continue;
        }
        let half = size / 2.0;
        let x0 = ((x - half).floor().max(0.0) as usize).max(canvas.x0);
        let x1 = ((x + half).ceil().max(0.0) as usize)
            .min(fb_w)
            .min(canvas.x1);
        let y0 = ((y - half).floor().max(0.0) as usize).max(canvas.y0);
        let y1 = ((y + half).ceil().max(0.0) as usize).min(canvas.y1);
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - x;
                let dy = py as f32 + 0.5 - y;
                let distance = (dx * dx + dy * dy).sqrt() / size;
                let alpha = rgba[3] * particle_coverage(distance, spec.square);
                if alpha <= 0.0 {
                    continue;
                }
                let i = py * fb_w + px;
                let rgb = [
                    (rgba[0] * 255.0) as u8,
                    (rgba[1] * 255.0) as u8,
                    (rgba[2] * 255.0) as u8,
                ];
                canvas.buf[i] = if spec.additive {
                    let add = |shift: u32, c: u8| {
                        let d = (canvas.buf[i] >> shift) & 0xff;
                        ((d as f32 + c as f32 * alpha).min(255.0) as u32) << shift
                    };
                    add(16, rgb[0]) | add(8, rgb[1]) | add(0, rgb[2])
                } else {
                    over(canvas.buf[i], rgb, (alpha * 255.0) as u32)
                };
            }
        }
    }
}

/// A frame with lighting, post-processing or particles in flight, rasterised whole:
/// the scene (the ops before the top-level `Overlay`), the particles of emitters the
/// frame no longer carries, the light map over both, the overlay, then bloom and CRT —
/// the GPU target's passes, in software.
fn raster_effects(
    r: &mut Renderer,
    frame: &[Op],
    pixels: &mut [u32],
    fb_w: usize,
    fb_h: usize,
    fx: &crate::host::gui::effects::FrameEffects,
) {
    use crate::host::gui::effects::{apply_bloom, apply_crt, apply_lighting};
    let cw = r.cell_w.max(1);
    let (ox, _) = r.grid_origin(fb_w, fb_h);
    let rows = r.rows(fb_w, fb_h);
    let bg0 = pack(r.bg());
    let split = fx.overlay_at.unwrap_or(frame.len());
    let (scene, overlay) = frame.split_at(split);
    let mut canvas = Canvas::band(pixels, fb_w, fb_h, 0, fb_h);
    canvas.fill_rows(0, fb_h, bg0);
    render_ops(scene, &mut canvas, r, ox, &rows, cw, bg0, 0);
    let carried: Vec<u32> = frame
        .iter()
        .filter_map(|op| match op {
            Op::Emitter { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    let lingering: Vec<u32> = r
        .cpu_particles
        .ids()
        .filter(|id| !carried.contains(id))
        .collect();
    for id in lingering {
        paint_particles(&mut canvas, r, id);
    }
    apply_lighting(pixels, fb_w, fb_h, fx);
    let mut canvas = Canvas::band(pixels, fb_w, fb_h, 0, fb_h);
    render_ops(overlay, &mut canvas, r, ox, &rows, cw, bg0, 0);
    if let Some(bloom) = fx.bloom {
        apply_bloom(pixels, fb_w, fb_h, bloom);
    }
    if let Some(crt) = fx.crt {
        apply_crt(pixels, fb_w, fb_h, crt);
    }
}

pub(super) fn raster_frame(
    r: &mut Renderer,
    frame: &[Op],
    fb_w: usize,
    fb_h: usize,
    force: bool,
) -> Vec<DamageRect> {
    let cw = r.cell_w.max(1);
    let (ox, _) = r.grid_origin(fb_w, fb_h);
    let rows = r.rows(fb_w, fb_h);
    let ch = rows.ch;
    let strips = Strips::new(rows, fb_h);
    let fresh = r.retained.canvas_size != (fb_w, fb_h);
    if fresh {
        r.retained.canvas = vec![0u32; fb_w * fb_h];
        r.retained.canvas_size = (fb_w, fb_h);
    }
    // Particles move between paints of the same frame, and an effect reaches every pixel,
    // so neither has a damage to diff: the whole frame is rasterised.
    let steps = r.emitters.step(frame);
    r.cpu_particles.step(&steps);
    let fx = crate::host::gui::effects::FrameEffects::of(frame);
    if fx.any() || !steps.is_empty() {
        let mut pixels = std::mem::take(&mut r.retained.canvas);
        raster_effects(r, frame, &mut pixels, fb_w, fb_h, &fx);
        r.retained.canvas = pixels;
        r.retained.blit_rows = 0;
        // the next frame without effects starts from a full raster, not a diff
        r.retained.prev_ops.clear();
        return vec![DamageRect::whole(fb_w, fb_h)];
    }
    // `same`: nothing to raster — the frame is the one painted, and so is the blink phase
    // of its caret (a phase flip of an unchanged frame dirties just the caret's row).
    let same_ops = frame == r.retained.prev_ops.as_slice();
    let same = same_ops && r.caret_hidden == r.retained.prev_caret_hidden;
    // `changed`: every rect whose pixels differ after this raster (what the present
    // ships); `raster`: the subset drawn; `blits`: the strips copied from old rows.
    let whole = || vec![DamageRect::whole(fb_w, fb_h)];
    let (changed, raster, blits) =
        if fresh || force || r.retained.prev_ops.is_empty() || !gui_damage_enabled() {
            (whole(), whole(), Vec::new())
        } else if same {
            (Vec::new(), Vec::new(), Vec::new())
        } else {
            let new = Leaves::of(frame, &strips, r.caret_hidden);
            let old = Leaves::of(&r.retained.prev_ops, &strips, r.retained.prev_caret_hidden);
            let mut dirty = strip_diff(&new, &old, &strips);
            let blits = if gui_blit_enabled() {
                strip_blits(&new, &old, &dirty, &strips, fb_h, cw)
            } else {
                Vec::new()
            };
            // A blitted strip is copied whole; a drawn one only in its changed columns.
            let mut columns = vec![(0, fb_w); strips.count];
            if gui_columns_enabled() {
                let ligatures = r.ligatures();
                for k in (0..strips.count).filter(|&k| dirty[k]) {
                    columns[k] = strip_columns(&new, &old, k, fb_w, ox, cw, ligatures);
                }
            }
            for &(k, _) in &blits {
                columns[k] = (0, fb_w);
            }
            let changed = dirty_rects(&dirty, &columns, &strips);
            for &(k, _) in &blits {
                dirty[k] = false;
            }
            (changed, dirty_rects(&dirty, &columns, &strips), blits)
        };
    let bg0 = pack(r.bg());
    // The canvas leaves the renderer for the duration of the raster (the primitives
    // borrow both), and comes back untouched in shape.
    let mut pixels = std::mem::take(&mut r.retained.canvas);
    r.retained.blit_rows = blits.len() * ch;
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
    for rect in &raster {
        let mut canvas = Canvas::rect(
            &mut pixels,
            fb_w,
            fb_h,
            (rect.x0, rect.y0),
            (rect.x1, rect.y1),
        );
        // The conventional frame opens with a full `:clear`; a frame that doesn't
        // still expects a clean background, and a strip repaint needs one either
        // way (the rect's old pixels are exactly what changed).
        canvas.fill_rows(rect.y0, rect.y1 - rect.y0, bg0);
        render_ops(frame, &mut canvas, r, ox, &rows, cw, bg0, 0);
    }
    r.retained.canvas = pixels;
    if !same_ops {
        r.retained.prev_ops = frame.to_vec();
    }
    r.retained.prev_caret_hidden = r.caret_hidden;
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
    let resized = r.retained.canvas_size != (fb_w, fb_h);
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
    let full = DamageRect::whole(fb_w, fb_h);
    if resized || !gui_damage_enabled() {
        r.retained.damage_ring.clear();
    }
    r.retained.damage_ring.push(bands.clone());
    if r.retained.damage_ring.len() > DAMAGE_HISTORY {
        let drop = r.retained.damage_ring.len() - DAMAGE_HISTORY;
        r.retained.damage_ring.drain(0..drop);
    }
    let age = buf.age() as usize;
    let rects: Vec<DamageRect> =
        if resized || !gui_damage_enabled() || age == 0 || age > r.retained.damage_ring.len() {
            vec![full]
        } else {
            let mut acc: Vec<DamageRect> = r
                .retained
                .damage_ring
                .iter()
                .rev()
                .take(age)
                .flatten()
                .copied()
                .filter(|d| !d.is_empty())
                .collect();
            // Sort + merge so each pixel is copied once: rects that share rows become their
            // bounding box, and a rect that continues the one above over the same columns
            // extends it (the canvas is the truth everywhere, so a larger copy is exact).
            acc.sort_by_key(|d| (d.y0, d.x0));
            let mut merged: Vec<DamageRect> = Vec::new();
            for d in acc {
                match merged.last_mut() {
                    Some(last)
                        if d.y0 < last.y1
                            || (d.y0 == last.y1 && d.x0 == last.x0 && d.x1 == last.x1) =>
                    {
                        last.x0 = last.x0.min(d.x0);
                        last.x1 = last.x1.max(d.x1);
                        last.y1 = last.y1.max(d.y1);
                    }
                    _ => merged.push(d),
                }
            }
            merged
        };
    for d in &rects {
        for y in d.y0..d.y1 {
            let (a, b) = (y * fb_w + d.x0, y * fb_w + d.x1);
            buf[a..b].copy_from_slice(&r.retained.canvas[a..b]);
        }
    }
    dump_canvas(&r.retained.canvas, fb_w, fb_h);
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
            let blit = r.retained.blit_rows;
            // `rows` is what was drawn: the changed rows less the ones a blit copied;
            // `changed` and `shipped` are pixels — a typed character changes a few cells
            // of one row (column damage, ADR-482), and the present copies only those
            let rows: usize = bands
                .iter()
                .map(|d| d.y1 - d.y0)
                .sum::<usize>()
                .saturating_sub(blit);
            let area = |d: &DamageRect| (d.x1 - d.x0) * (d.y1 - d.y0);
            let changed: usize = bands.iter().map(area).sum();
            let shipped: usize = rects.iter().map(area).sum();
            let aa = if r.subpixel_text() {
                "subpixel"
            } else {
                "gray"
            };
            eprintln!(
                "[gui-paint] {}us: fb={fb_w}x{fb_h} ops={op_count} rows={rows}/{fb_h} blit={blit} \
                 changed={changed}px shipped={shipped}px clusters={clusters} misses={misses} build={build_us}us \
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
        render_ops(
            ops,
            &mut canvas,
            &mut r,
            2,
            &RowMap::uniform(2, ch),
            cw,
            0,
            0,
        );
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
                        color: None,
                        blink: None,
                        under: None,
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
                color: None,
                blink: None,
                under: None,
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
            render_ops(
                ops,
                &mut canvas,
                &mut r,
                0,
                &RowMap::uniform(0, ch),
                cw,
                0,
                0,
            );
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

    /// A zone inside a `CellRegion` (a link in a zoomed pane) is placed at the region's
    /// origin (parent cells) and sized in the REGION's cells — bigger at a bigger px.
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
    fn text_px_width_counts_cells_times_scale() {
        assert_eq!(text_px_width("ab", 9, 1), 18);
        assert_eq!(text_px_width("ab", 9, 2), 36);
        assert_eq!(text_px_width("", 9, 1), 0);
        // a wide (two-cell) cluster spans two cells
        assert_eq!(text_px_width("日", 9, 1), 18);
    }

    #[test]
    fn quads_and_sprites_paint_in_software_and_survive_wild_geometry() {
        let mut r = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
        let (fb_w, fb_h) = (64usize, 48usize);
        let mut buf = vec![0u32; fb_w * fb_h];
        let mut canvas = Canvas::full(&mut buf, fb_w, fb_h);
        // A 2×2 checker texture: red, transparent / transparent, blue.
        r.textures.insert(
            7,
            CpuTexture {
                w: 2,
                h: 2,
                rgba: vec![
                    255, 0, 0, 255, 0, 0, 0, 0, //
                    0, 0, 0, 0, 0, 0, 255, 255,
                ],
            },
        );
        let ops = vec![
            // an opaque 10×6 quad at (4, 4)
            Op::Quad {
                x: 4.0,
                y: 4.0,
                w: 10.0,
                h: 6.0,
                color: [10, 200, 30, 255],
                rot: 0.0,
            },
            // a half-transparent white one over black at (30, 4): mid grey
            Op::Quad {
                x: 30.0,
                y: 4.0,
                w: 4.0,
                h: 4.0,
                color: [255, 255, 255, 128],
                rot: 0.0,
            },
            // a 10×10 square turned 45°: its corners reach past the box, the box's own
            // corners are outside it
            Op::Quad {
                x: 40.0,
                y: 20.0,
                w: 10.0,
                h: 10.0,
                color: [200, 200, 200, 255],
                rot: std::f32::consts::FRAC_PI_4,
            },
            // the checker, 8×8 at (4, 20): top-left quadrant red, bottom-right blue,
            // the others untouched (alpha 0)
            Op::Sprite {
                tex: 7,
                x: 4.0,
                y: 20.0,
                w: 8.0,
                h: 8.0,
                uv: [0.0, 0.0, 1.0, 1.0],
                tint: [255, 255, 255, 255],
                rot: 0.0,
            },
            // an unknown texture paints nothing; wild geometry paints nothing and
            // does not panic
            Op::Sprite {
                tex: 99,
                x: 0.0,
                y: 0.0,
                w: 20.0,
                h: 20.0,
                uv: [0.0, 0.0, 1.0, 1.0],
                tint: [255, 255, 255, 255],
                rot: 0.0,
            },
            Op::Quad {
                x: f32::NAN,
                y: 0.0,
                w: f32::MAX,
                h: f32::INFINITY,
                color: [255, 0, 0, 255],
                rot: f32::NAN,
            },
            Op::Quad {
                x: -1.0e30,
                y: 1.0e30,
                w: 1.0e30,
                h: 1.0e30,
                color: [255, 0, 0, 255],
                rot: 1.0e30,
            },
        ];
        render_ops(
            &ops,
            &mut canvas,
            &mut r,
            0,
            &RowMap::uniform(0, ch),
            cw,
            0,
            0,
        );
        let at = |x: usize, y: usize| buf[y * fb_w + x];
        let green = pack([10, 200, 30]);
        assert_eq!(at(4, 4), green);
        assert_eq!(at(13, 9), green);
        assert_eq!(at(14, 4), 0);
        assert_eq!(at(4, 10), 0);
        // half white over black: every channel near 128
        let grey = at(31, 5);
        for shift in [16, 8, 0] {
            let c = (grey >> shift) & 0xff;
            assert!((126..=129).contains(&c), "channel {c}");
        }
        // the diamond: its centre is filled, the box's corner is not, a point along the
        // diagonal past the box's edge is (the square's corner reaches sqrt(2)/2 × 10 out)
        let light = pack([200, 200, 200]);
        assert_eq!(at(45, 25), light);
        assert_eq!(at(40, 20), 0);
        assert_eq!(at(45, 19), light);
        assert_eq!(at(45, 31), light);
        // the checker's quadrants
        assert_eq!(at(5, 21), pack([255, 0, 0]));
        assert_eq!(at(10, 26), pack([0, 0, 255]));
        assert_eq!(at(10, 21), 0);
        assert_eq!(at(5, 26), 0);
    }

    #[test]
    fn a_quad_changing_rows_is_repainted_by_the_strip_diff() {
        // The band of a rotated quad is its bounding box, so moving one across rows marks
        // both the old and the new rows: after the move the old pixels are the clear colour.
        let mut r = Renderer::new(1.0, default_families(), 14.0);
        let (fb_w, fb_h) = (64usize, 48usize);
        let quad = |y: f32| Op::Quad {
            x: 10.0,
            y,
            w: 8.0,
            h: 8.0,
            color: [255, 255, 255, 255],
            rot: 0.3,
        };
        raster_frame(&mut r, &[Op::Clear, quad(4.0)], fb_w, fb_h, false);
        assert_eq!(r.retained.canvas[8 * fb_w + 14], pack([255, 255, 255]));
        // the incremental raster must agree with a full one after the move
        raster_frame(&mut r, &[Op::Clear, quad(30.0)], fb_w, fb_h, false);
        let mut full = Renderer::new(1.0, default_families(), 14.0);
        raster_frame(&mut full, &[Op::Clear, quad(30.0)], fb_w, fb_h, true);
        assert_ne!(r.retained.canvas[8 * fb_w + 14], pack([255, 255, 255]));
        assert_eq!(r.retained.canvas[34 * fb_w + 14], pack([255, 255, 255]));
        assert!(
            r.retained.canvas == full.retained.canvas,
            "incremental raster of a moved quad differs from a full one"
        );
    }

    #[test]
    fn pixel_text_paints_where_it_says_and_survives_wild_geometry() {
        use crate::host::gui::Align;
        let mut r = Renderer::new(1.0, default_families(), 14.0);
        let (cw, ch) = (r.cell_w.max(1), r.cell_h.max(1));
        let (fb_w, fb_h) = (96usize, 64usize);
        let mut buf = vec![0u32; fb_w * fb_h];
        let mut canvas = Canvas::full(&mut buf, fb_w, fb_h);
        let face = Face {
            bg: Some([10, 20, 30]),
            ..Face::default()
        };
        let op = |x: f32, y: f32, align: Align| Op::TextPx {
            x,
            y,
            s: "ab".into(),
            face,
            align,
        };
        // Left-aligned at (10, 5): the background fills [10, 10 + 2cw) × [5, 5 + ch).
        render_ops(
            &[op(10.0, 5.0, Align::Left)],
            &mut canvas,
            &mut r,
            0,
            &RowMap::uniform(0, ch),
            cw,
            0,
            0,
        );
        let bg = pack([10, 20, 30]);
        assert_eq!(buf[5 * fb_w + 10], bg);
        assert_eq!(buf[5 * fb_w + 10 + 2 * cw - 1], bg);
        assert_ne!(buf[5 * fb_w + 10 + 2 * cw], bg);
        assert_ne!(buf[4 * fb_w + 10], bg);
        // Centred at x = 50: the run starts at 50 - cw.
        let mut buf2 = vec![0u32; fb_w * fb_h];
        let mut canvas2 = Canvas::full(&mut buf2, fb_w, fb_h);
        render_ops(
            &[op(50.0, 5.0, Align::Center)],
            &mut canvas2,
            &mut r,
            0,
            &RowMap::uniform(0, ch),
            cw,
            0,
            0,
        );
        assert_eq!(buf2[5 * fb_w + 50 - cw], bg);
        assert_ne!(buf2[5 * fb_w + 50 - cw - 1], bg);
        // Off the top, off the left, and absurd: no panic, nothing out of bounds.
        let mut buf3 = vec![0u32; fb_w * fb_h];
        let mut canvas3 = Canvas::full(&mut buf3, fb_w, fb_h);
        render_ops(
            &[
                op(-5.0, -3.0, Align::Left),
                op(1.0e9, -1.0e9, Align::Right),
                op(f32::NAN, f32::NAN, Align::Center),
            ],
            &mut canvas3,
            &mut r,
            0,
            &RowMap::uniform(0, ch),
            cw,
            0,
            0,
        );
    }

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

/// The pixel width of a text run shaped at cell width `cw` and face scale `scale` —
/// what a `TextPx` op's alignment shifts by. Grapheme clusters, like the painters walk.
pub(crate) fn text_px_width(s: &str, cw: usize, scale: usize) -> usize {
    s.graphemes(true)
        .map(|g| cluster_cells(g) * cw * scale)
        .sum()
}
