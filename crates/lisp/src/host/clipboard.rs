//! OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
//! `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
//! alive to answer paste requests, so a fresh handle per call would lose the copied text
//! the moment it dropped. Init failure (no display server) is cached as `None`, so the
//! builtins degrade to no-ops rather than retrying. Without the feature every entry
//! point is the no-op the builtins already promise, so nothing above needs a `cfg`.
//!
//! Images cross this seam as raw RGBA8 (row-major, `width * height * 4` bytes), which is
//! what arboard speaks; encoding to PNG and bounding the size is the builtin's job
//! (`builtins/clipboard.rs`), so it is testable without a display.

#[cfg(feature = "clipboard")]
mod enabled {
    use arboard::{Clipboard, ImageData};
    use std::sync::{Mutex, OnceLock};

    static CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();

    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CLIPBOARD
            .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }

    pub fn get_text() -> Option<String> {
        handle()?.lock().ok()?.get_text().ok()
    }

    pub fn set_text(s: &str) {
        if let Some(m) = handle() {
            if let Ok(mut cb) = m.lock() {
                let _ = cb.set_text(s.to_owned());
            }
        }
    }

    /// The clipboard's image as `(width, height, rgba8)`, or `None` when it holds no
    /// image arboard can decode (empty, text, a file list, a format it does not read).
    pub fn get_image() -> Option<(usize, usize, Vec<u8>)> {
        let image = handle()?.lock().ok()?.get_image().ok()?;
        Some((image.width, image.height, image.bytes.into_owned()))
    }

    /// Put an RGBA8 image on the clipboard; silently nothing when unavailable.
    pub fn set_image(width: usize, height: usize, rgba: Vec<u8>) {
        if let Some(mutex) = handle() {
            if let Ok(mut clipboard) = mutex.lock() {
                let _ = clipboard.set_image(ImageData {
                    width,
                    height,
                    bytes: rgba.into(),
                });
            }
        }
    }
}

#[cfg(feature = "clipboard")]
pub use enabled::{get_image, get_text, set_image, set_text};

/// No clipboard compiled in: nothing to read.
#[cfg(not(feature = "clipboard"))]
pub fn get_text() -> Option<String> {
    None
}

/// No clipboard compiled in: nothing to write to.
#[cfg(not(feature = "clipboard"))]
pub fn set_text(_s: &str) {}

/// No clipboard compiled in: no image to read.
#[cfg(not(feature = "clipboard"))]
pub fn get_image() -> Option<(usize, usize, Vec<u8>)> {
    None
}

/// No clipboard compiled in: nowhere to put an image.
#[cfg(not(feature = "clipboard"))]
pub fn set_image(_width: usize, _height: usize, _rgba: Vec<u8>) {}
