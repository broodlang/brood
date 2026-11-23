//! Drawing a frame's rects on threads (ADR-739). `render_ops` draws through a [`Painter`]: the
//! renderer itself, which builds what it lacks as it goes, or a [`BandPainter`], which only
//! reads — the glyph cache, the textures, the cell sizes of the frame's zoomed panes — and
//! writes nothing but its own rows and its own blend tables. A paint that draws enough pixels
//! first PREPARES the frame on the GUI thread (`prepare_ops`: every glyph built, every pane's
//! cell size decided), then splits the rows it draws into disjoint bands, one per thread
//! (`std::thread::scope`), each drawing every rect cut to its rows in order. A band leaves a
//! picture (resampled into the renderer's cache as it is drawn) to the GUI thread, which draws
//! the picture's rows of that rect after the bands (ADR-746); a rect that met something else
//! prepared nothing for (a particle, a glyph the walk did not foresee) it draws whole. The
//! bytes never depend on how many threads drew them (the blend tables hold what the blend
//! computes, ADR-737), nor on what was drawn twice: a rect drawn from the frame is its pixels
//! whatever was under it.

use super::*;
use crate::host::gui::backend::render::{
    CachedGlyph, GlyphInk, GlyphMap, composite_glyph, glyph_key,
};
use unicode_segmentation::UnicodeSegmentation;

/// What `render_ops` draws through: the renderer's state an op reads, its glyphs, textures,
/// pictures and particles.
pub(in crate::host::gui::backend) trait Painter {
    /// The cell metrics a logical font size produces (a zoomed pane's).
    fn metrics_at(&mut self, logical_px: f32) -> CellMetrics;
    /// The metrics the ops are drawn at now.
    fn metrics(&self) -> CellMetrics;
    fn set_metrics(&mut self, metrics: CellMetrics);
    fn fg(&self) -> [u8; 3];
    fn bg(&self) -> [u8; 3];
    fn ligatures(&self) -> bool;
    fn scale(&self) -> f64;
    fn caret_hidden(&self) -> bool;
    fn texture(&self, handle: u32) -> Option<&CpuTexture>;
    /// The picture the frame's 3D `scene` was rasterised to (ADR-786).
    fn scene_picture(&self, scene: &crate::host::gui::mesh::Scene) -> Option<&CpuTexture>;
    #[allow(clippy::too_many_arguments)]
    fn draw_cluster(
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
    );
    fn image(
        &mut self,
        canvas: &mut Canvas,
        picture: &std::sync::Arc<crate::host::picture::Picture>,
        fit: crate::host::picture::Fit,
        anchor: crate::host::picture::Anchor,
        block: PixelBlock,
    );
    fn particles(&mut self, canvas: &mut Canvas, id: u32);
}

impl Painter for Renderer {
    fn metrics_at(&mut self, logical_px: f32) -> CellMetrics {
        Renderer::metrics_at(self, logical_px)
    }

    fn metrics(&self) -> CellMetrics {
        Renderer::metrics(self)
    }

    fn set_metrics(&mut self, metrics: CellMetrics) {
        Renderer::set_metrics(self, metrics)
    }

    fn fg(&self) -> [u8; 3] {
        Renderer::fg(self)
    }

    fn bg(&self) -> [u8; 3] {
        Renderer::bg(self)
    }

    fn ligatures(&self) -> bool {
        Renderer::ligatures(self)
    }

    fn scale(&self) -> f64 {
        Renderer::scale(self)
    }

    fn caret_hidden(&self) -> bool {
        self.caret_hidden
    }

    fn texture(&self, handle: u32) -> Option<&CpuTexture> {
        self.textures.get(&handle)
    }

    fn scene_picture(&self, scene: &crate::host::gui::mesh::Scene) -> Option<&CpuTexture> {
        Renderer::scene_picture(self, scene)
    }

    fn draw_cluster(
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
        Renderer::draw_cluster(
            self, canvas, left, top, g, family, bold, italic, scale, fg, clip_skip,
        )
    }

    fn image(
        &mut self,
        canvas: &mut Canvas,
        picture: &std::sync::Arc<crate::host::picture::Picture>,
        fit: crate::host::picture::Fit,
        anchor: crate::host::picture::Anchor,
        block: PixelBlock,
    ) {
        paint_image(canvas, self, picture, fit, anchor, block)
    }

    fn particles(&mut self, canvas: &mut Canvas, id: u32) {
        paint_particles(canvas, self, id)
    }
}

/// The renderer as a band reads it: a snapshot of its colours and switches, its glyph cache
/// and textures, the cell sizes the frame's zoomed panes were prepared at, and blend tables
/// of the band's own. What it could not draw in the rect it is drawing, it notes:
/// `missed`, something prepared nothing for (a particle, a glyph the walk did not foresee), and
/// `pictures`, the rows of the pictures it left out.
pub(in crate::host::gui::backend) struct BandPainter<'a> {
    cache: &'a GlyphMap<CachedGlyph>,
    textures: &'a HashMap<u32, CpuTexture>,
    /// The frame's 3D scenes as the GUI thread rasterised them (`Renderer::prepare_scenes`).
    scenes: &'a [(std::sync::Arc<crate::host::gui::mesh::Scene>, CpuTexture)],
    regions: &'a [(u32, CellMetrics)],
    ink: GlyphInk<'a>,
    tables: &'a mut BlendTables,
    /// The renderer's switches, copied; `metrics` is this band's own as it enters and leaves
    /// a zoomed pane.
    state: Snapshot,
    missed: bool,
    /// The pixel rows `[top, bottom)` of the pictures met in the rect being drawn: a picture
    /// is resampled into the renderer's cache as it is drawn, so the GUI thread draws those
    /// rows of the rect after the bands — nothing outside them depends on the picture.
    pictures: Vec<(isize, isize)>,
    /// The clusters drawn, added to `PAINT_CLUSTERS` once at the end: one shared counter
    /// bumped per glyph from every thread would bounce its cache line between them.
    clusters: u64,
}

impl Painter for BandPainter<'_> {
    fn metrics_at(&mut self, logical_px: f32) -> CellMetrics {
        match self
            .regions
            .iter()
            .find(|(bits, _)| *bits == logical_px.to_bits())
        {
            Some((_, metrics)) => *metrics,
            None => {
                self.missed = true;
                self.state.metrics
            }
        }
    }

    fn metrics(&self) -> CellMetrics {
        self.state.metrics
    }

    fn set_metrics(&mut self, metrics: CellMetrics) {
        self.state.metrics = metrics;
    }

    fn fg(&self) -> [u8; 3] {
        self.state.fg
    }

    fn bg(&self) -> [u8; 3] {
        self.state.bg
    }

    fn ligatures(&self) -> bool {
        self.state.ligatures
    }

    fn scale(&self) -> f64 {
        self.state.scale
    }

    fn caret_hidden(&self) -> bool {
        self.state.caret_hidden
    }

    fn texture(&self, handle: u32) -> Option<&CpuTexture> {
        self.textures.get(&handle)
    }

    fn scene_picture(&self, scene: &crate::host::gui::mesh::Scene) -> Option<&CpuTexture> {
        self.scenes
            .iter()
            .find(|(seen, _)| std::ptr::eq(&**seen, scene))
            .map(|(_, picture)| picture)
    }

    fn draw_cluster(
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
        let Some(key) = glyph_key(
            g,
            family.unwrap_or(self.state.default_family),
            bold,
            italic,
            scale,
            self.state.subpixel,
            self.state.metrics.px,
        ) else {
            return;
        };
        self.clusters += 1;
        let Some(glyph) = self.cache.get(&key) else {
            self.missed = true;
            return;
        };
        composite_glyph(
            canvas,
            glyph,
            (left, top),
            fg,
            clip_skip,
            self.ink,
            self.tables,
        );
    }

    fn image(
        &mut self,
        _canvas: &mut Canvas,
        _picture: &std::sync::Arc<crate::host::picture::Picture>,
        _fit: crate::host::picture::Fit,
        _anchor: crate::host::picture::Anchor,
        block: PixelBlock,
    ) {
        // the rows `paint_image` can reach: the block's, below the grid top it is clipped at
        self.pictures.push((
            block.top.max(block.clip_top),
            block.top.saturating_add(block.height as isize),
        ));
    }

    fn particles(&mut self, _canvas: &mut Canvas, _id: u32) {
        self.missed = true;
    }
}

/// Prepare `ops` for drawing on threads: every glyph a text op or a block caret will draw
/// built into the cache at the size it is drawn at, and every zoomed pane's cell size
/// decided (`regions`, by the logical px's bits). The single-threaded half of a threaded
/// paint: it writes the renderer, the bands only read it.
pub(in crate::host::gui::backend) fn prepare_ops(
    ops: &[Op],
    r: &mut Renderer,
    regions: &mut Vec<(u32, CellMetrics)>,
) {
    for op in ops {
        match op {
            Op::ScrollRegion { ops, .. } | Op::AboveSeam { ops } => prepare_ops(ops, r, regions),
            Op::Fragment(fragment) => prepare_ops(&fragment.0, r, regions),
            Op::CellRegion { px, ops, .. } => {
                let region = r.metrics_at(*px);
                if !regions.iter().any(|(bits, _)| *bits == px.to_bits()) {
                    regions.push((px.to_bits(), region));
                }
                if region.cell_w == 0 || region.cell_h == 0 {
                    continue;
                }
                let saved = r.metrics();
                r.set_metrics(region);
                prepare_ops(ops, r, regions);
                r.set_metrics(saved);
            }
            Op::Text { s, face, .. } => {
                let ligatures = r.ligatures();
                for unit in text_units(s, ligatures) {
                    if unit == "\t" || unit_cells(unit) == 0 {
                        continue;
                    }
                    prepare_glyph(r, unit, face);
                }
            }
            Op::TextPx { s, face, .. } => {
                for unit in s.graphemes(true) {
                    if cluster_cells(unit) > 0 {
                        prepare_glyph(r, unit, face);
                    }
                }
            }
            Op::Cursor {
                row,
                col,
                style: crate::host::gui::CursorStyle::Block,
                under: Some(under),
                ..
            } => {
                let glyph = caret_glyph(*row, *col, under, r.bg());
                prepare_ops(std::slice::from_ref(&glyph), r, regions);
            }
            _ => {}
        }
    }
}

fn prepare_glyph(r: &mut Renderer, unit: &str, face: &crate::host::gui::Face) {
    if let Some(key) = r.glyph_key(unit, face.family, face.bold, face.italic, face.scale) {
        r.ensure_cluster(&key, unit);
    }
}

/// How many threads draw a frame's bands: `BROOD_GUI_THREADS` (1 draws on the GUI thread
/// alone), else the machine's parallelism, at most 8. Read once.
pub(in crate::host::gui::backend) fn gui_threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        std::env::var("BROOD_GUI_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, |count| count.get())
                    .min(8)
            })
            .max(1)
    })
}

/// The fewest pixels a paint draws before its rects are split over threads: below it a
/// thread's start and the frame's prepare walk cost more than they save: at 1080p five
/// changed rows (200 000 px) draw as fast either way, ten (390 000 px) a quarter faster on
/// threads (ADR-739; `paint/timings.rs`' `paint_thread_threshold` re-measures it).
pub(in crate::host::gui::backend) const THREADED_MIN_PX: usize = 300_000;

/// Draw `rects` of `frame` into `pixels` (`fb_w`×`fb_h`): each cleared to `bg0` and the frame
/// rendered into it, in order. On `threads` threads when there is more than one and the rects
/// cover at least [`THREADED_MIN_PX`] (or `threads` is forced, for the tests), else on this
/// one; the bytes are the same either way. What the bands could not draw — a picture's rows,
/// a rect that met something prepared nothing for — this thread draws after them, over what
/// they drew: a rect drawn whole from the frame is its pixels whatever was under it.
#[allow(clippy::too_many_arguments)]
pub(in crate::host::gui::backend) fn draw_rects(
    r: &mut Renderer,
    frame: &[Op],
    pixels: &mut [u32],
    (fb_w, fb_h): (usize, usize),
    rects: &[DamageRect],
    (ox, rows, cw, bg0): (usize, &RowMap, usize, u32),
    threads: usize,
    force_threads: bool,
) {
    let threaded = threads > 1 && (force_threads || rects_area(rects) >= THREADED_MIN_PX);
    r.retained.bands = 0;
    r.retained.drawn_again_px = 0;
    if !threaded {
        for rect in rects {
            draw_rect(r, frame, pixels, 0, (fb_w, fb_h), rect, (ox, rows, cw, bg0));
        }
        return;
    }
    let (bands, again) = draw_threaded(
        r,
        frame,
        pixels,
        (fb_w, fb_h),
        rects,
        (ox, rows, cw, bg0),
        threads,
    );
    r.retained.bands = bands;
    r.retained.drawn_again_px = rects_area(&again);
    for rect in &again {
        draw_rect(r, frame, pixels, 0, (fb_w, fb_h), rect, (ox, rows, cw, bg0));
    }
}

/// One rect drawn by `painter` into `pixels`, the framebuffer's pixels from index `first` on.
#[allow(clippy::too_many_arguments)]
fn draw_rect<P: Painter>(
    painter: &mut P,
    frame: &[Op],
    pixels: &mut [u32],
    first: usize,
    (fb_w, fb_h): (usize, usize),
    rect: &DamageRect,
    (ox, rows, cw, bg0): (usize, &RowMap, usize, u32),
) {
    let mut canvas = Canvas::rect(pixels, fb_w, fb_h, (rect.x0, rect.y0), (rect.x1, rect.y1));
    canvas.first = first;
    // The conventional frame opens with a full `:clear`; a frame that doesn't still expects
    // a clean background, and a rect repaint needs one either way (its old pixels are
    // exactly what changed).
    canvas.fill_rows(rect.y0, rect.y1 - rect.y0, bg0);
    render_ops(frame, &mut canvas, painter, ox, rows, cw, bg0, 0);
}

/// `rects` cut to the pixel rows `[top, bottom)`, the empty cuts dropped.
fn cut_to(rects: &[DamageRect], top: usize, bottom: usize) -> impl Iterator<Item = DamageRect> {
    rects.iter().filter_map(move |rect| {
        let cut = DamageRect {
            y0: rect.y0.max(top),
            y1: rect.y1.min(bottom),
            ..*rect
        };
        (cut.y1 > cut.y0).then_some(cut)
    })
}

/// The threaded draw: prepare, cut the drawn rows into `threads` bands of about equal work,
/// and draw every rect cut to each band on a thread of its own. Answers the number of bands
/// and the rects the GUI thread still has to draw: the rows of each cut that held a picture,
/// a whole cut that met something prepared nothing for, every cut of a band that panicked.
#[allow(clippy::too_many_arguments)]
fn draw_threaded(
    r: &mut Renderer,
    frame: &[Op],
    pixels: &mut [u32],
    (fb_w, fb_h): (usize, usize),
    rects: &[DamageRect],
    (ox, rows, cw, bg0): (usize, &RowMap, usize, u32),
    threads: usize,
) -> (usize, Vec<DamageRect>) {
    let mut regions = Vec::new();
    prepare_ops(frame, r, &mut regions);
    let cuts = band_cuts(rects, fb_h, threads);
    if r.band_tables.len() < cuts.len() {
        r.band_tables.resize_with(cuts.len(), BlendTables::default);
    }
    let snapshot = Snapshot {
        metrics: r.metrics(),
        fg: Renderer::fg(r),
        bg: Renderer::bg(r),
        ligatures: Renderer::ligatures(r),
        scale: Renderer::scale(r),
        caret_hidden: r.caret_hidden,
        default_family: r.default_family,
        subpixel: r.subpixel_text(),
    };
    let (text_contrast, cov_lut) = r.glyph_ink_parts();
    let cov_lut: [u8; 256] = *cov_lut;
    let cov_lut = &cov_lut;
    let (cache, textures, tables) = (&r.cache, &r.textures, &mut r.band_tables);
    let scenes = &r.scene_pictures[..];
    let regions = &regions;
    // the bands' rows, each a disjoint slice of the framebuffer
    let mut slices = Vec::with_capacity(cuts.len());
    let mut rest = pixels;
    let mut taken = 0;
    for &(top, bottom) in &cuts {
        let (_, after) = rest.split_at_mut((top - taken) * fb_w);
        let (band, after) = after.split_at_mut((bottom - top) * fb_w);
        slices.push((top, band));
        rest = after;
        taken = bottom;
    }
    let again = std::thread::scope(|scope| {
        let workers: Vec<_> = slices
            .into_iter()
            .zip(tables.iter_mut())
            .map(|((top, band), tables)| {
                scope.spawn(move || {
                    let mut painter = BandPainter {
                        cache,
                        textures,
                        scenes,
                        regions,
                        ink: GlyphInk {
                            text_contrast,
                            cov_lut,
                        },
                        tables,
                        state: snapshot,
                        missed: false,
                        pictures: Vec::new(),
                        clusters: 0,
                    };
                    let bottom = top + band.len() / fb_w;
                    let mut again = Vec::new();
                    for cut in cut_to(rects, top, bottom) {
                        draw_rect(
                            &mut painter,
                            frame,
                            band,
                            top * fb_w,
                            (fb_w, fb_h),
                            &cut,
                            (ox, rows, cw, bg0),
                        );
                        if std::mem::take(&mut painter.missed) {
                            again.push(cut);
                        } else {
                            for (picture_top, picture_bottom) in painter.pictures.drain(..) {
                                let (y0, y1) = (
                                    picture_top.clamp(0, fb_h as isize) as usize,
                                    picture_bottom.clamp(0, fb_h as isize) as usize,
                                );
                                again.extend(cut_to(std::slice::from_ref(&cut), y0, y1));
                            }
                        }
                        painter.pictures.clear();
                    }
                    (again, painter.clusters)
                })
            })
            .collect();
        let mut again = Vec::new();
        let mut clusters = 0;
        // every band joined, a panicked one's rects all left to the GUI thread rather than
        // unwound
        for (worker, &(top, bottom)) in workers.into_iter().zip(&cuts) {
            match worker.join() {
                Ok((band_again, band_clusters)) => {
                    again.extend(band_again);
                    clusters += band_clusters;
                }
                Err(_) => again.extend(cut_to(rects, top, bottom)),
            }
        }
        // what the GUI thread draws again, it counts again
        PAINT_CLUSTERS.fetch_add(clusters, Ordering::Relaxed);
        again
    });
    (cuts.len(), again)
}

/// The renderer's switches a band copies (`BandPainter`).
#[derive(Clone, Copy)]
struct Snapshot {
    metrics: CellMetrics,
    fg: [u8; 3],
    bg: [u8; 3],
    ligatures: bool,
    scale: f64,
    caret_hidden: bool,
    default_family: u32,
    subpixel: bool,
}

/// Cut the rows `rects` cover into at most `threads` bands `[top, bottom)` of about equal
/// work (pixels drawn), in order, none empty.
fn band_cuts(rects: &[DamageRect], fb_h: usize, threads: usize) -> Vec<(usize, usize)> {
    let mut per_row = vec![0usize; fb_h];
    for rect in rects {
        for row in rect.y0..rect.y1.min(fb_h) {
            per_row[row] += rect.x1.saturating_sub(rect.x0);
        }
    }
    let total: usize = per_row.iter().sum();
    if total == 0 {
        return Vec::new();
    }
    let share = total.div_ceil(threads);
    let mut cuts = Vec::new();
    let (mut top, mut work) = (0, 0);
    for (row, &width) in per_row.iter().enumerate() {
        work += width;
        if work >= share && cuts.len() + 1 < threads {
            cuts.push((top, row + 1));
            top = row + 1;
            work = 0;
        }
    }
    if top < fb_h {
        cuts.push((top, fb_h));
    }
    cuts
}

#[cfg(test)]
mod band_cuts_tests {
    use super::*;

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DamageRect {
        DamageRect { x0, y0, x1, y1 }
    }

    /// The bands tile the framebuffer's rows top to bottom, no more of them than threads,
    /// and split the drawn pixels about evenly — wherever the rects sit.
    #[test]
    fn bands_tile_the_rows_and_share_the_work() {
        let fb_h = 100;
        for (rects, threads) in [
            (vec![rect(0, 0, 50, 100)], 4),
            (vec![rect(0, 10, 50, 20), rect(0, 80, 50, 90)], 2),
            (vec![rect(0, 40, 10, 41)], 8),
            (vec![rect(0, 0, 50, 100)], 200),
        ] {
            let cuts = band_cuts(&rects, fb_h, threads);
            assert!(!cuts.is_empty() && cuts.len() <= threads, "{cuts:?}");
            assert_eq!(cuts[0].0, 0);
            assert_eq!(cuts[cuts.len() - 1].1, fb_h);
            for pair in cuts.windows(2) {
                assert_eq!(pair[0].1, pair[1].0, "{cuts:?}");
            }
            assert!(cuts.iter().all(|(top, bottom)| top < bottom), "{cuts:?}");
            let total = rects_area(&rects);
            let work = |(top, bottom): (usize, usize)| -> usize {
                rects
                    .iter()
                    .map(|whole| {
                        let cut = DamageRect {
                            y0: whole.y0.max(top),
                            y1: whole.y1.min(bottom),
                            ..*whole
                        };
                        cut.area()
                    })
                    .sum()
            };
            let widest_row = rects.iter().map(|whole| whole.x1 - whole.x0).sum::<usize>();
            for &cut in &cuts {
                assert!(
                    work(cut) <= total.div_ceil(threads) + widest_row,
                    "band {cut:?} of {cuts:?} draws more than its share"
                );
            }
        }
        assert!(band_cuts(&[], fb_h, 4).is_empty());
    }
}
