//! OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
//! `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
//! alive to answer paste requests, so a fresh handle per call would lose the copied text
//! the moment it dropped. Init failure (no display server) is cached as `None`, so the
//! builtins degrade to no-ops rather than retrying. Without the feature both entry
//! points are the no-op the builtins already promise, so nothing above needs a `cfg`.

#[cfg(feature = "clipboard")]
mod enabled {
    use arboard::Clipboard;
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
}

#[cfg(feature = "clipboard")]
pub use enabled::{get_text, set_text};

/// No clipboard compiled in: nothing to read.
#[cfg(not(feature = "clipboard"))]
pub fn get_text() -> Option<String> {
    None
}

/// No clipboard compiled in: nothing to write to.
#[cfg(not(feature = "clipboard"))]
pub fn set_text(_s: &str) {}
