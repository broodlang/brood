//! Pictures for the display's `[:image …]` op (ADR-686): encoded bytes decoded ONCE per
//! content, and the geometry every painter shares — where a picture lands in its rect
//! (`place`), the picture resampled to the pixels it is drawn at (`ScaledPictures`), and the
//! half-block cells a terminal shows it in (`half_blocks`).
//!
//! A frame repaints often and a memoised fragment is not guaranteed, so the decode is cached
//! by CONTENT: the op is parsed every frame, the bytes are hashed (BLAKE3, no copy out of the
//! heap), and the decoded picture of bytes seen before comes back from [`DECODED`] as the same
//! `Arc`. Decoding stays bounded as `image-thumb`'s is: bytes nobody vouches for decode
//! within `IMAGE_MAX_SIDE` / `IMAGE_MAX_ALLOC` or not at all.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The largest side, in pixels, of an image a primitive will decode or accept.
pub const IMAGE_MAX_SIDE: u32 = 16384;
/// The largest buffer, in bytes, an image primitive will allocate for one image.
pub const IMAGE_MAX_ALLOC: u64 = 512 * 1024 * 1024;

/// Decode an encoded image (any format the `image` build reads: PNG, and JPEG / GIF /
/// WebP / BMP with `image-formats`) from bytes nobody vouches for — a file, the clipboard.
/// Per-call `Limits` bound a decompression bomb (`IMAGE_MAX_SIDE` a side, `IMAGE_MAX_ALLOC`
/// in all), and anything that is not a decodable image within them is `None`, never an
/// error.
pub fn decode_untrusted_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(IMAGE_MAX_SIDE);
    limits.max_image_height = Some(IMAGE_MAX_SIDE);
    limits.max_alloc = Some(IMAGE_MAX_ALLOC);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    reader.decode().ok()
}

/// A decoded picture: straight-alpha RGBA8 rows, and the content key it was decoded under.
pub struct Picture {
    pub key: [u8; 16],
    pub pixels: image::RgbaImage,
}

impl Picture {
    pub fn width(&self) -> u32 {
        self.pixels.width()
    }
    pub fn height(&self) -> u32 {
        self.pixels.height()
    }
}

/// A picture as an op holds it: shared, and equal to another exactly when their content
/// keys are — what the frame diff compares, so a frame repainting the same picture is
/// unchanged without a pixel compared.
#[derive(Clone)]
pub struct PictureRef(pub Arc<Picture>);

impl PartialEq for PictureRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0.key == other.0.key
    }
}

/// How a picture fills its rect (CSS's `object-fit`, whose words these are).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Fit {
    /// As large as fits, the aspect kept — scaled up or down.
    #[default]
    Contain,
    /// As `Contain`, but never larger than the picture's own size.
    ScaleDown,
    /// Stretched to the rect exactly, the aspect not kept.
    Fill,
    /// At its own size, one picture pixel to one device pixel, clipped to the rect.
    None,
}

/// Where in its rect a picture that does not fill it sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Anchor {
    /// Centred both ways (CSS's default `object-position`).
    #[default]
    Center,
    /// At the rect's top-left corner (Emacs's image-mode).
    TopLeft,
}

/// Where a `width`×`height` picture lands in a `box_w`×`box_h` rect under `fit` and
/// `anchor`: `(x, y, w, h)` relative to the rect's top-left, in the rect's pixels. The
/// drawn size may exceed the rect (`Fit::None`), and `x` / `y` may be negative — the
/// caller clips to the rect. A degenerate picture or rect places nothing (`None`).
pub fn place(
    fit: Fit,
    anchor: Anchor,
    width: u32,
    height: u32,
    box_w: u32,
    box_h: u32,
) -> Option<(i64, i64, u32, u32)> {
    if width == 0 || height == 0 || box_w == 0 || box_h == 0 {
        return None;
    }
    let (picture_w, picture_h) = (width as f64, height as f64);
    let scaled = |factor: f64| {
        (
            ((picture_w * factor).round() as u32).max(1),
            ((picture_h * factor).round() as u32).max(1),
        )
    };
    let fits = (box_w as f64 / picture_w).min(box_h as f64 / picture_h);
    let (drawn_w, drawn_h) = match fit {
        Fit::Contain => scaled(fits),
        Fit::ScaleDown => scaled(fits.min(1.0)),
        Fit::Fill => (box_w, box_h),
        Fit::None => (width, height),
    };
    let (x, y) = match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::Center => (
            (box_w as i64 - drawn_w as i64) / 2,
            (box_h as i64 - drawn_h as i64) / 2,
        ),
    };
    Some((x, y, drawn_w, drawn_h))
}

/// The most decoded pixels [`DECODED`] keeps, in bytes, before it lets the least recently
/// used go: a few dozen document pages.
const DECODED_BUDGET: usize = 256 * 1024 * 1024;

/// Pictures by content key, least recently used dropped past a byte budget.
struct PictureCache {
    entries: HashMap<[u8; 16], (Arc<Picture>, u64)>,
    bytes: usize,
    clock: u64,
    budget: usize,
}

impl PictureCache {
    fn new(budget: usize) -> Self {
        PictureCache {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
            budget,
        }
    }

    fn get(&mut self, key: &[u8; 16]) -> Option<Arc<Picture>> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(key).map(|(picture, used)| {
            *used = clock;
            picture.clone()
        })
    }

    fn insert(&mut self, picture: Arc<Picture>) {
        self.clock += 1;
        let size = picture.pixels.as_raw().len();
        if let Some((old, _)) = self
            .entries
            .insert(picture.key, (picture.clone(), self.clock))
        {
            self.bytes -= old.pixels.as_raw().len();
        }
        self.bytes += size;
        while self.bytes > self.budget && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(key, _)| **key != picture.key)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| *key)
            else {
                break;
            };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= gone.pixels.as_raw().len();
            }
        }
    }
}

/// Every picture an `[:image …]` op decoded, by content key — so a frame that names bytes
/// already seen hashes them and decodes nothing. Process-wide: the key fully determines the
/// picture (a pure cache).
static DECODED: Mutex<Option<PictureCache>> = Mutex::new(None);

fn with_decoded<T>(work: impl FnOnce(&mut PictureCache) -> T) -> T {
    let mut guard = DECODED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    work(guard.get_or_insert_with(|| PictureCache::new(DECODED_BUDGET)))
}

fn content_key(parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest.as_bytes()[..16]);
    key
}

/// The picture encoded `bytes` decode to — cached by their content, so the second frame
/// naming the same bytes decodes nothing — or `None` when they are not a decodable image
/// within the limits.
pub fn decoded(bytes: &[u8]) -> Option<Arc<Picture>> {
    let key = content_key(&[b"encoded", bytes]);
    if let Some(picture) = with_decoded(|cache| cache.get(&key)) {
        return Some(picture);
    }
    let pixels = decode_untrusted_image(bytes)?.to_rgba8();
    let picture = Arc::new(Picture { key, pixels });
    with_decoded(|cache| cache.insert(picture.clone()));
    Some(picture)
}

/// The picture of an already decoded `width`×`height` straight-alpha RGBA8 buffer (what
/// `gui/image-thumb` returns) — cached by content like [`decoded`]; `None` when the byte
/// count is not `width*height*4` or a side is out of range.
pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Arc<Picture>> {
    if width == 0
        || height == 0
        || width > IMAGE_MAX_SIDE
        || height > IMAGE_MAX_SIDE
        || rgba.len() as u64 != width as u64 * height as u64 * 4
    {
        return None;
    }
    let key = content_key(&[b"rgba", &width.to_le_bytes(), &height.to_le_bytes(), rgba]);
    if let Some(picture) = with_decoded(|cache| cache.get(&key)) {
        return Some(picture);
    }
    let pixels = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let picture = Arc::new(Picture { key, pixels });
    with_decoded(|cache| cache.insert(picture.clone()));
    Some(picture)
}

/// The largest picture a painter resamples to, in pixels: past it (a runaway rect) the op
/// paints nothing rather than allocate without bound.
pub const MAX_DRAWN_PIXELS: u64 = 8192 * 8192;

/// `picture` resampled to `width`×`height` — Catmull-Rom, which keeps the strokes of small
/// text distinct when a page is shrunk to a pane — or the picture itself at its own size.
pub fn resampled(picture: &Picture, width: u32, height: u32) -> image::RgbaImage {
    if picture.width() == width && picture.height() == height {
        return picture.pixels.clone();
    }
    image::imageops::resize(
        &picture.pixels,
        width,
        height,
        image::imageops::FilterType::CatmullRom,
    )
}

/// The resampled pictures one window paints with, by `(content key, width, height)`: a
/// window repainting a picture at the size it painted it last resamples nothing. Least
/// recently used dropped past a byte budget. Per window (the `Renderer` holds one), since
/// the sizes are that window's.
pub struct ScaledPictures {
    entries: HashMap<([u8; 16], u32, u32), (Arc<image::RgbaImage>, u64)>,
    bytes: usize,
    clock: u64,
}

/// What one window keeps resampled, in bytes.
const SCALED_BUDGET: usize = 128 * 1024 * 1024;

impl Default for ScaledPictures {
    fn default() -> Self {
        Self::new()
    }
}

impl ScaledPictures {
    pub fn new() -> Self {
        ScaledPictures {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
        }
    }

    /// `picture` at `width`×`height`, resampled the first time it is asked for.
    pub fn get(&mut self, picture: &Picture, width: u32, height: u32) -> Arc<image::RgbaImage> {
        self.clock += 1;
        let key = (picture.key, width, height);
        if let Some((scaled, used)) = self.entries.get_mut(&key) {
            *used = self.clock;
            return scaled.clone();
        }
        let scaled = Arc::new(resampled(picture, width, height));
        self.bytes += scaled.as_raw().len();
        self.entries.insert(key, (scaled.clone(), self.clock));
        while self.bytes > SCALED_BUDGET && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(entry, _)| **entry != key)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(entry, _)| *entry)
            else {
                break;
            };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= gone.as_raw().len();
            }
        }
        scaled
    }
}

/// Blend `channel` over `under` by `alpha` (0..255).
fn blend(channel: u8, under: u8, alpha: u8) -> u8 {
    ((channel as u32 * alpha as u32 + under as u32 * (255 - alpha as u32) + 127) / 255) as u8
}

/// One terminal cell of a picture: `(row, col)` relative to the op's rect, and its two
/// halves' colours, top and bottom — `None` for a half the picture leaves transparent.
pub type HalfBlock = (u16, u16, Option<[u8; 3]>, Option<[u8; 3]>);

/// The cells a terminal shows `picture` in, in a `cols`×`rows` rect: two pixels a cell, one
/// above the other (`▀` in the top's colour over the bottom's), the rect taken as `cols` ×
/// `2·rows` square pixels and the picture placed in it by `fit` and `anchor`. A partly
/// transparent pixel is blended over `under` (the terminal's background when known); a
/// wholly transparent one is `None`, and a cell with both halves `None` is left out, so what
/// is under it shows.
pub fn half_blocks(
    picture: &Picture,
    cols: u16,
    rows: u16,
    fit: Fit,
    anchor: Anchor,
    under: [u8; 3],
) -> Vec<HalfBlock> {
    let (box_w, box_h) = (cols as u32, rows as u32 * 2);
    let Some((x, y, width, height)) =
        place(fit, anchor, picture.width(), picture.height(), box_w, box_h)
    else {
        return Vec::new();
    };
    if width as u64 * height as u64 > MAX_DRAWN_PIXELS {
        return Vec::new();
    }
    let scaled = resampled(picture, width, height);
    let pixel = |box_x: i64, box_y: i64| -> Option<[u8; 3]> {
        let (picture_x, picture_y) = (box_x - x, box_y - y);
        if picture_x < 0 || picture_y < 0 || picture_x >= width as i64 || picture_y >= height as i64
        {
            return None;
        }
        let [red, green, blue, alpha] = scaled.get_pixel(picture_x as u32, picture_y as u32).0;
        if alpha == 0 {
            return None;
        }
        Some([
            blend(red, under[0], alpha),
            blend(green, under[1], alpha),
            blend(blue, under[2], alpha),
        ])
    };
    let mut cells = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let top = pixel(col as i64, row as i64 * 2);
            let bottom = pixel(col as i64, row as i64 * 2 + 1);
            if top.is_some() || bottom.is_some() {
                cells.push((row, col, top, bottom));
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Picture {
        Picture {
            key: [0; 16],
            pixels: image::RgbaImage::from_pixel(width, height, image::Rgba(rgba)),
        }
    }

    #[test]
    fn contain_fits_and_centres() {
        // a 200×100 picture in a 100×100 rect: 100×50, centred vertically
        assert_eq!(
            place(Fit::Contain, Anchor::Center, 200, 100, 100, 100),
            Some((0, 25, 100, 50))
        );
        // scaled up to fit
        assert_eq!(
            place(Fit::Contain, Anchor::TopLeft, 10, 10, 40, 80),
            Some((0, 0, 40, 40))
        );
    }

    #[test]
    fn scale_down_never_enlarges_and_none_keeps_its_size() {
        assert_eq!(
            place(Fit::ScaleDown, Anchor::TopLeft, 10, 10, 40, 80),
            Some((0, 0, 10, 10))
        );
        assert_eq!(
            place(Fit::None, Anchor::Center, 100, 100, 50, 50),
            Some((-25, -25, 100, 100))
        );
        assert_eq!(
            place(Fit::Fill, Anchor::Center, 3, 7, 50, 20),
            Some((0, 0, 50, 20))
        );
        assert_eq!(place(Fit::Contain, Anchor::Center, 0, 7, 50, 20), None);
    }

    #[test]
    fn the_same_bytes_decode_once() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode");
        let first = decoded(&png).expect("decodes");
        let second = decoded(&png).expect("decodes");
        assert!(Arc::ptr_eq(&first, &second), "the second is the cached one");
        assert_eq!((first.width(), first.height()), (3, 2));
        assert!(decoded(b"not an image").is_none());
    }

    #[test]
    fn half_blocks_take_two_pixels_a_cell_and_leave_transparency_out() {
        // 2×4 opaque red into 2 cols × 2 rows: four pixel rows, every cell both halves red
        let red = solid(2, 4, [255, 0, 0, 255]);
        let cells = half_blocks(&red, 2, 2, Fit::Contain, Anchor::TopLeft, [0, 0, 0]);
        assert_eq!(cells.len(), 4);
        assert!(
            cells.iter().all(
                |(_, _, top, bottom)| *top == Some([255, 0, 0]) && *bottom == Some([255, 0, 0])
            )
        );
        let clear = solid(2, 4, [255, 0, 0, 0]);
        assert!(half_blocks(&clear, 2, 2, Fit::Contain, Anchor::TopLeft, [0, 0, 0]).is_empty());
    }
}
