//! Colour as a painter uses it: sRGB and linear light, blending, the coverage curves that
//! make text read as text, and the LCD filter. Pure arithmetic over pixels; nothing here
//! knows a font or a frame.

use super::*;

/// Subpixels per pixel across a row: an LCD pixel is three stripes, R G B.
pub(super) const LCD_OVERSAMPLE: i32 = 3;

/// FreeType's default LCD filter (`FT_LCD_FILTER_DEFAULT`), in 256ths: each subpixel's
/// coverage spread over itself and two neighbours each side. Unfiltered, a stem edge
/// that covers one stripe of a pixel lights that stripe alone, a saturated orange or
/// blue fringe; filtered, the energy is shared with the stripes beside it and the edge
/// reads as a lighter, near-neutral one. The weights sum to 256, so a solid run stays
/// solid.
const LCD_FILTER: [u32; 5] = [0x08, 0x4D, 0x56, 0x4D, 0x08];

/// A glyph's per-channel coverage at whole pixels: `width` × the image's rows, three
/// bytes a pixel (R G B in panel order), its left edge `left` pixels from the pen.
pub(super) struct LcdMask {
    pub(super) left: i32,
    pub(super) width: usize,
    pub(super) rgb: Vec<u8>,
}

/// Fold a coverage image rendered at [`LCD_OVERSAMPLE`]× horizontal resolution (`w3`
/// subpixels wide, its first `left3` subpixels from the pen) into an [`LcdMask`]:
/// filter each row with [`LCD_FILTER`], then give each channel its own stripe of the
/// pixel — the left stripe to red on an R-G-B panel, to blue on a B-G-R one (`bgr`).
/// The filter reaches two subpixels past the image on each side, so the mask can be a
/// pixel wider than the image.
pub(super) fn lcd_filter(alpha: &[u8], left3: i32, w3: usize, h: usize, bgr: bool) -> LcdMask {
    let reach = (LCD_FILTER.len() / 2) as i32;
    let first = (left3 - reach).div_euclid(LCD_OVERSAMPLE);
    let last = (left3 + w3 as i32 - 1 + reach).div_euclid(LCD_OVERSAMPLE);
    let width = (last - first + 1).max(0) as usize;
    let mut rgb = vec![0u8; width * h * 3];
    for row in 0..h {
        let line = &alpha[row * w3..(row + 1) * w3];
        let at = |sub: i32| -> u32 {
            let i = sub - left3;
            if i < 0 || i >= w3 as i32 {
                0
            } else {
                line[i as usize] as u32
            }
        };
        for p in 0..width {
            for stripe in 0..LCD_OVERSAMPLE {
                let sub = (first + p as i32) * LCD_OVERSAMPLE + stripe;
                let sum: u32 = LCD_FILTER
                    .iter()
                    .enumerate()
                    .map(|(k, w)| w * at(sub + k as i32 - reach))
                    .sum();
                let channel = if bgr { 2 - stripe } else { stripe } as usize;
                rgb[(row * width + p) * 3 + channel] = ((sum + 128) >> 8).min(255) as u8;
            }
        }
    }
    LcdMask {
        left: first,
        width,
        rgb,
    }
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

/// The sRGB transfer curve as a float, 0..1 → 0..1: linear light to its encoded value.
#[inline]
pub(crate) fn srgb_encode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The inverse of [`srgb_encode`]: an encoded value, 0..1, to linear light.
#[inline]
pub(crate) fn srgb_decode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The linear-light luminance of an encoded colour, 0..1 (sRGB primaries, Rec. 709
/// weights) — what a text blend compares and corrects by (ADR-611).
#[inline]
pub(crate) fn luminance(rgb: [u8; 3]) -> f32 {
    let lut = &*SRGB_TO_LINEAR;
    0.2126 * lut[rgb[0] as usize] + 0.7152 * lut[rgb[1] as usize] + 0.0722 * lut[rgb[2] as usize]
}

/// The alpha text is blended with IN LINEAR LIGHT (ADR-611): the one whose result has the
/// luminance a lerp of the ENCODED values at `cov` would have. `fg` and `bg` are the text's
/// and the ground's luminances, `fg_encoded` and `bg_encoded` the same through
/// [`srgb_encode`] (passed in because a caller blending a run of pixels has them already).
///
/// This is Ghostty's `linear-corrected` blend. A rasteriser's coverage is the fraction of a
/// pixel an outline covers, and every font is tuned for it lerped in the encoded space
/// (ADR-358: a plain linear-light lerp puts white-on-black at half coverage at 188/255, and
/// every rim glows). But an encoded lerp of two COLOURS darkens between them: the mix of a
/// red glyph's rim and a green ground is muddier than either. So the hue is mixed in linear
/// light, and the weight — the luminance — is the encoded lerp's. For a neutral pair the
/// two agree exactly; they differ only where hues differ.
///
/// The weight is then `text_contrast`'s to set: the caller lifts `cov` to `cov^(1/γ)` first
/// for text lighter than its ground, so γ is the perceptual gamma light-on-dark text is
/// weighted at — 1 the encoded lerp's weight, about 2.2 linear light's (a light stem keeps
/// its light whatever pixel phase it falls on).
#[inline]
pub(crate) fn text_alpha(cov: f32, fg: f32, bg: f32, fg_encoded: f32, bg_encoded: f32) -> f32 {
    if (fg - bg).abs() <= 0.001 {
        return cov;
    }
    let target = srgb_decode(fg_encoded * cov + bg_encoded * (1.0 - cov));
    ((target - bg) / (fg - bg)).clamp(0.0, 1.0)
}

/// Linear light (0..1) to an sRGB byte through a table fine enough to round as
/// [`linear_to_srgb`] does: the text blend encodes three channels per rim pixel.
static LINEAR_TO_SRGB_BYTE: std::sync::LazyLock<Box<[u8; LINEAR_STEPS + 1]>> =
    std::sync::LazyLock::new(|| {
        let mut table = Box::new([0u8; LINEAR_STEPS + 1]);
        for (index, slot) in table.iter_mut().enumerate() {
            *slot = linear_to_srgb(index as f32 / LINEAR_STEPS as f32) as u8;
        }
        table
    });
const LINEAR_STEPS: usize = 8191;

#[inline]
fn encode_byte(c: f32) -> u32 {
    LINEAR_TO_SRGB_BYTE[(c.clamp(0.0, 1.0) * LINEAR_STEPS as f32 + 0.5) as usize] as u32
}

/// One text colour's blend, ready for a run of pixels (ADR-611): [`text_alpha`]'s inputs
/// for the text, and the contrast curve to lift its coverage with where it is lighter than
/// the pixel it lands on (`None` at γ = 1). The ground's half is recomputed only when the
/// pixel under the glyph changes, which along a run of text is rarely.
pub(super) struct TextInk<'a> {
    fg: [u8; 3],
    fg_linear: [f32; 3],
    fg_luminance: f32,
    fg_encoded: f32,
    lift: Option<&'a [u8; 256]>,
    ground: std::cell::Cell<Option<(u32, [f32; 3], f32, f32)>>,
}

impl<'a> TextInk<'a> {
    pub(super) fn new(fg: [u8; 3], lift: Option<&'a [u8; 256]>) -> TextInk<'a> {
        let lut = &*SRGB_TO_LINEAR;
        let fg_luminance = luminance(fg);
        TextInk {
            fg,
            fg_linear: [
                lut[fg[0] as usize],
                lut[fg[1] as usize],
                lut[fg[2] as usize],
            ],
            fg_luminance,
            fg_encoded: srgb_encode(fg_luminance),
            lift,
            ground: std::cell::Cell::new(None),
        }
    }

    /// The ground pixel `dst` decoded: its channels in linear light, its luminance and that
    /// encoded.
    fn ground(&self, dst: u32) -> ([f32; 3], f32, f32) {
        if let Some((pixel, linear, lum, encoded)) = self.ground.get()
            && pixel == dst
        {
            return (linear, lum, encoded);
        }
        let lut = &*SRGB_TO_LINEAR;
        let linear = [
            lut[((dst >> 16) & 0xff) as usize],
            lut[((dst >> 8) & 0xff) as usize],
            lut[(dst & 0xff) as usize],
        ];
        let lum = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
        let encoded = srgb_encode(lum);
        self.ground.set(Some((dst, linear, lum, encoded)));
        (linear, lum, encoded)
    }

    /// The linear-light alpha for coverage `cov` over a ground of luminance `lum`.
    fn alpha(&self, cov: u8, lum: f32, encoded: f32) -> f32 {
        let cov = match self.lift {
            Some(lut) if self.fg_luminance > lum => lut[cov as usize],
            _ => cov,
        };
        text_alpha(
            cov as f32 / 255.0,
            self.fg_luminance,
            lum,
            self.fg_encoded,
            encoded,
        )
    }

    /// Composite the text over pixel `dst` at coverage `cov`.
    pub(super) fn over(&self, dst: u32, cov: u8) -> u32 {
        match cov {
            0 => dst,
            255 => pack(self.fg),
            _ => {
                let (ground, lum, encoded) = self.ground(dst);
                let a = self.alpha(cov, lum, encoded);
                let ch = |k: usize| encode_byte(self.fg_linear[k] * a + ground[k] * (1.0 - a));
                (ch(0) << 16) | (ch(1) << 8) | ch(2)
            }
        }
    }

    /// The subpixel sibling of [`TextInk::over`]: a coverage per colour channel (`cov[0]`
    /// red, `[1]` green, `[2]` blue), each corrected as a gray glyph's would be, so a gray
    /// and an LCD glyph side by side carry the same weight.
    pub(super) fn over_rgb(&self, dst: u32, cov: [u8; 3]) -> u32 {
        if cov == [0, 0, 0] {
            return dst;
        }
        if cov == [255, 255, 255] {
            return pack(self.fg);
        }
        let (ground, lum, encoded) = self.ground(dst);
        let ch = |k: usize| {
            let a = self.alpha(cov[k], lum, encoded);
            encode_byte(self.fg_linear[k] * a + ground[k] * (1.0 - a))
        };
        (ch(0) << 16) | (ch(1) << 8) | ch(2)
    }
}

/// Composite `fg` over `dst` at coverage `cov` as text: [`TextInk::over`] with no lift.
#[cfg(test)]
pub(super) fn blend_text(dst: u32, fg: [u8; 3], cov: u8) -> u32 {
    TextInk::new(fg, None).over(dst, cov)
}

/// [`TextInk::over_rgb`] with no lift.
#[cfg(test)]
pub(super) fn blend_text_rgb(dst: u32, fg: [u8; 3], cov: [u8; 3]) -> u32 {
    TextInk::new(fg, None).over_rgb(dst, cov)
}

/// The coverage curve for a text contrast exponent `gamma`: `cov -> 255·(cov/255)^(1/γ)`.
/// Identity at 1.0; above it, partial coverage is lifted (a stem's anti-aliased rim reads
/// fuller), which is the correction light-on-dark text wants under a linear-light blend.
/// Full and zero coverage are fixed points, so a glyph's interior and exterior never move.
pub(super) fn contrast_lut(gamma: f32) -> [u8; 256] {
    let mut lut = [0u8; 256];
    let inv = 1.0 / gamma.max(0.01);
    for (i, slot) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *slot = (c.powf(inv) * 255.0 + 0.5) as u8;
    }
    lut[0] = 0;
    lut[255] = 255;
    lut
}
