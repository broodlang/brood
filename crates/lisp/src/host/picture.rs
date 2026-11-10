//! Pictures for the display's `[:image …]` op (ADR-686): encoded bytes decoded ONCE per
//! content, and the geometry every painter shares — where a picture lands in its rect
//! (`place`), the picture resampled to the pixels it is drawn at (`ScaledPictures`), and the
//! half-block cells a terminal shows it in (`half_blocks`).
//!
//! A frame repaints often and a memoised fragment is not guaranteed, so the decode is cached
//! by CONTENT: the op is parsed every frame, the bytes are keyed by their BLAKE3 digest — which
//! the heap blob keeps once hashed (`SharedBlob::digest`), so the same bytes value repainted
//! is not hashed again — and the decoded picture of bytes seen before comes back from [`DECODED`] as the same
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
    pub pixels: Arc<image::RgbaImage>,
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
    decoded_by_digest(bytes, *blake3::hash(bytes).as_bytes())
}

/// [`decoded`] for a heap blob (a `Value::Bytes`): keyed by the digest the blob keeps
/// (`SharedBlob::digest`), so a frame naming the same bytes VALUE again — an app holding
/// its picture in its model — costs neither a hash nor a decode.
pub fn decoded_blob(blob: &crate::core::blob::SharedBlob) -> Option<Arc<Picture>> {
    decoded_by_digest(blob.as_bytes(), blob.digest())
}

fn decoded_by_digest(bytes: &[u8], digest: [u8; 32]) -> Option<Arc<Picture>> {
    let key = content_key(&[b"encoded", &digest]);
    if let Some(picture) = with_decoded(|cache| cache.get(&key)) {
        return Some(picture);
    }
    let pixels = decode_untrusted_image(bytes)?.to_rgba8();
    let picture = Arc::new(Picture {
        key,
        pixels: Arc::new(pixels),
    });
    with_decoded(|cache| cache.insert(picture.clone()));
    Some(picture)
}

/// The picture of an already decoded `width`×`height` straight-alpha RGBA8 buffer (what
/// `gui/image-thumb` returns) — cached by content like [`decoded`]; `None` when the byte
/// count is not `width*height*4` or a side is out of range.
pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Arc<Picture>> {
    from_rgba_by_digest(width, height, rgba, *blake3::hash(rgba).as_bytes())
}

/// [`from_rgba`] for a heap blob, keyed by the digest it keeps (see [`decoded_blob`]).
pub fn from_rgba_blob(
    width: u32,
    height: u32,
    rgba: &crate::core::blob::SharedBlob,
) -> Option<Arc<Picture>> {
    from_rgba_by_digest(width, height, rgba.as_bytes(), rgba.digest())
}

fn from_rgba_by_digest(
    width: u32,
    height: u32,
    rgba: &[u8],
    digest: [u8; 32],
) -> Option<Arc<Picture>> {
    if width == 0
        || height == 0
        || width > IMAGE_MAX_SIDE
        || height > IMAGE_MAX_SIDE
        || rgba.len() as u64 != width as u64 * height as u64 * 4
    {
        return None;
    }
    let key = content_key(&[
        b"rgba",
        &width.to_le_bytes(),
        &height.to_le_bytes(),
        &digest,
    ]);
    if let Some(picture) = with_decoded(|cache| cache.get(&key)) {
        return Some(picture);
    }
    let pixels = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let picture = Arc::new(Picture {
        key,
        pixels: Arc::new(pixels),
    });
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
        return (*picture.pixels).clone();
    }
    image::imageops::resize(
        &*picture.pixels,
        width,
        height,
        image::imageops::FilterType::CatmullRom,
    )
}

/// A picture at the size a painter asked for, or — while that size is still being resampled
/// off the painting thread — another size of it to stretch in the meantime.
pub enum Scaled {
    /// Exactly the asked size: drawn pixel for pixel.
    Exact(Arc<image::RgbaImage>),
    /// Another size of the same picture (the nearest one resampled before, else the decoded
    /// picture itself): the painter stretches it to the asked size, nearest neighbour, until
    /// the exact one lands and the window repaints.
    StandIn(Arc<image::RgbaImage>),
}

impl Scaled {
    pub fn image(&self) -> &Arc<image::RgbaImage> {
        match self {
            Scaled::Exact(image) | Scaled::StandIn(image) => image,
        }
    }
}

/// The resampled pictures one window paints with, by `(content key, width, height)`: a
/// window repainting a picture at the size it painted it last resamples nothing. Least
/// recently used dropped past a byte budget. Per window (the `Renderer` holds one), since
/// the sizes are that window's.
///
/// A live window resamples OFF its painting thread ([`resample_in_background`]): a size not
/// yet resampled is handed to a worker and painted from a stand-in until it lands (ADR-695),
/// so the first paint at a new size — every step of a window resize — costs no resample on
/// the GUI thread. The offscreen painter resamples in place, so what it paints is
/// deterministic.
///
/// [`resample_in_background`]: ScaledPictures::resample_in_background
pub struct ScaledPictures {
    entries: HashMap<([u8; 16], u32, u32), (Arc<image::RgbaImage>, u64)>,
    bytes: usize,
    clock: u64,
    background: Option<Background>,
}

/// The worker a window's resamples run on, and what it has been asked for.
struct Background {
    jobs: std::sync::mpsc::Sender<Resample>,
    landed: std::sync::mpsc::Receiver<Landed>,
    /// The size each picture was last asked for: one outstanding resample a picture, so a
    /// size asked for again while it is under way is not queued twice.
    pending: HashMap<[u8; 16], (u32, u32)>,
}

/// A picture to resample to `width`×`height`.
struct Resample {
    picture: Arc<Picture>,
    width: u32,
    height: u32,
}

/// A resample done: the picture's key, the size, the pixels.
type Landed = ([u8; 16], u32, u32, Arc<image::RgbaImage>);

/// What one window keeps resampled, in bytes.
const SCALED_BUDGET: usize = 128 * 1024 * 1024;

impl Default for ScaledPictures {
    fn default() -> Self {
        Self::new()
    }
}

impl ScaledPictures {
    /// Resampling in place, on the caller's thread: the offscreen painter's and every test's.
    pub fn new() -> Self {
        ScaledPictures {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
            background: None,
        }
    }

    /// From now on resample on a worker thread of this window's, calling `wake` each time a
    /// resample lands — the window then takes it ([`take_landed`](Self::take_landed)) and
    /// repaints. Requests that pile up behind a slow resample are coalesced: only the newest
    /// size of each picture is resampled (a resize drag asks for many). The worker ends
    /// when the window's `ScaledPictures` is dropped.
    pub fn resample_in_background(&mut self, wake: impl Fn() + Send + 'static) {
        let (jobs, inbox) = std::sync::mpsc::channel::<Resample>();
        let (done, landed) = std::sync::mpsc::channel::<Landed>();
        let spawned = std::thread::Builder::new()
            .name("picture-resample".into())
            .spawn(move || {
                while let Ok(first) = inbox.recv() {
                    let mut newest = vec![first];
                    while let Ok(next) = inbox.try_recv() {
                        newest.retain(|job| job.picture.key != next.picture.key);
                        newest.push(next);
                    }
                    for job in newest {
                        let scaled = Arc::new(resampled(&job.picture, job.width, job.height));
                        if done
                            .send((job.picture.key, job.width, job.height, scaled))
                            .is_err()
                        {
                            return;
                        }
                        wake();
                    }
                }
            });
        // no thread to be had: resample in place, as before
        if spawned.is_ok() {
            self.background = Some(Background {
                jobs,
                landed,
                pending: HashMap::new(),
            });
        }
    }

    /// `picture` at `width`×`height`: the one resampled before, the picture itself at its own
    /// size, or a resample: done now in place, or handed to the worker with a
    /// [`Scaled::StandIn`] to paint meanwhile.
    pub fn get(&mut self, picture: &Arc<Picture>, width: u32, height: u32) -> Scaled {
        self.clock += 1;
        let key = (picture.key, width, height);
        if let Some((scaled, used)) = self.entries.get_mut(&key) {
            *used = self.clock;
            return Scaled::Exact(scaled.clone());
        }
        if picture.width() == width && picture.height() == height {
            return Scaled::Exact(picture.pixels.clone());
        }
        let Some(background) = &mut self.background else {
            let scaled = Arc::new(resampled(picture, width, height));
            self.insert(key, scaled.clone());
            return Scaled::Exact(scaled);
        };
        if background.pending.get(&picture.key) != Some(&(width, height)) {
            background.pending.insert(picture.key, (width, height));
            let _ = background.jobs.send(Resample {
                picture: picture.clone(),
                width,
                height,
            });
        }
        Scaled::StandIn(self.stand_in(picture, width))
    }

    /// What to paint `picture` from until its resample at `width` lands: the size of it
    /// resampled before whose width is nearest, else the decoded picture.
    fn stand_in(&self, picture: &Picture, width: u32) -> Arc<image::RgbaImage> {
        self.entries
            .iter()
            .filter(|((key, _, _), _)| *key == picture.key)
            .min_by_key(|((_, other, _), _)| other.abs_diff(width))
            .map(|(_, (scaled, _))| scaled.clone())
            .unwrap_or_else(|| picture.pixels.clone())
    }

    /// Keep the resamples the worker finished: true when one did, and the window should
    /// repaint (a frame that drew a stand-in now has the exact size waiting).
    pub fn take_landed(&mut self) -> bool {
        let Some(background) = &mut self.background else {
            return false;
        };
        let landed: Vec<Landed> = background.landed.try_iter().collect();
        for (picture, width, height, _) in &landed {
            if background.pending.get(picture) == Some(&(*width, *height)) {
                background.pending.remove(picture);
            }
        }
        let any = !landed.is_empty();
        for (picture, width, height, scaled) in landed {
            self.clock += 1;
            self.insert((picture, width, height), scaled);
        }
        any
    }

    fn insert(&mut self, key: ([u8; 16], u32, u32), scaled: Arc<image::RgbaImage>) {
        self.bytes += scaled.as_raw().len();
        if let Some((replaced, _)) = self.entries.insert(key, (scaled, self.clock)) {
            self.bytes -= replaced.as_raw().len();
        }
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
            pixels: Arc::new(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba(rgba),
            )),
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
        // a heap blob of the same bytes finds the same picture, keyed by the digest it keeps
        let blob = crate::core::blob::SharedBlob::new(&png);
        let from_blob = decoded_blob(&blob).expect("decodes");
        assert!(
            Arc::ptr_eq(&first, &from_blob),
            "one key for bytes and blob"
        );
        assert_eq!(blob.digest(), *blake3::hash(&png).as_bytes());
    }

    // A live window's resample runs off its painting thread (ADR-695): the first ask for a
    // size paints a stand-in, the worker's result lands with a wake, and the next ask is
    // exact. The offscreen painter's (no worker) is exact at once.
    #[test]
    fn a_new_size_is_resampled_off_the_thread_and_a_stand_in_painted_meanwhile() {
        let picture = Arc::new(solid(40, 20, [9, 8, 7, 255]));
        let mut in_place = ScaledPictures::new();
        assert!(
            matches!(in_place.get(&picture, 20, 10), Scaled::Exact(image) if image.width() == 20)
        );

        let mut window = ScaledPictures::new();
        let (woke, wakes) = std::sync::mpsc::channel();
        window.resample_in_background(move || {
            let _ = woke.send(());
        });
        // at its own size: the picture itself, nothing to resample
        assert!(
            matches!(window.get(&picture, 40, 20), Scaled::Exact(image) if Arc::ptr_eq(&image, &picture.pixels))
        );
        // a new size: the decoded picture stands in
        match window.get(&picture, 20, 10) {
            Scaled::StandIn(image) => assert_eq!(image.width(), 40),
            Scaled::Exact(_) => panic!("resampled on the painting thread"),
        }
        wakes
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker woke the window");
        assert!(window.take_landed(), "what landed is kept");
        assert!(
            matches!(window.get(&picture, 20, 10), Scaled::Exact(image) if image.width() == 20)
        );
        // another new size: the nearest size resampled before stands in
        match window.get(&picture, 22, 11) {
            Scaled::StandIn(image) => assert_eq!(image.width(), 20),
            Scaled::Exact(_) => panic!("resampled on the painting thread"),
        }
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
