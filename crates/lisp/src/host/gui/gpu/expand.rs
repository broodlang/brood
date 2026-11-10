//! The op walk: a frame's ops expanded into quads on the frame builder, arm for arm with
//! the CPU painter's `render_ops` — the grid they land on, the scroll and cell regions,
//! the cursor's geometry, and a memoised fragment replayed or expanded and recorded.

use super::*;

/// The most a scroll region shifts its ops, in pixels — the CPU painter's cap, so a
/// runaway `dy_frac` moves a region off-screen rather than to infinity.
pub(super) const MAX_SCROLL_PX: f32 = 16384.0;

/// Where the ops being expanded land: the grid's left edge, its rows (`RowMap`, ADR-610:
/// every row's pixel top) and cell size in effect (a `CellRegion` installs its own), the
/// scroll shift an enclosing `ScrollRegion` applies, and the clip rect to restore when a
/// region ends. The CPU painter threads the same values through `render_ops`.
#[derive(Clone, Copy)]
pub(super) struct Grid {
    pub(super) ox: f32,
    pub(super) rows: RowMap,
    pub(super) cw: f32,
    pub(super) ch: f32,
    pub(super) scroll_dy: f32,
}

impl Grid {
    /// The pixel top of (fractional) row `row`, shifted by the scroll in effect.
    pub(super) fn top(&self, row: f32) -> f32 {
        self.rows.yf(row) - self.scroll_dy
    }

    /// The pixel height of the rows `[row, row + n)` (`RowMap::spanf`).
    pub(super) fn span(&self, row: f32, n: f32) -> f32 {
        self.rows.spanf(row, n)
    }
}

/// A scroll region's shift in pixels for a `dy_frac` of the cell height, capped — the
/// CPU painter's `scroll_px`.
pub(super) fn scroll_px(dy_frac: f32, ch: f32) -> f32 {
    (dy_frac * ch).round().clamp(-MAX_SCROLL_PX, MAX_SCROLL_PX)
}

/// The cursor's quads for one cell: `(x, y, w, h, colour)` each. Block is a 50% wash
/// plus a solid rim; Hollow the rim alone, thinner (`cursor_rim`); Bar a caret on the
/// left edge; Underline a rule along the bottom — the CPU `cursor_cell`'s geometry,
/// thickness scaled with the cell so it stays proportional on HiDPI. `color` is the
/// op's caret colour (`None`: a white wash and `CURSOR_FG` strokes).
pub(super) fn cursor_quads(
    left: f32,
    top: f32,
    cw: f32,
    ch: f32,
    style: CursorStyle,
    color: Option<[u8; 3]>,
) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
    let stroke = color.unwrap_or(CURSOR_FG);
    let fg = [stroke[0], stroke[1], stroke[2], 255];
    let rim = |t: f32| {
        vec![
            (left, top, t, ch, fg),
            (left + cw - t, top, t, ch, fg),
            (left, top, cw, t, fg),
            (left, top + ch - t, cw, t, fg),
        ]
    };
    match style {
        CursorStyle::Block | CursorStyle::Hollow => {
            let t = cursor_rim(cw.max(0.0) as usize, style) as f32;
            let mut quads = Vec::with_capacity(5);
            if style == CursorStyle::Block {
                let wash = color.unwrap_or([255, 255, 255]);
                quads.push((left, top, cw, ch, [wash[0], wash[1], wash[2], 128]));
            }
            quads.extend(rim(t));
            quads
        }
        CursorStyle::Bar => vec![(left, top, (cw / 8.0).floor().max(2.0), ch, fg)],
        CursorStyle::Underline => {
            let t = (ch / 10.0).floor().max(2.0);
            vec![(left, top + ch - t, cw, t, fg)]
        }
    }
}

impl GpuWindow {
    /// What a fragment expanded on `fb` at grid `g` depends on besides its ops.
    pub(super) fn fragment_context(
        &self,
        fb: &FrameBuilder,
        renderer: &Renderer,
        g: Grid,
    ) -> FragmentContext {
        let metrics = renderer.metrics();
        FragmentContext {
            grid: {
                let [top, ch, seam, gap] = g.rows.key();
                [
                    g.ox.to_bits(),
                    top,
                    ch,
                    seam,
                    gap,
                    g.cw.to_bits(),
                    g.ch.to_bits(),
                    g.scroll_dy.to_bits(),
                ]
            },
            clip: fb.clip.map(f32::to_bits),
            viewport: [fb.fw.to_bits(), fb.fh.to_bits()],
            metrics: [
                metrics.cell_w as u32,
                metrics.cell_h as u32,
                metrics.px.to_bits(),
                renderer.line_height().to_bits(),
            ],
            family: renderer.default_family(),
            ground: renderer.bg(),
            fonts: (
                renderer.font_generation(),
                renderer.ligatures(),
                self.lcd_order(renderer),
            ),
            scale: renderer.scale().to_bits(),
            textures: self.texture_generation,
            caret_hidden: renderer.caret_hidden,
        }
    }

    /// Expand a memoised fragment (ADR-470): push the instances it produced last frame
    /// when it is the same allocation in the same context (`FragmentCache`), carrying its
    /// runs and nested fragments forward; expand its ops and record them when it is not.
    pub(super) fn expand_fragment(
        &mut self,
        fb: &mut FrameBuilder,
        renderer: &mut Renderer,
        ops: &std::sync::Arc<[Op]>,
        g: Grid,
    ) {
        if !self.replay_fragments {
            self.expand(fb, renderer, ops, g);
            return;
        }
        let context = self.fragment_context(fb, renderer, g);
        if let Some(entry) = self.fragments.take(ops, &context) {
            replay_fragment(fb, &entry);
            for &hash in &entry.runs {
                self.runs.keep(hash);
            }
            for (address, nested) in &entry.nested {
                self.fragments.carry(*address, nested);
            }
            // an enclosing fragment being recorded holds this one, and what it holds
            self.fragments.runs_used.extend_from_slice(&entry.runs);
            self.fragments
                .fragments_used
                .extend_from_slice(&entry.nested);
            self.fragments
                .fragments_used
                .push((fragment_address(ops), entry.context));
            self.fragments.hits += 1;
            self.fragments.restore(entry);
            return;
        }
        let mark = self.fragments.mark(fb);
        self.expand(fb, renderer, ops, g);
        let blinks = ops.iter().any(holds_blinking_caret);
        self.fragments.record(fb, mark, ops, context, blinks);
        self.fragments.misses += 1;
    }

    /// Expand `ops` into quads on `fb`, in order, on grid `g` — the CPU painter's
    /// `render_ops`, arm for arm. Recursion is the two region ops: a scroll region
    /// re-enters with its shift (and clips at the grid top, as the CPU path clips every
    /// shifted op there); a cell region re-enters with its own metrics, origin and clip
    /// band, restoring the renderer's metrics after.
    pub(super) fn expand(
        &mut self,
        fb: &mut FrameBuilder,
        renderer: &mut Renderer,
        ops: &[Op],
        g: Grid,
    ) {
        let scale_factor = renderer.scale() as f32;
        for op in ops {
            match op {
                // The frame was cleared to the background before any op.
                Op::Clear => {}
                // A memoised fragment is its ops in place (ADR-470), in the same grid.
                Op::Fragment(fragment) => self.expand_fragment(fb, renderer, &fragment.0, g),
                Op::ScrollRegion { dy_frac, ops } => {
                    let saved_clip = fb.clip;
                    // A shifted op is clipped at the grid origin, never painted into the
                    // inset above it — the CPU path's `clip_skip`.
                    fb.clip[1] = fb.clip[1].max(g.rows.top as f32);
                    let inner = Grid {
                        scroll_dy: scroll_px(*dy_frac, g.ch),
                        ..g
                    };
                    fb.nested += 1;
                    self.expand(fb, renderer, ops, inner);
                    fb.nested -= 1;
                    fb.clip = saved_clip;
                }
                Op::AboveSeam { ops } => {
                    // ADR-615, `render_ops`' arm: the rows above the seam carried on with no
                    // gap, clipped where the seam's row begins — what the group puts on the
                    // seam's row shows its top in the leftover strip.
                    // Under half a row of strip, the window's own rows: phase 1.
                    let saved_clip = fb.clip;
                    if let Some(cut) = g.rows.seam_top() {
                        fb.clip[3] = fb.clip[3].min(cut as f32);
                    }
                    if fb.clip[3] > fb.clip[1] {
                        let inner = Grid {
                            rows: g.rows.above_seam_rows(),
                            ..g
                        };
                        fb.nested += 1;
                        self.expand(fb, renderer, ops, inner);
                        fb.nested -= 1;
                    }
                    fb.clip = saved_clip;
                }
                Op::CellRegion {
                    x, y, h, px, ops, ..
                } => {
                    // The rect is in the PARENT's cells (and moves with an enclosing
                    // scroll); everything inside is on the region's own grid from its
                    // top-left. The clip band is narrowed to the rect's rows, so an op that
                    // overruns the region vertically is clipped rather than painted over
                    // its neighbour. Width is not clipped (the CPU bands rows, not columns).
                    let region = renderer.metrics_at(*px);
                    if region.cell_w == 0 || region.cell_h == 0 {
                        continue;
                    }
                    let top = g.top(*y as f32);
                    let bottom = top + g.span(*y as f32, *h as f32);
                    let saved_clip = fb.clip;
                    fb.clip[1] = fb.clip[1].max(top).max(0.0);
                    fb.clip[3] = fb.clip[3].min(bottom);
                    if fb.clip[3] > fb.clip[1] {
                        let saved = renderer.metrics();
                        renderer.set_metrics(region);
                        let inner = Grid {
                            ox: g.ox + *x as f32 * g.cw,
                            rows: RowMap::uniform(top.max(0.0) as usize, region.cell_h),
                            cw: region.cell_w as f32,
                            ch: region.cell_h as f32,
                            scroll_dy: 0.0,
                        };
                        fb.nested += 1;
                        self.expand(fb, renderer, ops, inner);
                        fb.nested -= 1;
                        renderer.set_metrics(saved);
                    }
                    fb.clip = saved_clip;
                }
                // Text: the cell BACKGROUNDS as solid quads (a coloured Life cell is a
                // space + `:bg`) — all of the op's cells first, so they form one solid
                // batch — then a textured quad per non-space cluster out of the atlas,
                // which together form one textured batch, then any underline. Shaped once
                // and replayed while the run repeats (`draw_run`).
                Op::Text { row, col, s, face } => {
                    let top = g.top(*row as f32);
                    self.draw_run(
                        fb,
                        renderer,
                        s,
                        face,
                        RunPlace::Cell { col: *col },
                        g,
                        g.ox,
                        top,
                    );
                }
                Op::Rect {
                    row,
                    col,
                    w,
                    h,
                    face,
                    radius,
                } => {
                    let bg = if face.reverse { face.fg } else { face.bg };
                    if let Some(bg) = bg {
                        let (x, y, pw, ph) = (
                            g.ox + *col as f32 * g.cw,
                            g.top(*row as f32),
                            *w as f32 * g.cw,
                            g.span(*row as f32, *h as f32),
                        );
                        if *radius > 0.0 {
                            fb.solid(
                                x,
                                y,
                                pw,
                                ph,
                                [bg[0], bg[1], bg[2], 255],
                                0.0,
                                Edge::Rounded(*radius * g.cw),
                            );
                        } else {
                            fb.fill(x, y, pw, ph, bg);
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
                    // Sub-cell rect: cell-unit floats → px via the same origin and
                    // metrics every op shares, then an AA, alpha-blended fill. A hairline
                    // snaps to whole device pixels first, as on the CPU path.
                    let bg = if face.reverse { face.fg } else { face.bg };
                    if let Some(bg) = bg {
                        let (fx, fw, fy, fh, r) = snap_hairline(
                            g.ox + *x * g.cw,
                            *w * g.cw,
                            g.top(*y),
                            g.span(*y, *h),
                            *radius * g.cw,
                            scale_factor,
                        );
                        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                        fb.solid(
                            fx,
                            fy,
                            fw,
                            fh,
                            [bg[0], bg[1], bg[2], alpha],
                            0.0,
                            Edge::Rounded(r),
                        );
                    }
                }
                // A blinking caret in its hidden half (ADR-469) draws nothing.
                Op::Cursor { blink: Some(_), .. } if renderer.caret_hidden => {}
                // A block with the text under it, Emacs's box (ADR-515): the cell solid in
                // the caret's colour, the text over it in its own face, coloured the window
                // background.
                Op::Cursor {
                    row,
                    col,
                    style: CursorStyle::Block,
                    color,
                    under: Some(under),
                    ..
                } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.top(*row as f32);
                    let cells = caret_cells(&under.0);
                    let fill = color.unwrap_or(CURSOR_FG);
                    fb.solid(
                        left,
                        top,
                        cells as f32 * g.cw,
                        g.ch,
                        [fill[0], fill[1], fill[2], 255],
                        0.0,
                        Edge::Plain,
                    );
                    let glyph = caret_glyph(*row, *col, under, renderer.bg());
                    self.expand(fb, renderer, std::slice::from_ref(&glyph), g);
                }
                Op::Cursor {
                    row,
                    col,
                    style,
                    color,
                    ..
                } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.top(*row as f32);
                    for (x, y, w, h, rgba) in cursor_quads(left, top, g.cw, g.ch, *style, *color) {
                        fb.solid(x, y, w, h, rgba, 0.0, Edge::Plain);
                    }
                }
                // Hover metadata, hit-tested in the event loop (ADR-080); nothing to draw.
                Op::CursorZone { .. } => {}
                // Played when the frame arrived (`UserEvent::Draw`); nothing to draw.
                Op::Sound { .. } | Op::Music { .. } => {}
                // Read off the whole frame by `paint` (`FrameEffects`); nothing to expand.
                Op::Ambient { .. }
                | Op::Light { .. }
                | Op::Bloom { .. }
                | Op::Crt { .. }
                | Op::Burst { .. } => {}
                // Placed the rows (`Grid::rows`); draws nothing itself.
                Op::Seam { .. } => {}
                Op::Overlay => {
                    if fb.nested == 0 && fb.overlay_at.is_none() {
                        fb.overlay_at = Some(fb.batches.len());
                    }
                }
                // Drawn in op order, out of the emitter's own buffer.
                Op::Emitter { id, .. } => fb.batches.push(Batch::Particles { id: *id }),
                Op::VSpans { row0, col0, cols } => {
                    for (i, segs) in cols.iter().enumerate() {
                        let left = g.ox + (*col0 as usize + i) as f32 * g.cw;
                        let mut row = *row0 as f32;
                        for (sh, color) in segs {
                            if let Some(rgb) = color {
                                fb.fill(left, g.top(row), g.cw, g.span(row, *sh as f32), *rgb);
                            }
                            row += *sh as f32;
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
                    if let Some(rgb) = color {
                        let asp = (*aspect).max(1) as usize;
                        let cell_w = (asp as f32) * g.cw;
                        let wmod = (*w).max(1) as usize;
                        for (bi, &byte) in bytes.iter().enumerate() {
                            let mut b = byte;
                            let base = bi * 8;
                            while b != 0 {
                                let bit = base + b.trailing_zeros() as usize;
                                let x = (bit % wmod) as f32;
                                let y = (bit / wmod) as f32;
                                fb.fill(
                                    g.ox + (*col0 as f32 + x * asp as f32) * g.cw,
                                    g.top(*row0 as f32 + y),
                                    cell_w,
                                    g.span(*row0 as f32 + y, 1.0),
                                    *rgb,
                                );
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
                    let cell_w = (asp as f32) * g.cw;
                    let wmod = (*w).max(1) as usize;
                    for (bi, &byte) in bytes.iter().enumerate() {
                        let mut b = byte;
                        let base = bi * 8;
                        while b != 0 {
                            let bit = base + b.trailing_zeros() as usize;
                            let rgb = colors.get(&(bit as u64)).copied().unwrap_or(*default);
                            let x = (bit % wmod) as f32;
                            let y = (bit / wmod) as f32;
                            fb.fill(
                                g.ox + (*col0 as f32 + x * asp as f32) * g.cw,
                                g.top(*row0 as f32 + y),
                                cell_w,
                                g.span(*row0 as f32 + y, 1.0),
                                rgb,
                            );
                            b &= b - 1;
                        }
                    }
                }
                // A picture in a cell block (ADR-686), the CPU `paint_image`: the block on the
                // grid, the picture placed in it and drawn texel for pixel from a texture of
                // exactly its drawn size, clipped to the block.
                Op::Image {
                    row,
                    col,
                    w,
                    h,
                    picture,
                    fit,
                    anchor,
                } => {
                    let left = g.ox + *col as f32 * g.cw;
                    let top = g.top(*row as f32);
                    let box_w = (*w as f32 * g.cw).max(0.0);
                    let box_h = g.span(*row as f32, *h as f32).max(0.0);
                    let Some((dx, dy, drawn_w, drawn_h)) = crate::host::picture::place(
                        *fit,
                        *anchor,
                        picture.0.width(),
                        picture.0.height(),
                        box_w as u32,
                        box_h as u32,
                    ) else {
                        continue;
                    };
                    let Some(id) = self.picture_texture(renderer, &picture.0, drawn_w, drawn_h)
                    else {
                        continue;
                    };
                    let saved_clip = fb.clip;
                    fb.clip[0] = fb.clip[0].max(left);
                    fb.clip[1] = fb.clip[1].max(top).max(g.rows.top as f32);
                    fb.clip[2] = fb.clip[2].min(left + box_w);
                    fb.clip[3] = fb.clip[3].min(top + box_h);
                    if fb.clip[2] > fb.clip[0] && fb.clip[3] > fb.clip[1] {
                        fb.textured(
                            TexKey::Picture(id),
                            left + dx as f32,
                            top + dy as f32,
                            drawn_w as f32,
                            drawn_h as f32,
                            [0.0, 0.0, 1.0, 1.0],
                            [255, 255, 255, 255],
                            0,
                            0.0,
                            0.0,
                        );
                    }
                    fb.clip = saved_clip;
                }
                // Pixel space: window coordinates as given, untouched by grid or scroll.
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
                    if self.textures.contains_key(tex) {
                        fb.textured(TexKey::User(*tex), *x, *y, *w, *h, *uv, *tint, 0, *rot, 0.0);
                    }
                }
                Op::Quad {
                    x,
                    y,
                    w,
                    h,
                    color,
                    rot,
                } => {
                    fb.solid(*x, *y, *w, *h, *color, *rot, Edge::Plain);
                }
                // Pixel-space text: the `Text` walk from a pixel top-left (after the
                // alignment's shift by the run's shaped width), untouched by grid or scroll.
                Op::TextPx {
                    x,
                    y,
                    s,
                    face,
                    align,
                } => {
                    let align = match align {
                        Align::Left => 0,
                        Align::Center => 1,
                        Align::Right => 2,
                    };
                    self.draw_run(fb, renderer, s, face, RunPlace::Pixel { align }, g, *x, *y);
                }
            }
        }
    }
}
