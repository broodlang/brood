//! The run cache: a shaped text run's quads kept, keyed by everything they depend on
//! except position, and replayed at a new anchor while the run repeats (ADR-440).

use super::*;

// ---- the run cache: damage tracking for the GPU target (ADR-440) -------------------
//
// The CPU painter retains its canvas and re-rasterises only the rows a frame changed
// (`paint::strip_diff`). A swapchain image is not retained — every frame is drawn whole
// — but drawing is the cheap half on a GPU. The expensive half is on the CPU: walking a
// text op's graphemes, measuring each cluster and looking its glyph up in the atlas, for
// every op of every frame. So the GPU target retains the RESULT of that walk instead: a
// text run's quads, relative to where the run starts, keyed by everything they depend
// on. A frame that repeats a run — every line a keystroke did not touch, every line a
// scroll merely moved, every frame of a cursor blink — replays its quads with an offset
// (`RunCache::take`); only a changed run is shaped again. The replay then lands in the
// instance buffers, which are written only where they changed (`changed_span`), so a
// blink uploads the cursor's few quads and a typed character the tail after it.
//
// Keyed WITHOUT position, deliberately: a run's row and the scroll offset only move it,
// so a scroll (whole lines or a sub-cell glide) reuses every run it keeps on screen —
// the GPU's answer to the CPU painter's scroll blit, with no blit. Its column is in the
// key for a cell run (a tab advances to the next SCREEN stop, so where a run starts
// changes its shape); its alignment for a pixel run. The metrics, the resolved family
// and the window background (the contrast lift's ground) are in the key too, so a font
// or theme change misses rather than replays stale quads.

/// One quad of a cached run, relative to the run's anchor (`expand` adds the anchor).
#[derive(Clone, Copy)]
pub(super) enum RunQuad {
    Fill {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgb: [u8; 3],
    },
    Glyph {
        page: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        tint: [u8; 4],
        mode: u32,
        /// the linear luminance of the glyph's ground (`textured`)
        ground: f32,
    },
}

/// Where a run is placed: on the cell grid starting at a column, or in pixel space
/// with an alignment about its `x`.
#[derive(Clone, Copy, PartialEq, Hash)]
pub(super) enum RunPlace {
    Cell { col: u16 },
    Pixel { align: u8 },
}

/// Everything a run's quads depend on except where it is drawn.
#[derive(Clone, Copy, PartialEq, Hash)]
pub(super) struct RunContext {
    pub(super) place: RunPlace,
    /// cell width, cell height, font px (bits) and the resolved default family
    pub(super) metrics: [u32; 4],
    /// the window background: the ground a run without `:bg` is lifted against
    pub(super) ground: [u8; 3],
    /// the font registration the glyphs were baked under, whether ligature units were
    /// shaped (ADR-442), and the subpixel order (`GpuWindow::lcd_order`): each changes
    /// the quads a run produces
    pub(super) fonts: (u64, bool, u8),
}

/// A cached run: its key (checked on a hash hit, so a collision is a miss, never a
/// wrong picture) and its quads.
pub(super) struct TextRun {
    pub(super) text: String,
    pub(super) face: Face,
    pub(super) context: RunContext,
    pub(super) quads: Vec<RunQuad>,
}

/// The runs of the last frame and of the one being built. A run is looked up in this
/// frame's map first (two identical lines share one), then taken from the last frame's;
/// whatever the last frame had and this one did not use is dropped when the frame ends
/// (`end_frame`), so the cache holds one screen's runs, never a history.
#[derive(Default)]
pub(super) struct RunCache {
    pub(super) current: HashMap<u64, TextRun>,
    pub(super) previous: HashMap<u64, TextRun>,
    /// this frame's runs replayed / shaped, for the trace
    pub(super) hits: u32,
    pub(super) misses: u32,
}

pub(super) fn run_hash(text: &str, face: &Face, context: &RunContext) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (text, face, context).hash(&mut hasher);
    hasher.finish()
}

impl RunCache {
    /// The run's quads when this frame or the last already shaped it.
    pub(super) fn take(
        &mut self,
        hash: u64,
        text: &str,
        face: &Face,
        context: &RunContext,
    ) -> Option<&[RunQuad]> {
        let matches =
            |run: &TextRun| run.text == text && run.face == *face && run.context == *context;
        if !self.current.get(&hash).is_some_and(matches) {
            let run = self.previous.remove(&hash).filter(matches)?;
            self.current.insert(hash, run);
        }
        self.hits += 1;
        self.current.get(&hash).map(|run| run.quads.as_slice())
    }

    /// Keep a run the last frame drew for the next one without replaying it — a run inside a
    /// fragment whose instances were pushed whole (`FragmentCache`).
    pub(super) fn keep(&mut self, hash: u64) {
        if !self.current.contains_key(&hash)
            && let Some(run) = self.previous.remove(&hash)
        {
            self.current.insert(hash, run);
        }
    }

    /// Remember a run this frame shaped.
    pub(super) fn put(&mut self, hash: u64, run: TextRun) {
        self.misses += 1;
        self.current.insert(hash, run);
    }

    /// The frame is drawn: what it used is the next frame's to reuse, and nothing else.
    pub(super) fn end_frame(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        self.hits = 0;
        self.misses = 0;
    }
}

/// `BROOD_GUI_DAMAGE=0` turns the GPU target's reuse off as it does the CPU painter's
/// row diff: every run shaped and every instance buffer written whole, each frame — the
/// escape hatch if reuse ever shows a stale picture. Read once.
pub(super) fn reuse_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("BROOD_GUI_DAMAGE")
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

/// Push a shaped run's quads onto the frame at `(anchor_x, anchor_y)`. Each goes through
/// the builder's own fill / textured, so culling, the clip band and batching are what
/// they would be had the run been shaped this frame.
pub(super) fn replay(fb: &mut FrameBuilder, quads: &[RunQuad], anchor_x: f32, anchor_y: f32) {
    for quad in quads {
        match *quad {
            RunQuad::Fill { x, y, w, h, rgb } => fb.fill(anchor_x + x, anchor_y + y, w, h, rgb),
            RunQuad::Glyph {
                page,
                x,
                y,
                w,
                h,
                uv,
                tint,
                mode,
                ground,
            } => fb.textured(
                TexKey::Atlas(page),
                anchor_x + x,
                anchor_y + y,
                w,
                h,
                uv,
                tint,
                mode,
                0.0,
                ground,
            ),
        }
    }
}
