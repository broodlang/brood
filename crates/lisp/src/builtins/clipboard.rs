//! The OS clipboard primitives: text (the editor's kill/yank ride on it) and images
//! (ADR-392, a pasted screenshot). Mechanism is `crate::host::clipboard`; every one
//! degrades to a no-op without a display or the feature. The image half encodes and
//! bounds here, not in the host module, so that part is testable without a display.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string};
use super::bytes::{bytes_to_value, collect_bytes};
use super::image::{IMAGE_MAX_ALLOC, IMAGE_MAX_SIDE, decode_untrusted_image, fit_within};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%clipboard-get",
        Arity::exact(0),
        Sig::nullary(any),
        &[],
        "The OS clipboard's text, or nil when empty / non-text / unavailable (no display server, or a build without the clipboard feature).",
        clipboard_get);
    primitives.def(
        "%clipboard-set",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "Copy string s to the OS clipboard so other apps can paste it; returns s. A no-op (still returns s) when no clipboard is available or the clipboard feature is off.",
        clipboard_set);
    primitives.def(
        "%clipboard-image",
        Arity::exact(1),
        Sig::new(vec![int.union(nil_ty)], map_or_nil),
        &["max-edge"],
        "The OS clipboard's image as {:png bytes :width w :height h} — PNG-encoded, alpha kept — or nil when the clipboard holds no image, is unreadable, no display is reachable, or the build has no clipboard feature. max-edge nil keeps the native size; an int downscales (never upscales) so the longer side is at most max-edge, and a non-positive one answers nil. A source over 16384 px a side or 512 MB of RGBA is refused as nil.",
        clipboard_image);
    primitives.def(
        "%clipboard-set-image",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], bytes_ty),
        &["png"],
        "Put an encoded image (PNG, or any format image-thumb decodes) on the OS clipboard; returns its argument. Raises when the bytes are not a decodable image within the image-thumb limits; a no-op otherwise when no clipboard is available or the clipboard feature is off.",
        clipboard_set_image);
    primitives.def(
        "%primary-get",
        Arity::exact(0),
        Sig::nullary(any),
        &[],
        "The PRIMARY selection's text (X11 / Wayland: what is highlighted, pasted by a middle click), or nil when empty / non-text / unavailable (not Linux, no display, a compositor without a primary selection, or a build without the clipboard feature). A blocking read like %clipboard-get, under the same rules. ADR-553.",
        primary_get);
    primitives.def(
        "%primary-set",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "Own the PRIMARY selection with string s, leaving the CLIPBOARD alone; returns s. A no-op (still returns s) where there is no primary selection or no clipboard feature. ADR-553.",
        primary_set);
}

/// `(%primary-get)` — the PRIMARY selection's text, or nil. Read through the same blocking
/// seam and reader as the clipboard ([`read_clipboard`]), since its owner is another app too.
pub(super) fn primary_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = read_clipboard(
        heap,
        "os/primary-get",
        "primary selection",
        crate::host::clipboard::get_primary_text,
    )?;
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::nil()),
    }
}

/// `(%primary-set s)` — own the PRIMARY selection with `s`; returns `s`. Waits on nobody,
/// like [`clipboard_set`].
pub(super) fn primary_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "os/primary-set", arg(args, 0))?;
    crate::host::clipboard::set_primary_text(&s);
    Ok(arg(args, 0))
}

/// `(clipboard-get)` — the OS clipboard's text, or nil when it's empty / non-text /
/// unavailable (no display server, or a build without the `clipboard` feature). The
/// editor's yank consults this so text copied in another app pastes in.
pub(super) fn clipboard_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = read_clipboard(
        heap,
        "os/clipboard-get",
        "text",
        crate::host::clipboard::get_text,
    )?;
    match text {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::nil()),
    }
}

/// A clipboard READ waits on another program: the selection's owner serialises what it
/// holds (an X11 conversion, a Wayland data-offer pipe), and a hung owner holds the reader
/// until the backend gives up. So it is a blocking call like `os/cmd` (ADR-517, ADR-536): a
/// responsive process is told of it or refused it, and the worker is marked dirty-blocked
/// for the wait, so a task reading the clipboard does not take a scheduler worker hostage.
/// Writing is not: it hands the text to this process's own selection owner and waits on
/// nobody. A read made while another is still waiting on the owner raises at once rather
/// than queueing behind it (`host::clipboard::Busy`): the one waiting may never end.
fn read_clipboard<T>(
    heap: &mut Heap,
    native: &'static str,
    what: &'static str,
    read: impl FnOnce() -> Result<T, crate::host::clipboard::Busy>,
) -> Result<T, LispError> {
    let blocking = crate::process::responsive::enter_blocking(heap, native, || what.into())?;
    let value = {
        let _dirty = crate::process::dirty_block();
        read()
    };
    if let Some(call) = blocking {
        call.report(heap);
    }
    value.map_err(|_| {
        LispError::runtime(format!(
            "{native}: another clipboard read is still waiting on the clipboard's owner"
        ))
    })
}

/// `(clipboard-set s)` — copy string `s` to the OS clipboard so other apps can paste
/// it; returns `s` (so it threads). A no-op (still returns `s`) when no clipboard is
/// available or the `clipboard` feature is off, so callers needn't special-case headless
/// builds. The editor's kill/copy commands call this so a kill is system-wide.
pub(super) fn clipboard_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "os/clipboard-set", arg(args, 0))?;
    crate::host::clipboard::set_text(&s);
    Ok(arg(args, 0))
}

/// A clipboard image made ready for Brood: PNG-encoded, with the dimensions of what the
/// PNG holds (after any downscale).
#[derive(Debug)]
struct EncodedImage {
    width: u32,
    height: u32,
    png: Vec<u8>,
}

/// Is a `width`×`height` RGBA8 image small enough to take from the clipboard? The
/// clipboard is untrusted input, bounded like `image-thumb`'s decode: at most
/// `IMAGE_MAX_SIDE` pixels a side and `IMAGE_MAX_ALLOC` bytes of RGBA, and not empty.
fn within_size_guard(width: usize, height: usize) -> bool {
    let side_limit = IMAGE_MAX_SIDE as usize;
    width > 0
        && height > 0
        && width <= side_limit
        && height <= side_limit
        && width as u64 * height as u64 * 4 <= IMAGE_MAX_ALLOC
}

/// RGBA8 (row-major, `width * height * 4` bytes) → a PNG, downscaled first when
/// `max_edge` is below the longer side. `None` for a source outside
/// `within_size_guard`, for a buffer whose length disagrees with its dimensions, and for
/// an encode failure. Alpha is kept, never flattened.
fn encode_png(
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    max_edge: Option<u32>,
) -> Option<EncodedImage> {
    use image::ImageEncoder;
    if !within_size_guard(width, height) || rgba.len() as u64 != width as u64 * height as u64 * 4 {
        return None;
    }
    let source = image::RgbaImage::from_raw(width as u32, height as u32, rgba)?;
    let mut image = image::DynamicImage::ImageRgba8(source);
    if let Some(edge) = max_edge {
        image = fit_within(image, edge, edge);
    }
    let pixels = image.into_rgba8();
    let (encoded_width, encoded_height) = pixels.dimensions();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(
            pixels.as_raw(),
            encoded_width,
            encoded_height,
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;
    Some(EncodedImage {
        width: encoded_width,
        height: encoded_height,
        png,
    })
}

/// `(clipboard-image max-edge)` — the clipboard's image as `{:png :width :height}`, or
/// nil (see the registration's docstring). `max-edge` nil means native size. Reading the
/// clipboard blocks while the selection owner serialises the image, so a caller with a
/// UI loop should call this from a worker process for large images.
pub(super) fn clipboard_image(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let max_edge = match arg(args, 0) {
        Value::Nil => None,
        bound => {
            let edge = expect_int(heap, "os/clipboard-image", bound)?;
            if edge <= 0 {
                return Ok(Value::nil());
            }
            Some(u32::try_from(edge).unwrap_or(u32::MAX))
        }
    };
    let image = read_clipboard(
        heap,
        "os/clipboard-image",
        "image",
        crate::host::clipboard::get_image,
    )?;
    let Some((width, height, rgba)) = image else {
        return Ok(Value::nil());
    };
    let Some(encoded) = encode_png(width, height, rgba, max_edge) else {
        return Ok(Value::nil());
    };
    // GC-safe: no eval between this alloc and map_from_pairs (a builtin never fires
    // GC mid-execution), as in `image_thumb`.
    let png = bytes_to_value(&encoded.png, heap);
    let keyword = |name: &'static str| Value::keyword(value::intern(name));
    let pairs = vec![
        (keyword("png"), png),
        (keyword("width"), Value::int(encoded.width as i64)),
        (keyword("height"), Value::int(encoded.height as i64)),
    ];
    Ok(heap.map_from_pairs(pairs))
}

/// `(clipboard-set-image png)` — decode `png` under the `image-thumb` limits and put
/// the pixels on the clipboard; returns `png`. Bytes that are not a decodable image are
/// the caller's mistake and raise; a missing clipboard is the environment and is a
/// silent no-op, as for `clipboard-set`.
pub(super) fn clipboard_set_image(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let encoded = collect_bytes("clipboard-set-image", arg(args, 0), heap)?;
    let Some(image) = decode_untrusted_image(&encoded) else {
        return Err(LispError::runtime(format!(
            "%clipboard-set-image: the {} bytes given are not a decodable image (PNG/JPEG/GIF/WebP/BMP, at most {} px a side)",
            encoded.len(),
            IMAGE_MAX_SIDE
        )));
    };
    let pixels = image.into_rgba8();
    let (width, height) = pixels.dimensions();
    crate::host::clipboard::set_image(width as usize, height as usize, pixels.into_raw());
    Ok(arg(args, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width`×`height` RGBA8 buffer whose every pixel is distinct and whose alpha
    /// varies, so a flattened alpha or a transposed row cannot pass for a round trip.
    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            for column in 0..width {
                rgba.extend_from_slice(&[
                    (column * 40) as u8,
                    (row * 60) as u8,
                    ((column + row) * 17) as u8,
                    255u8.wrapping_sub((column * 30) as u8),
                ]);
            }
        }
        rgba
    }

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("the encoder's output decodes as PNG")
            .into_rgba8()
    }

    #[test]
    fn encode_round_trips_pixels_and_alpha_at_native_size() {
        let rgba = gradient(5, 3);
        let encoded = encode_png(5, 3, rgba.clone(), None).expect("a small image encodes");
        assert_eq!((encoded.width, encoded.height), (5, 3));
        let decoded = decode(&encoded.png);
        assert_eq!(decoded.dimensions(), (5, 3));
        assert_eq!(
            decoded.into_raw(),
            rgba,
            "pixels (alpha included) survive the encode"
        );
    }

    #[test]
    fn max_edge_downscales_the_longer_side_and_keeps_the_aspect() {
        let encoded = encode_png(40, 20, gradient(40, 20), Some(10)).expect("encodes");
        assert_eq!((encoded.width, encoded.height), (10, 5));
        assert_eq!(
            decode(&encoded.png).dimensions(),
            (10, 5),
            ":width/:height describe the PNG"
        );
        // Tall images bound the height instead.
        let tall = encode_png(6, 30, gradient(6, 30), Some(15)).expect("encodes");
        assert_eq!((tall.width, tall.height), (3, 15));
    }

    #[test]
    fn max_edge_never_upscales() {
        let encoded = encode_png(5, 3, gradient(5, 3), Some(1000)).expect("encodes");
        assert_eq!((encoded.width, encoded.height), (5, 3));
        let exact = encode_png(5, 3, gradient(5, 3), Some(5)).expect("encodes");
        assert_eq!((exact.width, exact.height), (5, 3));
    }

    #[test]
    fn the_size_guard_bounds_each_side_and_the_total() {
        let side_limit = IMAGE_MAX_SIDE as usize;
        assert!(within_size_guard(side_limit, 1));
        assert!(within_size_guard(1, side_limit));
        assert!(!within_size_guard(side_limit + 1, 1), "one side too long");
        assert!(
            !within_size_guard(1, side_limit + 1),
            "the other side too long"
        );
        // Each side within the limit, the total not: 16384 x 16384 x 4 is 1 GiB. Half the
        // height is exactly the 512 MB budget, which is allowed; one row more is not.
        assert!(!within_size_guard(side_limit, side_limit));
        assert!(within_size_guard(side_limit, side_limit / 2));
        assert!(!within_size_guard(side_limit, side_limit / 2 + 1));
        assert!(!within_size_guard(0, 3), "an empty image");
    }

    #[test]
    fn encode_refuses_what_the_guard_refuses_and_malformed_buffers() {
        // A buffer of exactly the right length, so the side limit alone refuses it.
        let long = IMAGE_MAX_SIDE as usize + 1;
        assert!(encode_png(long, 1, vec![0; long * 4], Some(64)).is_none());
        // Exactly at the side limit on one axis is allowed (a 16384 x 1 strip).
        let side_limit = IMAGE_MAX_SIDE as usize;
        let strip =
            encode_png(side_limit, 1, vec![255; side_limit * 4], Some(64)).expect("at the limit");
        assert_eq!((strip.width, strip.height), (64, 1));
        // A buffer whose length disagrees with its dimensions.
        assert!(encode_png(2, 2, vec![0; 15], None).is_none());
        assert!(encode_png(2, 2, vec![0; 17], None).is_none());
    }
}
