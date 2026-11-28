//! The image primitives: bytes nobody vouches for decoded into a bounded RGBA buffer, and
//! such a buffer written back as a PNG. Rendering one (half-block cells, a GUI texture) is
//! Brood policy over the buffer.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};
use super::bytes::collect_bytes;

/// The `{:width int :height int :rgba bytes}` buffer `%image-thumb` decodes into — a closed
/// shape, so a caller's `(:width thumb)` reads an `int` with nothing to declare.
fn decoded_image() -> crate::types::Ty {
    use super::signature_types::{bytes_ty, int};
    let fields = [("width", int), ("height", int), ("rgba", bytes_ty)]
        .into_iter()
        .map(|(name, ty)| (crate::core::value::intern(name), (ty, true)))
        .collect();
    crate::types::Ty::record_shape(fields, crate::types::Ty::of(crate::core::value::Tag::Nil))
}

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%image-thumb",
        Arity::exact(3),
        Sig::new(vec![any, int, int], decoded_image().union(nil_ty)),
        &["bytes", "max-w", "max-h"],
        "Decode an encoded image (PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit within max-w×max-h pixels (aspect ratio preserved), returning {:width :height :rgba} where :rgba is a width*height*4 bytes value (row-major RGBA8). nil when the bytes aren't a decodable image or the dims are non-positive. Per-call decode limits bound a decompression bomb. The one image primitive; rendering (half-block cells, a GUI texture) is Brood policy over the decoded buffer.",
        image_thumb);
    primitives.def(
        "%image-thumbs",
        Arity::exact(3),
        Sig::new(vec![seq_items_ty, int, int], vec_ty),
        &["sources", "max-w", "max-h"],
        "`%image-thumb` over every item of `sources` (a vector or list of byte sequences), the images decoded side by side on threads of their own: a vector of what `%image-thumb` answers for each, `{:width :height :rgba}` or nil, in `sources`' order. A bytes item is read where it lies, not copied. The same answers as `%image-thumb` one at a time.",
        image_thumbs);
    primitives.def(
        "%image-write-png",
        Arity::range(2, 3),
        Sig::with_optional(vec![map_ty, string], vec![map_ty.union(nil_ty)], string),
        &["image", "path", "opts"],
        "Write image — a {:width :height :rgba} map (what gui/image-thumb, gui/render-frame and gui/capture return; :rgba width*height*4 bytes) — to path as a PNG, and return path. opts: :crop [x y w h] keeps that rectangle (clipped to the image), :scale n (1..16) enlarges it n times by repeating pixels, so a few pixels of spacing can be looked at. Errors when :rgba is not width*height*4 bytes or the file cannot be written.",
        image_write_png);
}

// The limits and the bounded decode live with the display's pictures (ADR-686), which decode
// the same untrusted bytes.
pub(super) use crate::host::picture::{IMAGE_MAX_ALLOC, IMAGE_MAX_SIDE, decode_untrusted_image};

/// Downscale `image` to fit within `max_width`×`max_height`, aspect ratio preserved.
/// Downscale-only: a source already within the box keeps its native size (never
/// upscaled — `thumbnail`/`resize` would blow a small image up to fill the box). The
/// one resize path the image primitives share (`image-thumb`, `clipboard-image`).
pub(super) fn fit_within(
    image: image::DynamicImage,
    max_width: u32,
    max_height: u32,
) -> image::DynamicImage {
    if image.width() <= max_width && image.height() <= max_height {
        image
    } else {
        image.thumbnail(max_width, max_height)
    }
}

/// `(image-thumb bytes max-w max-h)` — decode an encoded image (PNG / JPEG / GIF /
/// WebP / BMP) from a byte sequence and downscale it to fit within `max-w`×`max-h`
/// pixels (aspect ratio preserved), returning `{:width :height :rgba}` where `:rgba`
/// is a `width*height*4` bytes value (row-major RGBA8). Returns nil when the bytes
/// aren't a decodable image or the dims are non-positive — untrusted input degrades
/// to "not an image" rather than throwing. Per-call decode `Limits` bound a
/// decompression bomb (≤ 16384² px, ≤ 512 MB alloc). The one image primitive;
/// rendering (half-block cells, a GUI texture, …) is Brood policy over this buffer.
pub(super) fn image_thumb(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let bytes = collect_bytes("gui/image-thumb", arg(args, 0), heap)?;
    let max_w = expect_int(heap, "gui/image-thumb", arg(args, 1))?;
    let max_h = expect_int(heap, "gui/image-thumb", arg(args, 2))?;
    if max_w <= 0 || max_h <= 0 {
        return Ok(Value::nil());
    }
    Ok(thumb_value(
        decoded_thumb(&bytes, max_w as u32, max_h as u32),
        heap,
    ))
}

/// `bytes` decoded and fitted within `max_width`×`max_height` as RGBA8, or None when they
/// are not a decodable image: what `image-thumb` and `image-thumbs` both work out, off the
/// heap, so it can run on any thread.
fn decoded_thumb(bytes: &[u8], max_width: u32, max_height: u32) -> Option<image::RgbaImage> {
    let decoded = decode_untrusted_image(bytes)?;
    Some(fit_within(decoded, max_width, max_height).to_rgba8())
}

/// The `{:width :height :rgba}` map for a decoded thumb, nil for none. The pixels become
/// the bytes value without a copy (`SharedBlob::from_owned_bytes`): a 4096² image is 64 MB.
fn thumb_value(thumb: Option<image::RgbaImage>, heap: &mut Heap) -> Value {
    let Some(thumb) = thumb else {
        return Value::nil();
    };
    let (width, height) = (thumb.width(), thumb.height());
    // GC-safe: no eval between this alloc and map_from_pairs (a builtin never fires
    // GC mid-execution), mirroring file_stat holding its string handles.
    let rgba = heap.alloc_bytes(crate::core::blob::SharedBlob::from_owned_bytes(
        thumb.into_raw(),
    ));
    let pairs = vec![
        (kw("width"), Value::int(width as i64)),
        (kw("height"), Value::int(height as i64)),
        (kw("rgba"), rgba),
    ];
    heap.map_from_pairs(pairs)
}

/// `(image-thumbs sources max-w max-h)` — `image-thumb` over every item of `sources`, the
/// images decoded side by side: a game's start decodes its model's textures one after
/// another otherwise, a second and more for a few large PNGs, where the work is all native
/// and independent. Spread over Brood processes (`seq/pmap`) each image's bytes and pixels
/// are copied across twice; here a bytes item is read where it lies (its blob is shared,
/// an `Arc`) and each image's pixels become its bytes value uncopied. At most as many
/// threads as the machine has cores, each taking the next image; decoding stays bounded per
/// image as `image-thumb`'s is. The answers are `image-thumb`'s, in `sources`' order.
pub(super) fn image_thumbs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::core::blob::SharedBlob;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let items = heap.seq_items(arg(args, 0))?;
    let max_w = expect_int(heap, "gui/image-thumbs", arg(args, 1))?;
    let max_h = expect_int(heap, "gui/image-thumbs", arg(args, 2))?;
    if max_w <= 0 || max_h <= 0 {
        return Ok(heap.alloc_vector(vec![Value::nil(); items.len()]));
    }
    let sources: Vec<Arc<SharedBlob>> = items
        .iter()
        .map(|item| match *item {
            Value::Bytes(id) => Ok(heap.bytes(id).clone()),
            other => Ok(SharedBlob::from_owned_bytes(collect_bytes(
                "gui/image-thumbs",
                other,
                heap,
            )?)),
        })
        .collect::<Result<_, LispError>>()?;
    let (max_width, max_height) = (max_w as u32, max_h as u32);
    let workers = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(sources.len());
    // (each answer put at its own index: a worker that panicked loses its images, which
    // then read as not decodable, as a bad image does, never as an error, and never moves
    // another image out of its place)
    let mut thumbs: Vec<Option<image::RgbaImage>> = Vec::new();
    thumbs.resize_with(sources.len(), || None);
    if workers <= 1 {
        for (index, source) in sources.iter().enumerate() {
            thumbs[index] = decoded_thumb(source.as_bytes(), max_width, max_height);
        }
    } else {
        // each worker takes the next image not yet taken, so a large one never holds up
        // the small ones queued behind it
        let next = AtomicUsize::new(0);
        let found: Vec<(usize, Option<image::RgbaImage>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    scope.spawn(|| {
                        let mut done = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(source) = sources.get(index) else {
                                break done;
                            };
                            done.push((
                                index,
                                decoded_thumb(source.as_bytes(), max_width, max_height),
                            ));
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap_or_default())
                .collect()
        });
        for (index, thumb) in found {
            thumbs[index] = thumb;
        }
    }
    let values: Vec<Value> = thumbs
        .into_iter()
        .map(|thumb| thumb_value(thumb, heap))
        .collect();
    Ok(heap.alloc_vector(values))
}

/// `(image-write-png image path [opts])` — encode a `{:width :height :rgba}` map as a
/// PNG at `path` (ADR-609): the other half of `gui/render-frame` and `gui/capture`, and
/// of `image-thumb` (whatever it decodes writes back). `opts` `:crop [x y w h]` keeps a
/// rectangle and `:scale n` enlarges it by repeating pixels — nearest neighbour, so a
/// one-pixel gap stays a sharp n-pixel one rather than a blur.
pub(super) fn image_write_png(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "gui/write-png";
    let Value::Map(image) = arg(args, 0) else {
        return Err(LispError::wrong_type(
            heap,
            who,
            "map {:width :height :rgba}",
            arg(args, 0),
        ));
    };
    let field = |name: &str| heap.map_get(image, kw(name)).unwrap_or(Value::Nil);
    let (width_value, height_value, rgba_value) = (field("width"), field("height"), field("rgba"));
    let width = expect_int(heap, who, width_value)?;
    let height = expect_int(heap, who, height_value)?;
    let rgba = collect_bytes("gui/write-png", rgba_value, heap)?;
    let path = expect_string(heap, who, arg(args, 1))?;
    if width <= 0 || height <= 0 || (width * height * 4) as usize != rgba.len() {
        return Err(LispError::runtime(format!(
            "{who}: :rgba is {} bytes, not {width}*{height}*4",
            rgba.len()
        )));
    }
    let Some(mut picture) = image::RgbaImage::from_raw(width as u32, height as u32, rgba) else {
        return Err(LispError::runtime(format!("{who}: not an image")));
    };
    if let Value::Map(opts) = arg(args, 2) {
        if let Some(Value::Vector(crop)) = heap.map_get(opts, kw("crop")) {
            let parts: Vec<i64> = heap
                .vector(crop)
                .iter()
                .map(|v| match v {
                    Value::Int(n) => *n,
                    _ => 0,
                })
                .collect();
            if let [x, y, w, h] = parts[..] {
                let (x, y) = (x.clamp(0, width - 1), y.clamp(0, height - 1));
                let (w, h) = (w.clamp(1, width - x), h.clamp(1, height - y));
                picture =
                    image::imageops::crop_imm(&picture, x as u32, y as u32, w as u32, h as u32)
                        .to_image();
            }
        }
        if let Some(Value::Int(scale)) = heap.map_get(opts, kw("scale")) {
            let scale = scale.clamp(1, 16) as u32;
            if scale > 1 {
                picture = image::imageops::resize(
                    &picture,
                    picture.width() * scale,
                    picture.height() * scale,
                    image::imageops::FilterType::Nearest,
                );
            }
        }
    }
    picture
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| LispError::runtime(format!("{who}: {path}: {e}")))?;
    Ok(arg(args, 1))
}
