//! The frame under construction: the solid and textured instance data in op order and
//! the batch list that draws runs of each, with culling and the clip band applied as
//! each quad is pushed.

use super::*;

/// One solid instance: rect (4) + rgba (4) + rot (1) + radius (1) + aa (1) + clip (4).
pub(super) const SOLID_FLOATS: usize = 15;
/// One textured instance: rect (4) + uv (4) + tint (4) + mode (1 u32 as its bits) + rot (1)
/// + clip (4) + the ground's luminance (1, a glyph's `text_alpha` input).
pub(super) const TEX_WORDS: usize = 19;

/// A run of consecutive quads drawn with one pipeline state: `count` instances starting
/// at `first` in that pipeline's instance buffer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Batch {
    /// `linear`: some quad in the run is translucent, so the run is drawn through the
    /// sRGB view and blends in linear light (`segments`). A run of opaque fills — their
    /// anti-aliased rims included — rides whichever pass is open.
    Solid {
        first: u32,
        count: u32,
        linear: bool,
    },
    /// `lcd`: the quads are per-channel text masks (`mode` 3 / 4), drawn with the LCD
    /// pipeline; a run never mixes them with grey or colour quads.
    Tex {
        key: TexKey,
        first: u32,
        count: u32,
        lcd: bool,
    },
    /// Emitter `id`'s particles, straight out of its buffer.
    Particles { id: u32 },
}

/// A solid quad's edge treatment: a plain fill, or the SDF ramp (with a corner radius in
/// pixels) a rounded or sub-cell rect wants.
#[derive(Clone, Copy)]
pub(super) enum Edge {
    Plain,
    Rounded(f32),
}

/// The frame under construction: both instance buffers' contents and the batch list, in
/// op order. Solids and textured quads keep separate buffers (different strides) and the
/// batch list says in which order to draw runs of each. `clip` is the rect every quad
/// pushed while it is set is clipped to — the whole viewport outside any region.
pub(super) struct FrameBuilder {
    pub(super) solids: Vec<f32>,
    pub(super) texs: Vec<u32>,
    pub(super) batches: Vec<Batch>,
    pub(super) fw: f32,
    pub(super) fh: f32,
    pub(super) clip: [f32; 4],
    /// The batch the top-level `Overlay` starts at: no batch merges across it, so the scene
    /// and the unlit layer draw as separate runs.
    pub(super) overlay_at: Option<usize>,
    /// How many regions deep the walk is (an `Overlay` counts only at the top).
    pub(super) nested: u32,
}

impl FrameBuilder {
    pub(super) fn new(fw: f32, fh: f32) -> FrameBuilder {
        FrameBuilder {
            solids: Vec::new(),
            texs: Vec::new(),
            batches: Vec::new(),
            fw,
            fh,
            clip: [0.0, 0.0, fw, fh],
            overlay_at: None,
            nested: 0,
        }
    }

    /// Whether a quad can put a pixel on screen and inside the clip. A rotated quad is
    /// judged by the disc its rotation sweeps, so a long thin quad turned across the
    /// viewport's corner is kept rather than culled by its unrotated rect.
    pub(super) fn visible(&self, x: f32, y: f32, w: f32, h: f32, rot: f32) -> bool {
        let [cx0, cy0, cx1, cy1] = self.clip;
        if cx1 <= cx0 || cy1 <= cy0 {
            return false;
        }
        if rot == 0.0 {
            return quad_visible(x, y, w, h, self.fw, self.fh)
                && x < cx1
                && y < cy1
                && x + w > cx0
                && y + h > cy0;
        }
        let radius = (w * w + h * h).sqrt() * 0.5;
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        quad_visible(
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
            self.fw,
            self.fh,
        )
    }

    /// A solid quad; `rgba[3]` is its alpha.
    pub(super) fn solid(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [u8; 4],
        rot: f32,
        edge: Edge,
    ) {
        if !self.visible(x, y, w, h, rot) {
            return;
        }
        let (radius, aa) = match edge {
            Edge::Plain => (0.0, 0.0),
            Edge::Rounded(r) => (r.max(0.0), 1.0),
        };
        let index = (self.solids.len() / SOLID_FLOATS) as u32;
        self.solids.extend_from_slice(&[
            x,
            y,
            w,
            h,
            rgba[0] as f32 / 255.0,
            rgba[1] as f32 / 255.0,
            rgba[2] as f32 / 255.0,
            rgba[3] as f32 / 255.0,
            rot,
            radius,
            aa,
            self.clip[0],
            self.clip[1],
            self.clip[2],
            self.clip[3],
        ]);
        // Only a translucent fill needs linear light. An opaque shape's anti-aliased rim reads
        // the same blended either way, so it rides whichever pass is open — a pane divider or
        // a mode-line chip between two runs of text costs no pass switch.
        let blends = rgba[3] < 255;
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Solid { count, linear, .. }) if merge => {
                *count += 1;
                *linear |= blends;
            }
            _ => self.batches.push(Batch::Solid {
                first: index,
                count: 1,
                linear: blends,
            }),
        }
    }

    /// Whether a quad may join the last batch: not when that batch ends the scene.
    pub(super) fn can_merge(&self) -> bool {
        self.overlay_at.is_none_or(|at| at < self.batches.len())
    }

    /// Solid instances recorded earlier (`FragmentCache::record`), already culled and
    /// clipped: they join the open solid batch as the first of them would have
    /// (`solid`), and the rest follow it — `linear` when any of them is translucent.
    pub(super) fn push_solids(&mut self, instances: &[f32], linear: bool) {
        let count = (instances.len() / SOLID_FLOATS) as u32;
        if count == 0 {
            return;
        }
        let first = (self.solids.len() / SOLID_FLOATS) as u32;
        self.solids.extend_from_slice(instances);
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Solid {
                count: open,
                linear: open_linear,
                ..
            }) if merge => {
                *open += count;
                *open_linear |= linear;
            }
            _ => self.batches.push(Batch::Solid {
                first,
                count,
                linear,
            }),
        }
    }

    /// Textured instances recorded earlier, sampling `key`: `push_solids`' twin.
    pub(super) fn push_texs(&mut self, key: TexKey, lcd: bool, instances: &[u32]) {
        let count = (instances.len() / TEX_WORDS) as u32;
        if count == 0 {
            return;
        }
        let first = (self.texs.len() / TEX_WORDS) as u32;
        self.texs.extend_from_slice(instances);
        let merge = self.can_merge();
        match self.batches.last_mut() {
            Some(Batch::Tex {
                key: open_key,
                count: open,
                lcd: open_lcd,
                ..
            }) if *open_key == key && *open_lcd == lcd && merge => *open += count,
            _ => self.batches.push(Batch::Tex {
                key,
                first,
                count,
                lcd,
            }),
        }
    }

    /// A plain opaque solid — the common cell-aligned fill.
    pub(super) fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
        self.solid(x, y, w, h, [rgb[0], rgb[1], rgb[2], 255], 0.0, Edge::Plain);
    }

    /// A textured quad sampling `uv` (`[u0 v0 du dv]`, texture space) of `key`; `mode`
    /// is the shader's: 0 a colour texel, 1 a coverage mask tinted, 2 a mask lifted, 3 / 4
    /// the same per colour channel (`glyph_mode`). `ground` is the linear luminance of what a
    /// glyph is drawn over, which its blend is corrected by (ADR-611); 0 for a sprite.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn textured(
        &mut self,
        key: TexKey,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        tint: [u8; 4],
        mode: u32,
        rot: f32,
        ground: f32,
    ) {
        if !self.visible(x, y, w, h, rot) {
            return;
        }
        let index = (self.texs.len() / TEX_WORDS) as u32;
        self.texs.extend_from_slice(&[
            x.to_bits(),
            y.to_bits(),
            w.to_bits(),
            h.to_bits(),
            uv[0].to_bits(),
            uv[1].to_bits(),
            uv[2].to_bits(),
            uv[3].to_bits(),
            (tint[0] as f32 / 255.0).to_bits(),
            (tint[1] as f32 / 255.0).to_bits(),
            (tint[2] as f32 / 255.0).to_bits(),
            (tint[3] as f32 / 255.0).to_bits(),
            mode,
            rot.to_bits(),
            self.clip[0].to_bits(),
            self.clip[1].to_bits(),
            self.clip[2].to_bits(),
            self.clip[3].to_bits(),
            ground.to_bits(),
        ]);
        let merge = self.can_merge();
        let lcd = mode >= 3;
        match self.batches.last_mut() {
            Some(Batch::Tex {
                key: k,
                count,
                lcd: l,
                ..
            }) if *k == key && *l == lcd && merge => *count += 1,
            _ => self.batches.push(Batch::Tex {
                key,
                first: index,
                count: 1,
                lcd,
            }),
        }
    }
}
