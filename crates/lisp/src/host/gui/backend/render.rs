//! The text renderer: a shared `cosmic-text` engine, the glyph/cluster cache, and the
//! software rasteriser that paints one cell (or one cluster, or a rounded rect, or the
//! cursor) into the frame buffer.

use super::*;

use swash::scale::ScaleContext;

// ---- rasterising the cell grid ------------------------------------------

/// A rasterised grapheme cluster, baked into a small RGBA canvas sized to its
/// cell span (`width`×`height` px, the cluster's `string/display-width` cells wide). For
/// a `color` cluster (emoji) the RGBA is the glyph's own colors; for a monochrome
/// cluster the RGB is white and only the alpha carries coverage, so the caller
/// recolors it with the face `fg` at blit time (syntax colors vary per op). A
/// `subpixel` cluster is monochrome too, but its R/G/B bytes are the coverage of each
/// colour channel's third of the pixel (LCD text) — recoloured per channel at blit time.
pub(crate) struct CachedGlyph {
    pub(crate) color: bool,
    pub(crate) subpixel: bool,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rgba: Vec<u8>, // width*height*4, straight (non-premultiplied) alpha
}

/// How monochrome text is anti-aliased (behind `gui-text-aa!`). `Gray` is one coverage
/// value per pixel; `Subpixel` renders three — one per colour channel, each shifted a
/// third of a pixel — tripling the horizontal resolution of every stem on an LCD panel
/// whose subpixels run R-G-B left to right (`Bgr` for the other order). `Auto` (the
/// default) is subpixel at a 1× scale, where text has the fewest pixels to spend and the
/// gain is plainest, and gray on HiDPI, where grayscale is already sharp and the
/// compositor may scale or rotate the surface (which turns subpixel fringes into colour
/// noise).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextAa {
    Auto,
    Gray,
    Subpixel,
    Bgr,
}

/// The grapheme-cluster part of a glyph-cache key. The vast majority of probes
/// are single chars (one per cell, one repaint per keystroke), so they key on a
/// `Char` and allocate nothing; only the rare multi-char cluster (ZWJ emoji,
/// flag, accented base+mark) takes the `Str` path and allocates a `Box<str>`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum ClusterKey {
    Char(char),
    Str(Box<str>),
}

impl ClusterKey {
    /// The cheapest key for a cluster: a lone char allocates nothing.
    fn of(g: &str) -> ClusterKey {
        let mut it = g.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => ClusterKey::Char(c),
            _ => ClusterKey::Str(g.into()),
        }
    }
}

/// The raster target: a `w`×`h` framebuffer plus the half-open pixel-row band
/// `[y0, y1)` the primitives may touch. Pixels outside the band are left exactly as
/// they were — that is what lets a paint re-rasterise only the cell rows whose ops
/// changed (`paint::strip_diff`) and trust the rest of the retained canvas.
pub(super) struct Canvas<'a> {
    pub(super) buf: &'a mut [u32],
    pub(super) w: usize,
    pub(super) h: usize,
    pub(super) y0: usize,
    pub(super) y1: usize,
}

impl<'a> Canvas<'a> {
    /// The whole framebuffer (the fuzz tests' target; a paint always goes by band).
    #[cfg(test)]
    pub(super) fn full(buf: &'a mut [u32], w: usize, h: usize) -> Self {
        Canvas {
            buf,
            w,
            h,
            y0: 0,
            y1: h,
        }
    }

    /// The framebuffer with painting confined to rows `[y0, y1)` (clamped to it).
    pub(super) fn band(buf: &'a mut [u32], w: usize, h: usize, y0: usize, y1: usize) -> Self {
        Canvas {
            buf,
            w,
            h,
            y0: y0.min(h),
            y1: y1.min(h),
        }
    }

    /// The rows of `[top, top + h)` that fall inside the band — the loop bounds every
    /// primitive clips to.
    #[inline]
    pub(super) fn rows(&self, top: usize, h: usize) -> std::ops::Range<usize> {
        let lo = top.max(self.y0);
        let hi = top.saturating_add(h).min(self.y1);
        lo..hi.max(lo)
    }

    /// Fill rows `[top, top + h)` of the band with `color` — the clear.
    pub(super) fn fill_rows(&mut self, top: usize, h: usize, color: u32) {
        let w = self.w;
        for y in self.rows(top, h) {
            self.buf[y * w..(y + 1) * w].fill(color);
        }
    }
}

/// The shared text engine on the single GUI thread: cosmic-text's `FontSystem`
/// (font database + shaping + fallback) and `SwashCache` (glyph rasterisation,
/// color and mono), plus the family-keyword → family-name map a `:family` resolves
/// through. Shared by every window's renderer (so `gui-font-register` reaches them
/// all), like the old family registry.
pub(super) struct FontShared {
    fs: FontSystem,
    swash: SwashCache,
    /// swash's own scaler, for the subpixel (LCD) raster path: cosmic-text's
    /// `SwashCache` renders alpha masks only, so a per-channel mask is rendered here
    /// from the same font + glyph id + hinting.
    scaler: ScaleContext,
    /// interned family keyword id → fontdb family name (`:mono` → "DejaVu Sans Mono").
    names: HashMap<u32, String>,
}

impl FontShared {
    pub(super) fn new() -> Self {
        let src = |b: &'static [u8]| fontdb::Source::Binary(Rc2::new(b));
        // Load the bundled mono faces + the emoji fallback; no system fonts, so the
        // editor renders identically everywhere (self-contained, ADR-046).
        let fs = FontSystem::new_with_fonts([
            src(FONT_REGULAR),
            src(FONT_BOLD),
            src(FONT_ITALIC),
            src(FONT_BOLD_ITALIC),
            src(FONT_EMOJI),
        ]);
        let mut names = HashMap::new();
        names.insert(value::intern(DEFAULT_FAMILY), MONO_FAMILY.to_string());
        FontShared {
            fs,
            swash: SwashCache::new(),
            scaler: ScaleContext::new(),
            names,
        }
    }

    /// The fontdb family name for keyword id `id` (the bundled mono if unknown).
    fn name_of(&self, id: u32) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| MONO_FAMILY.to_string())
    }

    /// Register a family from raw TTF bytes per style (behind `gui-font-register`):
    /// load all four faces into the db, and map `id` to the regular face's family
    /// name so an `Attrs` built for it picks the right faces (weight/style matched).
    pub(super) fn register(
        &mut self,
        id: u32,
        regular: Vec<u8>,
        bold: Vec<u8>,
        italic: Vec<u8>,
        bold_italic: Vec<u8>,
    ) {
        let ids = self
            .fs
            .db_mut()
            .load_font_source(fontdb::Source::Binary(Rc2::new(regular)));
        let fam = ids
            .first()
            .and_then(|fid| self.fs.db().face(*fid))
            .and_then(|f| f.families.first().map(|(n, _)| n.clone()));
        for b in [bold, italic, bold_italic] {
            self.fs.db_mut().load_font_data(b);
        }
        if let Some(fam) = fam {
            self.names.insert(id, fam);
        }
    }
}

// fontdb's `Source::Binary` wants an `Arc<dyn AsRef<[u8]> + Send + Sync>`; alias it
// so the bundled `&'static [u8]` and the registered `Vec<u8>` both drop straight in.
use std::sync::Arc as Rc2;

/// Build the shared text engine seeded with the bundled `:mono` family + emoji.
pub(super) fn default_families() -> Families {
    Rc::new(RefCell::new(FontShared::new()))
}

pub(crate) struct Renderer {
    families: Families,
    default_family: u32,
    base_px: f32,
    scale: f64,
    px: f32,
    pub(crate) cell_w: usize,
    pub(crate) cell_h: usize,
    baseline: i32,       // pixels from a cell's top to the text baseline
    base_inset: f32,     // logical-px content margin before the grid (ADR-079); 0 = flush
    bg: Option<[u8; 3]>, // window background (clear/inset-margin fill); None = DEFAULT_BG
    line_height: f32,    // cell height as a multiple of the font px (`gui-line-height!`)
    text_aa: TextAa,     // how monochrome text is anti-aliased (`gui-text-aa!`)

    // keyed by (cluster, family id, bold, italic, scale, subpixel): the same cluster at
    // a different family/style/scale/AA rasterises to a different baked canvas.
    pub(super) cache: HashMap<(ClusterKey, u32, bool, bool, u16, bool), CachedGlyph>,

    // The retained frame (see `paint`): `canvas` holds the pixels of the last frame
    // rasterised, `prev_ops` the ops that produced it. A new frame is diffed against
    // `prev_ops` per cell row and only the rows that changed are re-rasterised into
    // `canvas`; `damage_ring` is the per-frame list of changed pixel bands of recent
    // frames (oldest→newest), so a present can cover the last `buffer.age()` frames.
    pub(super) canvas: Vec<u32>,
    /// `canvas`'s `(width, height)` — compared as a pair, not a length: a rotation of
    /// the window (800×600 → 600×800) keeps the pixel count and changes every row.
    pub(super) canvas_size: (usize, usize),
    pub(super) prev_ops: Vec<Op>,
    pub(super) damage_ring: Vec<Vec<DamageRect>>,
}

impl Renderer {
    pub(super) fn new(scale: f64, families: Families, base_px: f32) -> Self {
        let mut r = Renderer {
            families,
            default_family: value::intern(DEFAULT_FAMILY),
            base_px,
            scale,
            px: base_px,
            cell_w: 1,
            cell_h: 1,
            baseline: 0,
            base_inset: 0.0,
            bg: None,
            line_height: LINE_HEIGHT,
            text_aa: TextAa::Auto,
            cache: HashMap::new(),
            canvas: Vec::new(),
            canvas_size: (0, 0),
            prev_ops: Vec::new(),
            damage_ring: Vec::new(),
        };
        r.recompute();
        r
    }

    /// The content inset in PHYSICAL pixels (the logical `base_inset` × HiDPI
    /// scale) — the margin painted before the cell grid on every edge, and the
    /// offset every pixel↔cell conversion shares (`update_cells`, `px_to_cell`,
    /// `paint`) so the grid the renderer draws and the one the mouse hit-tests stay
    /// the same. 0 leaves the grid flush to the window edge (the default).
    pub(crate) fn inset(&self) -> usize {
        (self.base_inset * self.scale as f32).round().max(0.0) as usize
    }

    /// Build (and cache) a cluster's rasterised RGBA glyph and hand back a reference —
    /// the GPU backend uploads it to a texture atlas. Same key/build/cache path as
    /// `draw_cluster`, minus the CPU blit.
    // The `gui-gpu` glyph-atlas upload path calls this (`gui/gpu.rs`); it's unused in a
    // CPU-only `gui` build, so suppress dead-code only when `gui-gpu` is off.
    #[cfg_attr(not(feature = "gui-gpu"), allow(dead_code))]
    pub(crate) fn cluster_glyph(
        &mut self,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
    ) -> &CachedGlyph {
        let fid = family.unwrap_or(self.default_family);
        // Always an alpha mask: the GPU shader recolours one coverage per pixel.
        let key = (ClusterKey::of(g), fid, bold, italic, scale.max(1), false);
        if !self.cache.contains_key(&key) {
            let baked = self.build_cluster(g, fid, bold, italic, scale, false);
            self.cache.insert(key.clone(), baked);
        }
        &self.cache[&key]
    }

    /// Set the content inset (logical px). The cell metrics don't change — only the
    /// grid's origin and how many cells fit — so the caller recomputes the grid
    /// (`update_cells`) and repaints; no glyph-cache drop (unlike `set_font`).
    pub(super) fn set_inset(&mut self, px: f32) {
        self.base_inset = px.max(0.0);
        self.invalidate();
    }

    /// The window background colour — the fill for `Op::Clear`, the pre-clear, and
    /// (since it's outside every cell) the inset margin + the snap remainder. `None`
    /// falls back to `DEFAULT_BG`. Behind `gui-bg!`, so an app's padding matches its
    /// theme instead of showing the hardcoded default.
    pub(crate) fn bg(&self) -> [u8; 3] {
        self.bg.unwrap_or(DEFAULT_BG)
    }

    /// Set the window background (clear/inset-margin fill). No metric change — only a
    /// repaint — so the caller just requests a redraw.
    pub(super) fn set_bg(&mut self, rgb: Option<[u8; 3]>) {
        self.bg = rgb;
        self.invalidate();
    }

    /// The grid's top-left origin in PHYSICAL px, beyond the inset. `cols`/`rows`
    /// are floor divisions, so an arbitrary window leaves up to one cell of sub-cell
    /// remainder per axis that doesn't fill a whole cell. The two axes place it
    /// differently:
    ///   - **horizontal**: HALF the remainder, so the left/right margins stay
    ///     symmetric (the grid is centred between them).
    ///   - **vertical**: the FULL remainder above the grid — anchoring the leftover
    ///     at the *top* pushes the grid down so its bottom row (the editor's mode
    ///     line / status bar) sits flush against the window's bottom edge (modulo the
    ///     inset), instead of floating on half a cell. The slack reads as headroom up
    ///     top, where the eye expects it.
    /// WM-independent (no window resize, so it works where `request_inner_size` is
    /// ignored), and the mouse hit-test (`px_to_cell`) shares it so clicks stay
    /// aligned with what's painted.
    pub(crate) fn grid_origin(&self, w_px: usize, h_px: usize) -> (usize, usize) {
        let inset = self.inset();
        let (cw, ch) = (self.cell_w.max(1), self.cell_h.max(1));
        let rem_w = w_px.saturating_sub(2 * inset) % cw;
        let rem_h = h_px.saturating_sub(2 * inset) % ch;
        (inset + rem_w / 2, inset + rem_h)
    }

    /// Recompute the px size + cell metrics by shaping a reference glyph ('M') in
    /// the default family at the current size × HiDPI scale, dropping the cluster
    /// cache (baked at the old px). The grid stays uniform — a per-face
    /// `:family`/`:italic` only changes glyphs within the fixed cell.
    fn recompute(&mut self) {
        // Whole pixels per em: the rasteriser hints outlines to the pixel grid, and
        // hinting at a fractional ppem (15 px × a 1.25 HiDPI scale = 18.75) leaves stems
        // straddling pixels — soft, uneven text. Rounding keeps every glyph on the grid
        // the cell metrics already round to; the ≤0.5 px size error is invisible.
        self.px = (self.base_px * self.scale as f32).round().max(1.0);
        self.cache.clear();
        self.invalidate();
        let line_h = (self.px * self.line_height).round().max(1.0);
        self.cell_h = line_h as usize;
        // `name_of` returns owned data, so the immutable borrow ends on this
        // line — letting the `borrow_mut` below succeed (don't make it borrow).
        let fam = self.families.borrow().name_of(self.default_family);
        let mut shared = self.families.borrow_mut();
        let shared = &mut *shared;
        let metrics = Metrics::new(self.px, line_h);
        let mut tb = CtBuffer::new(&mut shared.fs, metrics);
        tb.set_size(Some(line_h * 4.0), Some(line_h * 2.0));
        let attrs = Attrs::new().family(Family::Name(fam.as_str()));
        tb.set_text("M", &attrs, Shaping::Advanced, None);
        tb.shape_until_scroll(&mut shared.fs, false);
        let (mut cw, mut base) = (self.px, self.px);
        if let Some(run) = tb.layout_runs().next() {
            base = run.line_y;
            if let Some(gl) = run.glyphs.first() {
                cw = gl.w;
            }
        }
        self.cell_w = cw.round().max(1.0) as usize;
        self.baseline = base.round() as i32;
    }

    /// Adjust for a new HiDPI scale factor (then recompute metrics).
    pub(super) fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.recompute();
    }

    /// The HiDPI scale factor (physical px per logical px).
    pub(crate) fn scale(&self) -> f64 {
        self.scale
    }

    /// Set the cell height as a multiple of the font px (behind `gui-line-height!`),
    /// then recompute the grid: the row count changes, so the caller re-derives
    /// `(cols, rows)` and re-renders — the same path as `set_font`.
    pub(super) fn set_line_height(&mut self, mult: f32) {
        self.line_height = if mult.is_finite() {
            mult.clamp(0.8, 3.0)
        } else {
            LINE_HEIGHT
        };
        self.recompute();
    }

    /// Set how monochrome text is anti-aliased (behind `gui-text-aa!`). Drops the
    /// cluster cache (baked in the old mode) and the retained frame, so the next paint
    /// re-rasterises everything.
    pub(super) fn set_text_aa(&mut self, mode: TextAa) {
        self.text_aa = mode;
        self.cache.clear();
        self.invalidate();
    }

    /// Whether text is rasterised per colour channel right now: the explicit mode, or
    /// under `Auto` only at a 1× scale (see `TextAa`).
    pub(super) fn subpixel_text(&self) -> bool {
        match self.text_aa {
            TextAa::Gray => false,
            TextAa::Subpixel | TextAa::Bgr => true,
            TextAa::Auto => (self.scale - 1.0).abs() < 1e-6,
        }
    }

    /// Forget the retained frame, so the next paint rasterises every row. For a
    /// metric change (font, inset, scale, line height) — `recompute` does it — and
    /// for anything else that changes how the same ops look (`set_bg`).
    pub(super) fn invalidate(&mut self) {
        self.prev_ops.clear();
    }

    /// Set the global default cell font — family and/or pixel size — then
    /// recompute the grid. The whole-window knob behind `gui-font!`.
    pub(super) fn set_font(&mut self, family: Option<u32>, px: Option<f32>) {
        if let Some(f) = family {
            self.default_family = f;
        }
        if let Some(p) = px {
            self.base_px = p.max(1.0);
        }
        self.recompute();
    }

    /// Shape + rasterise grapheme cluster `g` (cosmic-text + swash) into a small
    /// RGBA canvas sized to its cell span, cached by (cluster, family, style,
    /// scale). A color cluster (emoji) keeps its own colors; a monochrome cluster
    /// stores coverage in the alpha with white RGB, so the caller recolors it.
    fn build_cluster(
        &self,
        g: &str,
        fid: u32,
        bold: bool,
        italic: bool,
        scale: u16,
        subpixel: bool,
    ) -> CachedGlyph {
        let scale = scale.max(1) as usize;
        let px = (self.px * scale as f32).max(1.0);
        let cells = cluster_cells(g).max(1);
        let cw = (self.cell_w * scale * cells).max(1);
        let ch = (self.cell_h * scale).max(1);
        let line_h = ch as f32;
        // The grid baseline (from the mono 'M', not this cluster's own line) so a
        // fallback glyph aligns with the surrounding text rather than floating to
        // wherever its own font's line metrics put it.
        let baseline = (self.baseline * scale as i32).max(0);
        // `name_of` returns owned data, so the immutable borrow ends on this
        // line — letting the `borrow_mut` below succeed (don't make it borrow).
        let fam = self.families.borrow().name_of(fid);
        let mut shared = self.families.borrow_mut();
        let shared = &mut *shared;
        let attrs = |()| {
            let mut a = Attrs::new().family(Family::Name(fam.as_str()));
            if bold {
                a = a.weight(Weight::BOLD);
            }
            if italic {
                a = a.style(Style::Italic);
            }
            a
        };
        // Shape once at the text size to see whether the cluster fell back to a
        // *color* font (an emoji). Text/symbol glyphs are mono.
        let tb = shape_cluster(shared, g, attrs(()), px, cw as f32, line_h);
        let mut rgba = vec![0u8; cw * ch * 4];
        let (color, subpixel) = if first_glyph_is_color(shared, &tb) {
            // Emoji: render big enough to fill the cell block and center it — color
            // glyphs have no useful text baseline, so baseline-aligning them looks
            // low and cramped. Size to the smaller block dimension so the (square)
            // glyph fits its `cells`-wide span.
            let epx = ch.min(cw) as f32;
            let tb2 = shape_cluster(shared, g, attrs(()), epx, cw as f32, epx);
            composite_cluster(shared, &tb2, &mut rgba, cw, ch, Placement::Center);
            (true, false)
        } else if subpixel
            && composite_cluster_subpixel(
                shared,
                &tb,
                &mut rgba,
                cw,
                ch,
                baseline,
                self.text_aa == TextAa::Bgr,
            )
        {
            (false, true)
        } else {
            // Gray AA — also the fallback when a glyph has no outline to render per
            // channel (a bitmap-only font), so the cluster still appears.
            rgba.fill(0);
            composite_cluster(
                shared,
                &tb,
                &mut rgba,
                cw,
                ch,
                Placement::Baseline(baseline),
            );
            (false, false)
        };
        CachedGlyph {
            color,
            subpixel,
            width: cw,
            height: ch,
            rgba,
        }
    }

    /// Blit grapheme cluster `g` into the framebuffer at cell-pixel `(left, top)`,
    /// alpha-compositing over the cell background. A color cluster (emoji) draws in
    /// its own colors; a monochrome one is recolored with the face `fg`. The cluster
    /// occupies `string/display-width` cells (the caller advances the cursor to match).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_cluster(
        &mut self,
        canvas: &mut Canvas,
        left: usize,
        top: usize,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
        fg: [u8; 3],
        clip_skip: usize,
    ) {
        if g == " " {
            return;
        }
        let fid = family.unwrap_or(self.default_family);
        let subpixel = self.subpixel_text();
        // The common single-char cluster keys via `ClusterKey::Char` with no
        // allocation; only a rare multi-char cluster allocates (a `Box<str>`).
        let key = (ClusterKey::of(g), fid, bold, italic, scale.max(1), subpixel);
        PAINT_CLUSTERS.fetch_add(1, Ordering::Relaxed);
        if !self.cache.contains_key(&key) {
            let t0 = Instant::now();
            let baked = self.build_cluster(g, fid, bold, italic, scale, subpixel);
            PAINT_BUILD_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            PAINT_MISSES.fetch_add(1, Ordering::Relaxed);
            self.cache.insert(key.clone(), baked);
        }
        let cg = &self.cache[&key];
        // `clip_skip` glyph rows from the top are above the clip boundary (the grid
        // origin); they're skipped and the remaining rows land at `top` onward. The
        // canvas band clips the rest: rows outside `[y0, y1)` are not touched.
        let (fb_w, y0, y1) = (canvas.w, canvas.y0, canvas.y1);
        for ry in clip_skip..cg.height {
            let py = top + (ry - clip_skip);
            if py >= y1 {
                break;
            }
            if py < y0 {
                continue;
            }
            let row = py * fb_w;
            for rx in 0..cg.width {
                let pxx = left + rx;
                if pxx >= fb_w {
                    break;
                }
                let i = (ry * cg.width + rx) * 4;
                let a = cg.rgba[i + 3];
                if a == 0 {
                    continue;
                }
                let dst = &mut canvas.buf[row + pxx];
                *dst = if cg.color {
                    blend(*dst, [cg.rgba[i], cg.rgba[i + 1], cg.rgba[i + 2]], a)
                } else if cg.subpixel {
                    blend_rgb(*dst, fg, [cg.rgba[i], cg.rgba[i + 1], cg.rgba[i + 2]])
                } else {
                    blend(*dst, fg, a)
                };
            }
        }
    }
}

/// Source-over composite a straight-alpha pixel into the RGBA cluster canvas at
/// `(x, y)` (a no-op off-canvas / at zero alpha). Used to bake a shaped cluster's
/// glyphs into one canvas before it's cached.
#[allow(clippy::too_many_arguments)]
pub(super) fn canvas_over(
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    x: i32,
    y: i32,
    sr: u8,
    sg: u8,
    sb: u8,
    sa: u8,
) {
    if sa == 0 || x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
        return;
    }
    let idx = (y as usize * cw + x as usize) * 4;
    let (sa, da) = (sa as u32, rgba[idx + 3] as u32);
    let out_a = sa + da * (255 - sa) / 255;
    if out_a == 0 {
        return;
    }
    let mix = |s: u8, d: u8| (((s as u32 * sa) + (d as u32 * da * (255 - sa) / 255)) / out_a) as u8;
    rgba[idx] = mix(sr, rgba[idx]);
    rgba[idx + 1] = mix(sg, rgba[idx + 1]);
    rgba[idx + 2] = mix(sb, rgba[idx + 2]);
    rgba[idx + 3] = out_a as u8;
}

/// Where a cluster's glyphs sit in its baked canvas. `Baseline(y)` puts the text
/// baseline at row `y` (the shared grid baseline, so a fallback symbol aligns with
/// the surrounding text). `Center` ignores the baseline and centers the glyph's
/// bounding box in the canvas — for color emoji, which have no useful text baseline.
pub(super) enum Placement {
    Baseline(i32),
    Center,
}

/// Shape grapheme cluster `g` into a fresh cosmic-text buffer at `px` / line height
/// `line_h`, in family/style `attrs`. The layout box is generous so a wide glyph
/// isn't wrapped or clipped during shaping.
pub(super) fn shape_cluster(
    shared: &mut FontShared,
    g: &str,
    attrs: Attrs,
    px: f32,
    w: f32,
    line_h: f32,
) -> CtBuffer {
    let mut tb = CtBuffer::new(&mut shared.fs, Metrics::new(px.max(1.0), line_h.max(1.0)));
    tb.set_size(Some(w + px), Some(line_h + px));
    tb.set_text(g, &attrs, Shaping::Advanced, None);
    tb.shape_until_scroll(&mut shared.fs, false);
    tb
}

/// True if the cluster's first rasterised glyph is a *color* (emoji) bitmap — the
/// signal to size + center it rather than baseline-align it as text.
pub(super) fn first_glyph_is_color(shared: &mut FontShared, tb: &CtBuffer) -> bool {
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            if let Some(img) = shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                return matches!(img.content, SwashContent::Color);
            }
        }
    }
    false
}

/// Composite a shaped cluster's glyphs into the RGBA canvas `rgba` (`cw`×`ch`).
/// `Baseline` lays each glyph at its pen position on the shared baseline (text);
/// `Center` puts the glyph's bounding box in the middle of the canvas (emoji).
/// Color glyphs keep their own RGBA; mask glyphs store coverage as white + alpha
/// (the caller recolors them with the face fg).
pub(super) fn composite_cluster(
    shared: &mut FontShared,
    tb: &CtBuffer,
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    place: Placement,
) {
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            let img = match shared.swash.get_image(&mut shared.fs, phys.cache_key) {
                Some(img) => img,
                None => continue,
            };
            let (iw, ih) = (img.placement.width as i32, img.placement.height as i32);
            let (ox, oy) = match place {
                Placement::Baseline(b) => {
                    (phys.x + img.placement.left, b + phys.y - img.placement.top)
                }
                Placement::Center => ((cw as i32 - iw) / 2, (ch as i32 - ih) / 2),
            };
            match img.content {
                SwashContent::Mask => {
                    for ry in 0..ih {
                        for rx in 0..iw {
                            let a = img.data[(ry * iw + rx) as usize];
                            canvas_over(rgba, cw, ch, ox + rx, oy + ry, 255, 255, 255, a);
                        }
                    }
                }
                SwashContent::Color => {
                    for ry in 0..ih {
                        for rx in 0..iw {
                            let i = ((ry * iw + rx) * 4) as usize;
                            canvas_over(
                                rgba,
                                cw,
                                ch,
                                ox + rx,
                                oy + ry,
                                img.data[i],
                                img.data[i + 1],
                                img.data[i + 2],
                                img.data[i + 3],
                            );
                        }
                    }
                }
                SwashContent::SubpixelMask => {}
            }
        }
    }
}

/// Composite a shaped cluster's glyphs into `rgba` as a **subpixel** mask: R/G/B carry
/// the coverage of each channel's third of the pixel (rendered by swash at 1/3-px
/// offsets, hinted like the gray path), A is the max of the three so the blit's
/// "anything here?" test still works. `bgr` flips the channel order for a panel whose
/// subpixels run blue-first. Baseline-placed like text. Returns false — leaving `rgba`
/// untouched — if no glyph had an outline to render this way (a bitmap-only face),
/// so the caller can fall back to the gray mask.
pub(super) fn composite_cluster_subpixel(
    shared: &mut FontShared,
    tb: &CtBuffer,
    rgba: &mut [u8],
    cw: usize,
    ch: usize,
    baseline: i32,
    bgr: bool,
) -> bool {
    use cosmic_text::CacheKeyFlags;
    use swash::scale::{Render, Source};
    use swash::zeno::{Angle, Format, Transform, Vector};
    let mut drew = false;
    for run in tb.layout_runs() {
        for gl in run.glyphs.iter() {
            let phys = gl.physical((0.0, 0.0), 1.0);
            let key = phys.cache_key;
            let Some(font) = shared.fs.get_font(key.font_id, key.font_weight) else {
                continue;
            };
            // The same scaler settings cosmic-text's own (alpha) rasteriser uses for
            // this cache key — hinting, a synthesised italic for a family without an
            // italic face, whole-pixel offsets for a pixel font — so the subpixel glyph
            // is the gray glyph with three coverages, not a differently shaped one.
            let flags = key.flags;
            let mut scaler = shared
                .scaler
                .builder(font.as_swash())
                .size(f32::from_bits(key.font_size_bits))
                .hint(!flags.contains(CacheKeyFlags::DISABLE_HINTING))
                .build();
            let format = if bgr {
                Format::subpixel_bgra()
            } else {
                Format::Subpixel
            };
            let offset = if flags.contains(CacheKeyFlags::PIXEL_FONT) {
                Vector::new(key.x_bin.as_float().round(), key.y_bin.as_float().round())
            } else {
                Vector::new(key.x_bin.as_float(), key.y_bin.as_float())
            };
            let transform = flags
                .contains(CacheKeyFlags::FAKE_ITALIC)
                .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)));
            let Some(img) = Render::new(&[Source::Outline])
                .format(format)
                .offset(offset)
                .transform(transform)
                .render(&mut scaler, key.glyph_id)
            else {
                continue;
            };
            if img.data.len() < (img.placement.width * img.placement.height * 4) as usize {
                continue;
            }
            let (iw, ih) = (img.placement.width as i32, img.placement.height as i32);
            let (ox, oy) = (
                phys.x + img.placement.left,
                baseline + phys.y - img.placement.top,
            );
            for ry in 0..ih {
                for rx in 0..iw {
                    let i = ((ry * iw + rx) * 4) as usize;
                    let (r, g, b) = (img.data[i], img.data[i + 1], img.data[i + 2]);
                    let a = r.max(g).max(b);
                    if a == 0 {
                        continue;
                    }
                    let (x, y) = (ox + rx, oy + ry);
                    if x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
                        continue;
                    }
                    let idx = (y as usize * cw + x as usize) * 4;
                    // Per-channel union with what's there (two glyphs of one cluster
                    // may overlap — a base + combining mark): the max coverage wins.
                    rgba[idx] = rgba[idx].max(r);
                    rgba[idx + 1] = rgba[idx + 1].max(g);
                    rgba[idx + 2] = rgba[idx + 2].max(b);
                    rgba[idx + 3] = rgba[idx + 3].max(a);
                    drew = true;
                }
            }
        }
    }
    drew
}

pub(super) fn pack(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

/// linear-light (0..=1) → sRGB byte — the exact inverse of [`SRGB_TO_LINEAR`].
#[inline]
pub(super) fn linear_to_srgb(c: f32) -> u32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u32
}

/// Alpha-composite `fg` over destination pixel `dst` with coverage `cov` (0..=255),
/// **in linear light** so anti-aliased edges are weighted correctly. The rasteriser's
/// coverage is linear, but a naive sRGB-space lerp under-weights partial-coverage
/// pixels — on a dark theme (light text on `#1e1e2e`) that makes stems look thin and
/// the text fuzzy. Decoding to linear, blending, then re-encoding keeps strokes full
/// and edges crisp. The glyph interior (`cov == 255`) and exterior (`cov == 0`) — the
/// bulk of the pixels — take an exact fast path with no float work, so only the thin
/// anti-aliased rim pays the gamma cost.
pub(super) fn blend(dst: u32, fg: [u8; 3], cov: u8) -> u32 {
    if cov == 0 {
        return dst;
    }
    if cov == 255 {
        return pack(fg);
    }
    let a = cov as f32 / 255.0;
    let lut = &*SRGB_TO_LINEAR;
    let dr = lut[((dst >> 16) & 0xff) as usize];
    let dg = lut[((dst >> 8) & 0xff) as usize];
    let db = lut[(dst & 0xff) as usize];
    let r = linear_to_srgb(lut[fg[0] as usize] * a + dr * (1.0 - a));
    let g = linear_to_srgb(lut[fg[1] as usize] * a + dg * (1.0 - a));
    let b = linear_to_srgb(lut[fg[2] as usize] * a + db * (1.0 - a));
    (r << 16) | (g << 8) | b
}

/// The subpixel sibling of `blend`: composite `fg` over `dst` with a separate coverage
/// per colour channel (`cov[0]` red, `[1]` green, `[2]` blue), in linear light. Each
/// channel is the same single-channel lerp `blend` does, so the two are visually
/// consistent where a gray and a subpixel glyph sit side by side.
pub(super) fn blend_rgb(dst: u32, fg: [u8; 3], cov: [u8; 3]) -> u32 {
    if cov == [0, 0, 0] {
        return dst;
    }
    if cov == [255, 255, 255] {
        return pack(fg);
    }
    let lut = &*SRGB_TO_LINEAR;
    let ch = |shift: u32, c: u8, f: u8| -> u32 {
        let d = lut[((dst >> shift) & 0xff) as usize];
        let a = c as f32 / 255.0;
        linear_to_srgb(lut[f as usize] * a + d * (1.0 - a))
    };
    (ch(16, cov[0], fg[0]) << 16) | (ch(8, cov[1], fg[1]) << 8) | ch(0, cov[2], fg[2])
}

pub(super) fn fill_cell(
    canvas: &mut Canvas,
    left: usize,
    top: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    let fb_w = canvas.w;
    let x1 = left.saturating_add(w).min(fb_w);
    if left >= x1 {
        return;
    }
    for y in canvas.rows(top, h) {
        let row = y * fb_w;
        canvas.buf[row + left..row + x1].fill(color);
    }
}

/// Fill a sub-pixel-positioned rounded rectangle with `color` at `opacity` (0..1),
/// anti-aliased. The shape is a signed-distance field: the distance from each pixel
/// centre to the rect's rounded core (the rect inset by `radius` on every side)
/// gives a 1px coverage ramp at the edge, so corners and fractional edges read
/// smooth instead of stair-stepped. `radius == 0` is a sharp rect (full coverage
/// inside, just the opacity blend). Blends over whatever's already in `buf`
/// (`blend`), so a faded overlay shows the content beneath it. A dimension thinner than a
/// logical pixel is a hairline and snaps to whole device pixels first (`snap_hairline`).
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_rrect(
    canvas: &mut Canvas,
    fx: f32,
    fy: f32,
    fw: f32,
    fh: f32,
    radius: f32,
    color: [u8; 3],
    opacity: f32,
    scale: f32,
) {
    if fw <= 0.0 || fh <= 0.0 || opacity <= 0.0 {
        return;
    }
    // A hairline — a dimension thinner than one LOGICAL pixel — snaps to a whole
    // number of device pixels (one per logical pixel, so 2 px at 2×) centred where it
    // was asked for. Otherwise a 0.45 px rule lands on 1 or 2 solid pixels depending on
    // which pixel boundary its fractional position straddles: uneven, and different
    // for every divider. Snapped, every hairline in a window is the same crisp line.
    let (fx, fw, fy, fh, radius) = snap_hairline(fx, fw, fy, fh, radius, scale);
    let (fb_w, fb_h) = (canvas.w, canvas.h);
    let base = opacity.clamp(0.0, 1.0) * 255.0;
    let r = radius.max(0.0).min(fw / 2.0).min(fh / 2.0);
    // The inner core the corners round around: the rect inset by `r` on each side.
    let (rx0, ry0) = (fx + r, fy + r);
    let (rx1, ry1) = (fx + fw - r, fy + fh - r);
    let x0 = fx.floor().max(0.0) as usize;
    let y0 = (fy.floor().max(0.0) as usize).max(canvas.y0);
    let x1 = ((fx + fw).ceil().max(0.0) as usize).min(fb_w);
    let y1 = ((fy + fh).ceil().max(0.0) as usize)
        .min(fb_h)
        .min(canvas.y1);
    for py in y0..y1 {
        let cy = py as f32 + 0.5;
        let row = py * fb_w;
        for px in x0..x1 {
            let cx = px as f32 + 0.5;
            let dx = (rx0 - cx).max(cx - rx1).max(0.0);
            let dy = (ry0 - cy).max(cy - ry1).max(0.0);
            // Inside the straight body dx==dy==0 → full coverage; in a corner the
            // euclidean distance past the core gives the rounded falloff.
            let cov_f = if r <= 0.0 {
                1.0
            } else {
                (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
            };
            if cov_f <= 0.0 {
                continue;
            }
            let cov = (base * cov_f) as u32;
            if cov == 0 {
                continue;
            }
            let idx = row + px;
            canvas.buf[idx] = blend(canvas.buf[idx], color, cov.min(255) as u8);
        }
    }
}

/// The hairline rule behind `fill_rrect`: a rect dimension under one logical pixel
/// (`scale` device px) becomes exactly `max(1, round(scale))` device pixels, placed so
/// the requested centre is inside them; the snapped line is square-cornered (a
/// radius on a 1 px line only fades it). Both axes are checked independently, so a
/// thin vertical and a thin horizontal rule both snap. Returns the (possibly)
/// adjusted `(x, w, y, h, radius)`; a rect that isn't a hairline passes through.
pub(super) fn snap_hairline(
    fx: f32,
    fw: f32,
    fy: f32,
    fh: f32,
    radius: f32,
    scale: f32,
) -> (f32, f32, f32, f32, f32) {
    let logical = scale.max(1.0);
    let hair = logical.round().max(1.0);
    let snap = |pos: f32, len: f32| -> (f32, f32) {
        let centre = pos + len / 2.0;
        ((centre - hair / 2.0).round(), hair)
    };
    let thin_w = fw < logical;
    let thin_h = fh < logical;
    if !thin_w && !thin_h {
        return (fx, fw, fy, fh, radius);
    }
    let (fx, fw) = if thin_w { snap(fx, fw) } else { (fx, fw) };
    let (fy, fh) = if thin_h { snap(fy, fh) } else { (fy, fh) };
    (fx, fw, fy, fh, 0.0)
}

/// Draw the text cursor at a cell per its `style`:
///   * `Block` — overlay 50% white on the whole cell, so the glyph under it
///     stays faintly visible (the terminal-style caret), plus a crisp solid
///     rim around the cell so the cursor reads unambiguously against any
///     background (a translucent fill alone sinks into a busy highlight —
///     e.g. the bracket-match block — and the eye loses which cell owns it);
///   * `Bar` — a thin, solid vertical line on the cell's left edge (a modern
///     GUI insertion caret) that doesn't obscure the glyph;
///   * `Underline` — a thin solid rule along the cell bottom.
/// The bar/underline thickness scales with the cell so it stays proportional on
/// HiDPI (≥2 physical px). They paint solid `CURSOR_FG` rather than a blend, so a
/// 2px caret reads crisply.
/// `draw_top`/`draw_bottom` are already clamped to the visible region `[oy, fb_h)`.
/// `cell_top` is the logical (unclipped) top of the cursor cell in framebuffer pixels;
/// it may be negative or above `oy` when the cursor is partially scrolled off the top.
/// Used only for `Underline` to locate the baseline inside the cell.
pub(super) fn cursor_cell(
    canvas: &mut Canvas,
    left: usize,
    draw_top: usize,
    draw_bottom: usize,
    w: usize,
    h: usize,
    cell_top: isize,
    style: crate::host::gui::CursorStyle,
) {
    // The band clips the visible slice further; everything below derives from it.
    let draw_top = draw_top.max(canvas.y0);
    let draw_bottom = draw_bottom.min(canvas.y1);
    if draw_top >= draw_bottom {
        return;
    }
    let fb_w = canvas.w;
    let buf = &mut *canvas.buf;
    match style {
        crate::host::gui::CursorStyle::Block => {
            for y in draw_top..draw_bottom {
                let row = y * fb_w;
                for x in left..(left + w).min(fb_w) {
                    buf[row + x] = blend(buf[row + x], [0xff, 0xff, 0xff], 128);
                }
            }
            // The rim: solid CURSOR_FG, scaled like the bar caret so it stays
            // proportional on HiDPI. Vertical edges span the visible slice;
            // horizontal edges sit at the LOGICAL cell top/bottom (cell_top may
            // be off-screen when the cursor is partially scrolled off).
            let t = (w / 10).max(2);
            let x0 = left;
            let x1 = (left + w).min(fb_w);
            for y in draw_top..draw_bottom {
                let row = y * fb_w;
                for x in x0..(x0 + t).min(fb_w) {
                    buf[row + x] = pack(CURSOR_FG);
                }
                for x in x1.saturating_sub(t)..x1 {
                    buf[row + x] = pack(CURSOR_FG);
                }
            }
            let edges = [
                (cell_top, cell_top + t as isize),
                (cell_top + h as isize - t as isize, cell_top + h as isize),
            ];
            for (ey0, ey1) in edges {
                let y0 = ey0.max(draw_top as isize).max(0) as usize;
                let y1 = ey1.min(draw_bottom as isize).max(0) as usize;
                for y in y0..y1 {
                    let row = y * fb_w;
                    for x in x0..x1 {
                        buf[row + x] = pack(CURSOR_FG);
                    }
                }
            }
        }
        crate::host::gui::CursorStyle::Bar => {
            let thickness = (w / 8).max(2);
            fill_cell(
                canvas,
                left,
                draw_top,
                thickness,
                draw_bottom - draw_top,
                pack(CURSOR_FG),
            );
        }
        crate::host::gui::CursorStyle::Underline => {
            let thickness = (h / 10).max(2);
            // Underline sits at the bottom of the logical cell regardless of clipping.
            let uy = (cell_top + h as isize).saturating_sub(thickness as isize);
            if uy >= 0 {
                let uy = uy as usize;
                if uy >= draw_top && uy < draw_bottom {
                    let visible = thickness.min(draw_bottom - uy);
                    fill_cell(canvas, left, uy, w, visible, pack(CURSOR_FG));
                }
            }
        }
    }
}

#[cfg(test)]
mod text_aa_tests {
    use super::*;

    /// A subpixel-rendered cluster carries three coverages per pixel that differ from
    /// one another somewhere along a stem's edge — the whole point of LCD text — and
    /// the gray path renders the same glyph as one coverage in the alpha.
    #[test]
    fn subpixel_text_renders_per_channel_coverage() {
        let mut r = Renderer::new(1.0, default_families(), 15.0);
        r.set_text_aa(TextAa::Subpixel);
        let sub = r.build_cluster("l", r.default_family, false, false, 1, true);
        assert!(sub.subpixel, "the subpixel path fell back to gray");
        let fringe = sub
            .rgba
            .chunks(4)
            .any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2]));
        assert!(fringe, "no per-channel coverage differences at all");
        let gray = r.build_cluster("l", r.default_family, false, false, 1, false);
        assert!(!gray.subpixel);
        assert!(gray.rgba.chunks(4).any(|p| p[3] > 0));
    }
}
