//! The image primitives: bytes nobody vouches for decoded into a bounded RGBA buffer, and
//! such a buffer written back as a PNG. Rendering one (half-block cells, a GUI texture) is
//! Brood policy over the buffer.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, kw};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};
use super::bytes::{bytes_to_value, collect_bytes};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%image-thumb",
        Arity::exact(3),
        Sig::new(vec![any, int, int], map_ty.union(nil_ty)),
        &["bytes", "max-w", "max-h"],
        "Decode an encoded image (PNG/JPEG/GIF/WebP/BMP) from a byte sequence and downscale it to fit within max-w×max-h pixels (aspect ratio preserved), returning {:width :height :rgba} where :rgba is a width*height*4 bytes value (row-major RGBA8). nil when the bytes aren't a decodable image or the dims are non-positive. Per-call decode limits bound a decompression bomb. The one image primitive; rendering (half-block cells, a GUI texture) is Brood policy over the decoded buffer.",
        image_thumb);
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
    let Some(img) = decode_untrusted_image(&bytes) else {
        return Ok(Value::nil());
    };
    let thumb = fit_within(img, max_w as u32, max_h as u32).to_rgba8();
    let (w, h) = (thumb.width(), thumb.height());
    // GC-safe: no eval between this alloc and map_from_pairs (a builtin never fires
    // GC mid-execution), mirroring file_stat holding its string handles.
    let rgba = bytes_to_value(thumb.as_raw(), heap);
    let pairs = vec![
        (kw("width"), Value::int(w as i64)),
        (kw("height"), Value::int(h as i64)),
        (kw("rgba"), rgba),
    ];
    Ok(heap.map_from_pairs(pairs))
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
