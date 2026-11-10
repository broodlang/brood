//! The glyph atlas: rasterised clusters packed onto shelves of shared texture pages, keyed
//! by everything a cluster is drawn from, and the shader mode a glyph is drawn in.

use super::*;

/// The glyph atlas page size. 2048² RGBA is 16 MB — a few thousand cells' worth of
/// glyphs at an editor size, and every device wgpu runs on allows at least 8192².
pub(super) const ATLAS_SIZE: u32 = 2048;

/// One page of the glyph atlas: the texture plus a shelf packer's cursor. Glyphs are
/// packed left to right along a shelf whose height is the tallest glyph on it; a glyph
/// that does not fit starts a new shelf below, and one that does not fit the page starts
/// a new page. Simple, and enough: every glyph of one font size is about the same height.
pub(super) struct AtlasPage {
    pub(super) texture: wgpu::Texture,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) next_x: u32,
    pub(super) next_y: u32,
    pub(super) shelf_h: u32,
}

/// Where a rasterised cluster sits in the atlas: page + pixel rect + whether it is a
/// coverage mask (`mode` 1 in the shader) rather than a colour bitmap, and whether that
/// mask is per colour channel (`lcd`, `mode` 3).
#[derive(Clone, Copy)]
pub(super) struct AtlasGlyph {
    pub(super) page: usize,
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
    pub(super) mono: bool,
    pub(super) lcd: bool,
}

impl GpuWindow {
    /// Pack a rasterised `w`×`h` cluster into the atlas, opening a new shelf or page as
    /// needed, and return where it landed. `None` for a glyph larger than a page.
    pub(super) fn pack_glyph(
        &mut self,
        rgba: &[u8],
        w: u32,
        h: u32,
        mono: bool,
        lcd: bool,
    ) -> Option<AtlasGlyph> {
        if w == 0 || h == 0 || w > ATLAS_SIZE || h > ATLAS_SIZE {
            return None;
        }
        let mut page_index = self.atlas.len().saturating_sub(1);
        loop {
            if page_index >= self.atlas.len() {
                let (texture, bind_group) = self.create_texture(ATLAS_SIZE, ATLAS_SIZE);
                self.atlas.push(AtlasPage {
                    texture,
                    bind_group,
                    next_x: 0,
                    next_y: 0,
                    shelf_h: 0,
                });
            }
            let page = &mut self.atlas[page_index];
            if page.next_x + w > ATLAS_SIZE {
                page.next_y += page.shelf_h;
                page.next_x = 0;
                page.shelf_h = 0;
            }
            if page.next_y + h > ATLAS_SIZE {
                page_index += 1;
                continue;
            }
            let (x, y) = (page.next_x, page.next_y);
            page.next_x += w;
            page.shelf_h = page.shelf_h.max(h);
            let texture = &self.atlas[page_index].texture;
            self.write_rgba(texture, x, y, rgba, w, h);
            return Some(AtlasGlyph {
                page: page_index,
                x,
                y,
                w,
                h,
                mono,
                lcd,
            });
        }
    }

    /// The subpixel order glyphs are baked in for this window: the renderer's
    /// (`Renderer::lcd_order`) when this device could make the LCD pipeline, else 0 (grey).
    pub(super) fn lcd_order(&self, renderer: &Renderer) -> u8 {
        if self.lcd.is_some() {
            renderer.lcd_order()
        } else {
            0
        }
    }

    /// The atlas slot for cluster `g` under a face at the renderer's current metrics,
    /// rasterising and packing it on first sight. `None` when it has no pixels.
    pub(super) fn glyph_slot(
        &mut self,
        renderer: &mut Renderer,
        g: &str,
        family: Option<u32>,
        bold: bool,
        italic: bool,
        scale: u16,
    ) -> Option<AtlasGlyph> {
        let metrics = renderer.metrics();
        let lcd_order = self.lcd_order(renderer);
        let key = glyph_key(
            g,
            family.unwrap_or(renderer.default_family()),
            bold,
            italic,
            scale,
            metrics.px,
            (metrics.cell_w, metrics.cell_h),
            (renderer.font_generation(), lcd_order),
        );
        if let Some(slot) = self.glyphs.get(&key) {
            return *slot;
        }
        let cg = renderer.cluster_glyph(g, family, bold, italic, scale, lcd_order != 0);
        let (rgba, w, h, color, lcd) = (
            cg.rgba.clone(),
            cg.width as u32,
            cg.height as u32,
            cg.color,
            cg.subpixel,
        );
        let packed = self.pack_glyph(&rgba, w, h, !color, lcd);
        self.glyphs.insert(key, packed);
        packed
    }
}

/// The textured shader's `mode` for a glyph: 0 a colour bitmap (an emoji), else a coverage
/// mask — 1 grey, 3 per channel (LCD) — plus one when the text takes the contrast lift
/// (`lift`: lighter than its ground).
pub(super) fn glyph_mode(mono: bool, lcd: bool, lift: bool) -> u32 {
    match (mono, lcd) {
        (false, _) => 0,
        (true, false) => 1 + lift as u32,
        (true, true) => 3 + lift as u32,
    }
}

/// The atlas key of a cluster under a face at a font size: what it is drawn from depends
/// on the cluster, the RESOLVED family (a face without one is drawn in the window's
/// default, which `gui-font!` changes), the weight/slant, the scale, the pixel size (a
/// cell region rasterises at its own px), the cell box the cluster is baked into (a
/// line-height change resizes it at the same px) and how it was baked: the font registration
/// (a re-registered family keyword names other faces) and the subpixel order (0 grey, 1
/// R-G-B, 2 B-G-R — `gui-text-aa!` bakes the same cluster three ways) — not the colour, a
/// per-instance tint.
pub(super) fn glyph_key(
    g: &str,
    family: u32,
    bold: bool,
    italic: bool,
    scale: u16,
    px: f32,
    cell: (usize, usize),
    baked: (u64, u8),
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    g.hash(&mut h);
    family.hash(&mut h);
    bold.hash(&mut h);
    italic.hash(&mut h);
    scale.hash(&mut h);
    px.to_bits().hash(&mut h);
    cell.hash(&mut h);
    baked.hash(&mut h);
    h.finish()
}
