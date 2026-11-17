//! Glyph outlines hinted the way the desktop draws text (ADR-730).
//!
//! cosmic-text hands its rasteriser (swash) glyphs with hinting off, and swash's own hinter
//! has one fixed mode — FreeType's LCD target — so every outline glyph the window painted was
//! unhinted: a stroke that falls between two pixel rows is two rows of grey. A GNOME desktop
//! at a 1× scale draws text with FreeType's LIGHT target ("slight" in fontconfig): outlines
//! snapped to the pixel grid vertically only, so the baseline, the x-height and every
//! horizontal stroke sit on whole rows and the shapes keep their width. That is the crisp
//! look of the terminal beside the editor.
//!
//! This draws an outline glyph through skrifa's hinter — the TrueType interpreter for a font
//! with instructions, its autohinter otherwise, as FreeType chooses — at the target the
//! window's [`TextHinting`] names, and rasterises it with the same zeno pass, placement and
//! offsets swash uses, so the only difference from the old glyph is the hinting. Colour
//! glyphs (an emoji font's COLR, CBDT, sbix or SVG) are not this module's: the caller keeps
//! them on swash.

use std::collections::HashMap;

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, fontdb};
use skrifa::instance::{Location, Size};
use skrifa::outline::{
    DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, SmoothMode, Target,
};
use skrifa::{FontRef, GlyphId, MetadataProvider, Tag};
use swash::scale::Source;
use swash::scale::image::{Content, Image};
use swash::zeno::{Angle, Command, Format, Mask, Origin, Transform, Vector};

/// How text is hinted (behind `gui/text-hinting`), named as fontconfig and GNOME name it.
/// `Slight` is FreeType's LIGHT target — vertical snapping only; `Full` its NORMAL one
/// (GNOME's "medium" and "full" both draw this way with an autohinted or v40-hinted font).
/// `Auto` (the default) is slight at a 1× scale and none on HiDPI, as GTK 4 decides: with
/// twice the pixels a stroke is sharp unsnapped, and snapping would only move shapes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextHinting {
    Auto,
    None,
    Slight,
    Full,
}

/// The hinting a glyph is drawn with: [`TextHinting`] with `Auto` resolved.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Hint {
    None,
    Light,
    Normal,
}

impl TextHinting {
    /// What a window at HiDPI `scale` draws with.
    pub(crate) fn resolve(self, scale: f64) -> Hint {
        match self {
            TextHinting::None => Hint::None,
            TextHinting::Slight => Hint::Light,
            TextHinting::Full => Hint::Normal,
            TextHinting::Auto if (scale - 1.0).abs() < 1e-6 => Hint::Light,
            TextHinting::Auto => Hint::None,
        }
    }
}

/// A hinting instance is the font's programs run for one size, weight and target: built once,
/// then every glyph of that size is drawn through it.
type InstanceKey = (fontdb::ID, u16, u32, Hint);

/// The hinting instances in use.
#[derive(Default)]
pub(super) struct HintedGlyphs {
    instances: HashMap<InstanceKey, Option<HintingInstance>>,
}

/// How many instances are kept before the lot is dropped: each is a few kilobytes, and a
/// window draws in two or three sizes, so this is only ever reached by a zoom sweep.
const INSTANCES_KEPT: usize = 64;

impl HintedGlyphs {
    /// The coverage mask of the outline glyph `key` names, hinted as `hint` says and
    /// rasterised `x_scale` times wider than it is (1 for grey text, 3 for a per-subpixel
    /// mask), placed as swash places a mask; `None` for a glyph this path does not draw — no
    /// outline, or a font with colour glyphs — which the caller leaves to swash.
    pub(super) fn mask(
        &mut self,
        fs: &mut FontSystem,
        key: CacheKey,
        hint: Hint,
        x_scale: f32,
    ) -> Option<Image> {
        let font = fs.get_font(key.font_id, key.font_weight)?;
        let index = fs.db().face(font.id())?.index;
        let font_ref = FontRef::from_index(font.data(), index).ok()?;
        if has_color_glyphs(&font_ref) {
            return None;
        }
        let face = (font.id(), key.font_weight.0, key.font_size_bits);
        let commands = self.outline(&font_ref, face, key.glyph_id, hint)?;
        Some(rasterise(
            &commands,
            key.x_bin.as_float(),
            key.y_bin.as_float(),
            key.flags,
            x_scale,
        ))
    }

    /// Glyph `glyph_id` of `font` as path commands in pixels, y up, hinted as `hint` says —
    /// `face` naming the font, the CSS weight and the px (as `f32` bits) it is drawn at, which
    /// is what a hinting instance is built for. `None` for a glyph with no outline.
    fn outline(
        &mut self,
        font: &FontRef,
        face: (fontdb::ID, u16, u32),
        glyph_id: u16,
        hint: Hint,
    ) -> Option<Vec<Command>> {
        let (id, weight, px_bits) = face;
        let size = Size::new(f32::from_bits(px_bits));
        let location = weight_location(font, weight);
        let outlines = font.outline_glyphs();
        let glyph = outlines.get(GlyphId::new(u32::from(glyph_id)))?;
        let mut pen = Commands::default();
        let instance = match hint {
            Hint::None => None,
            Hint::Light | Hint::Normal => {
                if self.instances.len() >= INSTANCES_KEPT {
                    self.instances.clear();
                }
                self.instances
                    .entry((id, weight, px_bits, hint))
                    .or_insert_with(|| {
                        HintingInstance::new(&outlines, size, &location, options(hint)).ok()
                    })
                    .as_ref()
            }
        };
        let drawn = match instance {
            Some(instance) => glyph.draw(DrawSettings::hinted(instance, false), &mut pen),
            // unhinted, or a font whose programs fail to run: drawn as it is
            None => glyph.draw(DrawSettings::unhinted(size, &location), &mut pen),
        };
        drawn.ok()?;
        Some(pen.0)
    }
}

/// The hinter's settings for `hint`, as FreeType draws fontconfig's styles. Slight is the
/// AUTOHINTER in its light mode whatever the font carries: measured against FreeType (pango,
/// hintslight) on Ubuntu Sans Mono at 15 px, the autohinter matches it to the byte — the `=`
/// bars on one row each, 236 then 28 — where the font's own instructions in light mode move
/// nothing. Full is FreeType's engine choice (the font's instructions when it has them, the
/// autohinter otherwise) at its normal target. Advances stay as the shaper laid them out.
fn options(hint: Hint) -> HintingOptions {
    HintingOptions {
        engine: if hint == Hint::Light {
            Engine::Auto(None)
        } else {
            Engine::AutoFallback
        },
        target: Target::Smooth {
            mode: if hint == Hint::Normal {
                SmoothMode::Normal
            } else {
                SmoothMode::Light
            },
            symmetric_rendering: true,
            preserve_linear_metrics: true,
        },
    }
}

/// The font's variation location for CSS weight `weight` on its `wght` axis — clamped into the
/// axis, as cosmic-text clamps it for swash — or the default location for a static font.
fn weight_location(font: &FontRef, weight: u16) -> Location {
    let axes = font.axes();
    if axes.iter().any(|axis| axis.tag() == Tag::new(b"wght")) {
        axes.location([("wght", f32::from(weight))])
    } else {
        Location::default()
    }
}

/// Whether the font draws any glyph in colour: those are swash's to render.
fn has_color_glyphs(font: &FontRef) -> bool {
    [b"COLR", b"CBDT", b"sbix", b"SVG "]
        .iter()
        .any(|tag| font.table_data(Tag::new(tag)).is_some())
}

/// The mask of the outline in `commands` (pixels, y up) at the subpixel offset `(x, y)` (a cache
/// key's bins), with the synthesised italic `flags` may ask for, `x_scale` times wider: the zeno
/// pass swash runs for an outline.
fn rasterise(commands: &[Command], x: f32, y: f32, flags: CacheKeyFlags, x_scale: f32) -> Image {
    let offset = if flags.contains(CacheKeyFlags::PIXEL_FONT) {
        Vector::new(x.round(), y.round())
    } else {
        Vector::new(x, y)
    };
    let offset = Vector::new(offset.x * x_scale, offset.y);
    let widen = Transform::scale(x_scale, 1.0);
    let transform = if flags.contains(CacheKeyFlags::FAKE_ITALIC) {
        Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)).then(&widen)
    } else {
        widen
    };
    // measured first (`inspect`), then rendered: `render` alone reads a bottom-left mask's height
    // before it has one, and puts every glyph's top at zero
    let mut data = Vec::new();
    let placement = Mask::new(commands)
        .format(Format::Alpha)
        .origin(Origin::BottomLeft)
        .transform(Some(transform))
        .offset(offset)
        .render_offset(offset)
        .inspect(|format, width, height| data.resize(format.buffer_size(width, height), 0))
        .render_into(&mut data, None);
    Image {
        source: Source::Outline,
        content: Content::Mask,
        placement,
        data,
    }
}

/// An outline as zeno path commands: what skrifa's pen draws.
#[derive(Default)]
struct Commands(Vec<Command>);

impl OutlinePen for Commands {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::MoveTo(Vector::new(x, y)));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(Command::LineTo(Vector::new(x, y)));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0
            .push(Command::QuadTo(Vector::new(cx0, cy0), Vector::new(x, y)));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.push(Command::CurveTo(
            Vector::new(cx0, cy0),
            Vector::new(cx1, cy1),
            Vector::new(x, y),
        ));
    }

    fn close(&mut self) {
        self.0.push(Command::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swash::scale::{Render, ScaleContext};

    /// The bundled DejaVu Sans Mono.
    const DEJAVU: &[u8] = include_bytes!("../../../../assets/DejaVuSansMono.ttf");

    /// `ch` of DejaVu at `px` through this module, hinted as `hint` says, and rasterised.
    fn glyph(hint: Hint, ch: char, px: f32) -> Image {
        let font = FontRef::new(DEJAVU).unwrap();
        let glyph_id = font.charmap().map(ch).unwrap().to_u32() as u16;
        let commands = HintedGlyphs::default()
            .outline(
                &font,
                (fontdb::ID::dummy(), 400, px.to_bits()),
                glyph_id,
                hint,
            )
            .unwrap();
        rasterise(&commands, 0.0, 0.0, CacheKeyFlags::empty(), 1.0)
    }

    /// The darkest coverage on each pixel row of `image`, top to bottom.
    fn rows(image: &Image) -> Vec<u8> {
        let width = image.placement.width as usize;
        image
            .data
            .chunks(width.max(1))
            .map(|row| row.iter().copied().max().unwrap_or(0))
            .collect()
    }

    #[test]
    fn unhinted_it_is_the_glyph_swash_draws_placed_where_swash_places_it() {
        // the old path, for the same font, size and glyph: any difference in the mask or its
        // placement is this module's, and would move every glyph (a mask measured before it
        // was sized put every top at zero)
        let font = swash::FontRef::from_index(DEJAVU, 0).unwrap();
        let glyph_id = font.charmap().map('H');
        let mut context = ScaleContext::new();
        let mut scaler = context.builder(font).size(15.0).build();
        let theirs = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, glyph_id)
            .unwrap();
        let ours = glyph(Hint::None, 'H', 15.0);
        let place = |image: &Image| {
            let p = image.placement;
            (p.left, p.top, p.width, p.height)
        };
        assert_eq!(place(&ours), place(&theirs));
        assert_eq!(ours.data, theirs.data);
    }

    #[test]
    fn slight_hinting_puts_a_horizontal_stroke_on_whole_rows() {
        // `=` at 14 px: unhinted, the lower bar straddles two rows (148 and 152); hinted lightly
        // (FreeType's light autohinter, fontconfig's hintslight), each bar is one solid row
        let solid = |image: &Image| rows(image).iter().filter(|&&r| r >= 200).count();
        let unhinted = glyph(Hint::None, '=', 14.0);
        let slight = glyph(Hint::Light, '=', 14.0);
        assert_eq!(solid(&slight), 2, "both bars solid: {:?}", rows(&slight));
        assert!(
            solid(&unhinted) < 2,
            "unhinted is grey: {:?}",
            rows(&unhinted)
        );
        // and the glyph keeps its width: light hinting moves nothing sideways
        assert_eq!(slight.placement.width, unhinted.placement.width);
    }

    #[test]
    fn auto_hinting_is_slight_at_one_times_and_off_on_hidpi() {
        assert_eq!(TextHinting::Auto.resolve(1.0), Hint::Light);
        assert_eq!(TextHinting::Auto.resolve(2.0), Hint::None);
        assert_eq!(TextHinting::Slight.resolve(2.0), Hint::Light);
        assert_eq!(TextHinting::None.resolve(1.0), Hint::None);
    }
}
