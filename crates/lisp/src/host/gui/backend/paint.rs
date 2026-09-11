//! Painting a frame: turn a window's render ops into pixels, with damage tracking so a
//! quiet frame blits only the rows that changed. Runs on the GUI thread, where a panic
//! takes the event loop with it — so `render_ops` is fuzzed against wild inputs here.

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
        std::env::var("BROOD_STALL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

/// A half-open damage rectangle in physical pixels: `[x0,x1) × [y0,y1)`.
#[derive(Clone, Copy)]
pub(super) struct DamageRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl DamageRect {
    const EMPTY: DamageRect = DamageRect {
        x0: 0,
        y0: 0,
        x1: 0,
        y1: 0,
    };
    fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    fn union(self, o: DamageRect) -> DamageRect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        DamageRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// The changed bounding box between `new` and `old` (both `w*h`), or `EMPTY`
/// when identical. Rows compare as slices first (a fast memcmp), so the
/// unchanged rows — the common case when only a line or two changed — cost one
/// comparison each.
pub(super) fn damage_bbox(new: &[u32], old: &[u32], w: usize, h: usize) -> DamageRect {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        let base = y * w;
        let nr = &new[base..base + w];
        let or = &old[base..base + w];
        if nr == or {
            continue;
        }
        let mut fx = 0;
        while nr[fx] == or[fx] {
            fx += 1;
        }
        let mut lx = w - 1;
        while nr[lx] == or[lx] {
            lx -= 1;
        }
        x0 = x0.min(fx);
        x1 = x1.max(lx + 1);
        if y0 == usize::MAX {
            y0 = y;
        }
        y1 = y + 1;
    }
    if y0 == usize::MAX {
        DamageRect::EMPTY
    } else {
        DamageRect { x0, y0, x1, y1 }
    }
}

/// Damage-only present is **on by default**; `BROOD_GUI_DAMAGE=0` opts back to
/// a full-buffer blit every frame (the safe fallback) — a one-line escape hatch
/// if a backend ever mishandles damage. Read once.
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

/// Render `ops` into `buf` with the given `scroll_dy` pixel shift (positive = content
/// shifted upward). Called recursively for `ScrollRegion` — each region overrides the
/// parent's `scroll_dy` with its own, then automatically restores on return.
pub(super) fn render_ops(
    ops: &[Op],
    buf: &mut [u32],
    fb_w: usize,
    fb_h: usize,
    r: &mut Renderer,
    ox: usize,
    oy: usize,
    cw: usize,
    ch: usize,
    bg0: u32,
    scroll_dy: isize,
) {
    for op in ops {
        match op {
            Op::Clear => {
                for p in buf.iter_mut() {
                    *p = bg0;
                }
            }
            Op::ScrollRegion { dy_frac, ops } => {
                let inner_dy = scroll_px(*dy_frac, ch);
                render_ops(ops, buf, fb_w, fb_h, r, ox, oy, cw, ch, bg0, inner_dy);
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
                        fill_cell(
                            buf, fb_w, fb_h, left, render_top, block_w, visible_h, bg_packed,
                        );
                    }
                    r.draw_cluster(
                        buf,
                        fb_w,
                        fb_h,
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
                            fill_cell(
                                buf,
                                fb_w,
                                fb_h,
                                left,
                                uy_signed as usize,
                                block_w,
                                scale,
                                pack(fg),
                            );
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
                                buf,
                                fb_w,
                                fb_h,
                                (ox + *col as usize * cw) as f32,
                                render_top as f32,
                                (*w as usize * cw) as f32,
                                visible_h as f32,
                                *radius * cw as f32,
                                bg,
                                1.0,
                            );
                        } else {
                            fill_cell(
                                buf,
                                fb_w,
                                fb_h,
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
                        buf,
                        fb_w,
                        fb_h,
                        ox as f32 + *x * cw as f32,
                        oy as f32 + *y * ch as f32,
                        *w * cw as f32,
                        *h * ch as f32,
                        *radius * cw as f32,
                        bg,
                        *opacity,
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
                        buf,
                        fb_w,
                        fb_h,
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
                            fill_cell(buf, fb_w, fb_h, left, y, cw, span_h, pack(*rgb));
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
                            fill_cell(buf, fb_w, fb_h, left, top, cell_w, ch, packed);
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
                        fill_cell(buf, fb_w, fb_h, left, top, cell_w, ch, pack(rgb));
                        b &= b - 1;
                    }
                }
            }
        }
    }
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
    let ts_a = Instant::now();
    let sz = window.inner_size();
    let (w, h) = (sz.width.max(1), sz.height.max(1));
    let ts_isz = Instant::now();
    if surface
        .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
        .is_err()
    {
        return;
    }
    let ts_resize = Instant::now();
    let mut buf = match surface.buffer_mut() {
        Ok(b) => b,
        Err(_) => return,
    };
    // Phase timer (BROOD_STALL_MS): `setup` = inner_size + resize + buffer_mut; the
    // tail splits those three, plus `body` (clear + op loop) vs `present` (blit).
    let t_setup = Instant::now();
    let (fb_w, fb_h) = (w as usize, h as usize);
    let bg0 = pack(r.bg());
    // Coordinate contract: `r.cell_w`/`cell_h` are PHYSICAL (post-scale) pixels;
    // `Op` row/col are BASE cells (top-left pixel = col*cell_w, row*cell_h); a
    // face `:scale n` multiplies into that same physical grid (n×n base cells).
    //
    // The conventional frame opens with a full `:clear`, which already paints the
    // whole buffer with `bg0` — so skip the unconditional pre-clear in that case
    // to avoid a redundant full-buffer write every frame. We still pre-clear when
    // the frame does NOT start with a full clear, so the background is clean.
    if !matches!(frame.first(), Some(Op::Clear)) {
        for p in buf.iter_mut() {
            *p = bg0;
        }
    }
    let (cw, ch) = (r.cell_w, r.cell_h);
    // The grid's origin: the inset plus the sub-cell remainder placement (centred
    // horizontally, anchored at the top vertically — see `grid_origin`), so the
    // leftover pixels read as headroom up top and the bottom row sits flush rather
    // than on a lopsided margin. `ox`/`oy` are the per-axis offsets every op's pixel base
    // adds; `px_to_cell` shares them so painted and hit-tested grids coincide. The
    // `clear`/pre-clear already filled the surrounding margin with the background.
    let (ox, oy) = r.grid_origin(fb_w, fb_h);
    render_ops(frame, &mut buf, fb_w, fb_h, r, ox, oy, cw, ch, bg0, 0);
    let t_body = Instant::now();
    // Present. Whenever we can't be *certain* a narrower damage is safe we blit
    // the whole buffer — no corruption risk. By default we declare only the
    // changed region (BROOD_GUI_DAMAGE=0 forces the full blit): the buffer we
    // got holds content from `buf.age()` frames ago, so
    // the damage we must declare is the union of the last `age` frames' changes
    // (computed by diffing against the last presented pixels). A resize resets
    // the history and full-presents. See `damage_bbox`/`DamageRect`.
    if !gui_damage_enabled() {
        let _ = buf.present();
    } else {
        let n = fb_w * fb_h;
        if r.prev_pixels.len() != n {
            // First frame or a resize: reset history, present the whole buffer.
            r.prev_pixels.resize(n, 0);
            r.prev_pixels.copy_from_slice(&buf);
            r.damage_ring.clear();
            let _ = buf.present();
        } else {
            let age = buf.age() as usize;
            let this = damage_bbox(&buf, &r.prev_pixels, fb_w, fb_h);
            // Save pixels + record damage BEFORE present (present consumes buf).
            r.prev_pixels.copy_from_slice(&buf);
            r.damage_ring.push(this);
            if r.damage_ring.len() > DAMAGE_HISTORY {
                let drop = r.damage_ring.len() - DAMAGE_HISTORY;
                r.damage_ring.drain(0..drop);
            }
            // Safe to narrow only if the buffer's age is known (≠0) and within
            // our recorded history; else full-present.
            let acc = if age != 0 && age <= r.damage_ring.len() {
                r.damage_ring
                    .iter()
                    .rev()
                    .take(age)
                    .fold(DamageRect::EMPTY, |a, d| a.union(*d))
            } else {
                DamageRect {
                    x0: 0,
                    y0: 0,
                    x1: fb_w,
                    y1: fb_h,
                }
            };
            if acc.is_empty() {
                // Nothing changed across the relevant frames (zero-area damage
                // is invalid) — a cheap full present.
                let _ = buf.present();
            } else {
                let rect = softbuffer::Rect {
                    x: acc.x0 as u32,
                    y: acc.y0 as u32,
                    width: NonZeroU32::new((acc.x1 - acc.x0) as u32).unwrap(),
                    height: NonZeroU32::new((acc.y1 - acc.y0) as u32).unwrap(),
                };
                let _ = buf.present_with_damage(&[rect]);
            }
        }
    }
    // Slow-paint breakdown: when this paint took >= BROOD_STALL_MS, attribute the
    // time across phases. `present` dominating = the softbuffer→window blit (a
    // platform/software-render cost — candidate for damage-only present); `body`
    // dominating with build~0 = the full-buffer clear + rect/blit fills.
    if let Some((ms, t0)) = paint_t0 {
        let el = t0.elapsed().as_millis();
        if el >= ms {
            let clusters = PAINT_CLUSTERS.load(Ordering::Relaxed);
            let misses = PAINT_MISSES.load(Ordering::Relaxed);
            let build_ms = PAINT_BUILD_NS.load(Ordering::Relaxed) / 1_000_000;
            let setup_ms = t_setup.duration_since(t0).as_millis();
            let inner_ms = ts_isz.duration_since(ts_a).as_millis();
            let resize_ms = ts_resize.duration_since(ts_isz).as_millis();
            let bufmut_ms = t_setup.duration_since(ts_resize).as_millis();
            let body_ms = t_body.duration_since(t_setup).as_millis();
            let present_ms = t_body.elapsed().as_millis();
            eprintln!(
                "[gui-paint] {el}ms: fb={fb_w}x{fb_h} ops={op_count} \
                 clusters={clusters} misses={misses} build={build_ms}ms | \
                 setup={setup_ms}ms (inner={inner_ms} resize={resize_ms} bufmut={bufmut_ms}) \
                 body={body_ms}ms present={present_ms}ms"
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
        render_ops(ops, &mut buf, fb_w, fb_h, &mut r, 2, 2, cw, ch, 0, 0);
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
