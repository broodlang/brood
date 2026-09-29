//! The display round trip for clipboard images (ADR-392): `os/clipboard-set-image`, then
//! `os/clipboard-image`, through the Brood entry points a caller reaches.
//!
//! **Ignored by default, and a binary of its own, on purpose.** It writes the REAL,
//! process-global OS clipboard (clobbering whatever the user had copied), and two cases
//! doing that at once would read each other's image — so it must never run in a parallel
//! suite. Run it by name, on a machine with a display, with the feature on:
//!
//! ```text
//! cargo nextest run -p brood --features clipboard --test clipboard_display --run-ignored only
//! ```
//!
//! Without `DISPLAY` / `WAYLAND_DISPLAY` it says so and passes, the way the GUI dump tests
//! stand aside on a headless box. `tests/clipboard_test.blsp` covers the headless contract.
#![cfg(feature = "clipboard")]

use brood::Interp;
use image::ImageEncoder;

fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// A 4x2 PNG whose pixels are all distinct and whose alpha varies, so a flattened alpha or
/// a transposed row cannot pass for a round trip.
fn fixture_png() -> Vec<u8> {
    let (width, height) = (4u32, 2u32);
    let mut rgba = Vec::new();
    for row in 0..height {
        for column in 0..width {
            rgba.extend_from_slice(&[
                (column * 60) as u8,
                (row * 200) as u8,
                (column * 20 + row * 7) as u8,
                (255 - column * 50 - row * 10) as u8,
            ]);
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .expect("the fixture encodes");
    png
}

#[test]
#[ignore = "writes the real OS clipboard and needs a display; run by name (see the file header)"]
fn set_image_then_image_round_trips_through_the_os_clipboard() {
    if !has_display() {
        eprintln!("skipped: neither DISPLAY nor WAYLAND_DISPLAY is set");
        return;
    }
    let byte_list = fixture_png()
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    // Text goes on first, so an image left on the clipboard by an earlier run cannot pass
    // for this one — and text must read as no image. The comparison is of decoded pixels
    // (image-thumb at native size), not PNG bytes: the platform re-encodes, so equal
    // images need not be equal files.
    let program = format!(
        "(let (png (bytes {byte_list})
               pixels (fn (encoded) (get (gui/image-thumb encoded 100 100) :rgba)))
           (os/clipboard-set \"clipboard_display: text, not an image\")
           (let (text-read (os/clipboard-image))
             (os/clipboard-set-image png)
             (let (native (os/clipboard-image)
                   bounded (os/clipboard-image 2))
               [text-read
                (get native :width) (get native :height)
                (bytes? (get native :png))
                (= (pixels (get native :png)) (pixels png))
                (get bounded :width) (get bounded :height)])))"
    );
    let mut interp = Interp::new();
    let result = interp
        .eval_str(&program)
        .unwrap_or_else(|error| panic!("the round trip raised: {error}"));
    assert_eq!(
        interp.print(result),
        "[nil 4 2 true true 2 1]",
        "[text read as image, native width, height, :png is bytes, pixels equal, bounded width, height]"
    );
}
