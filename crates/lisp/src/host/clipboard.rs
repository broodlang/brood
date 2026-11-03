//! OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
//! `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
//! alive to answer paste requests, so a fresh handle per call would lose the copied text
//! the moment it dropped. Init failure (no display server) is cached as `None`, so the
//! builtins degrade to no-ops rather than retrying. Without the feature every entry
//! point is the no-op the builtins already promise, so nothing above needs a `cfg`.
//!
//! READS go through a handle of their own, taken with `try_lock` ([`read_exclusive`]). A
//! read waits on whichever app owns the selection, and a wedged owner — a Wayland data
//! offer whose pipe is never written — holds the reader for as long as it likes. With one
//! shared handle that read held the WRITE side too: every later read, and every kill the
//! editor mirrors to the clipboard, queued behind it. Now a read that finds another still
//! waiting answers [`Busy`] at once, and writes, on the owner's handle, never wait on a read.
//!
//! Images cross this seam as raw RGBA8 (row-major, `width * height * 4` bytes), which is
//! what arboard speaks; encoding to PNG and bounding the size is the builtin's job
//! (`builtins/clipboard.rs`), so it is testable without a display.

/// A clipboard read refused because another is still waiting on the clipboard's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// Run `read` on the handle behind `lock` when no other read holds it, else [`Busy`] at
/// once — never a wait behind a read that may never end. A reader that panicked leaves
/// the handle usable.
#[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
fn read_exclusive<C, R>(
    lock: &std::sync::Mutex<C>,
    read: impl FnOnce(&mut C) -> R,
) -> Result<R, Busy> {
    match lock.try_lock() {
        Ok(mut handle) => Ok(read(&mut handle)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Ok(read(&mut poisoned.into_inner())),
        Err(std::sync::TryLockError::WouldBlock) => Err(Busy),
    }
}

#[cfg(feature = "clipboard")]
mod enabled {
    use super::{Busy, read_exclusive};
    use arboard::{Clipboard, ImageData};
    use std::sync::{Mutex, OnceLock};

    /// The selection owner: every write, and the text it serves to other apps.
    static CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    /// The reader: a read holds only this, so a wedged one blocks no write.
    static READER: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();

    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CLIPBOARD
            .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }

    fn reader() -> Option<&'static Mutex<Clipboard>> {
        READER
            .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }

    pub fn get_text() -> Result<Option<String>, Busy> {
        match reader() {
            None => Ok(None),
            Some(lock) => read_exclusive(lock, |clipboard| clipboard.get_text().ok()),
        }
    }

    pub fn set_text(s: &str) {
        if let Some(m) = handle()
            && let Ok(mut cb) = m.lock()
        {
            let _ = cb.set_text(s.to_owned());
        }
    }

    /// The clipboard's image as `(width, height, rgba8)`, or `None` when it holds no
    /// image arboard can decode (empty, text, a file list, a format it does not read).
    pub fn get_image() -> Result<Option<(usize, usize, Vec<u8>)>, Busy> {
        match reader() {
            None => Ok(None),
            Some(lock) => read_exclusive(lock, |clipboard| {
                clipboard
                    .get_image()
                    .ok()
                    .map(|image| (image.width, image.height, image.bytes.into_owned()))
            }),
        }
    }

    /// Put an RGBA8 image on the clipboard; silently nothing when unavailable.
    pub fn set_image(width: usize, height: usize, rgba: Vec<u8>) {
        if let Some(mutex) = handle()
            && let Ok(mut clipboard) = mutex.lock()
        {
            let _ = clipboard.set_image(ImageData {
                width,
                height,
                bytes: rgba.into(),
            });
        }
    }
}

#[cfg(feature = "clipboard")]
pub use enabled::{get_image, get_text, set_image, set_text};

/// No clipboard compiled in: nothing to read.
#[cfg(not(feature = "clipboard"))]
pub fn get_text() -> Result<Option<String>, Busy> {
    Ok(None)
}

/// No clipboard compiled in: nothing to write to.
#[cfg(not(feature = "clipboard"))]
pub fn set_text(_s: &str) {}

/// No clipboard compiled in: no image to read.
#[cfg(not(feature = "clipboard"))]
pub fn get_image() -> Result<Option<(usize, usize, Vec<u8>)>, Busy> {
    Ok(None)
}

/// No clipboard compiled in: nowhere to put an image.
#[cfg(not(feature = "clipboard"))]
pub fn set_image(_width: usize, _height: usize, _rgba: Vec<u8>) {}

#[cfg(test)]
mod tests {
    use super::{Busy, read_exclusive};
    use std::sync::{Arc, Mutex, mpsc};

    // A read whose owner never answers holds the reader for good. The next read must not
    // queue behind it: it is told Busy at once, and the first is still the only one waiting.
    #[test]
    fn a_read_behind_one_that_never_answers_is_busy_at_once() {
        let lock = Arc::new(Mutex::new(0u32));
        let (held_tx, held_rx) = mpsc::channel();
        let (_never_tx, never_rx) = mpsc::channel::<()>();
        let stand_in = Arc::clone(&lock);
        // the stand-in owner: takes the reader and never lets go (its sender is never used)
        std::thread::spawn(move || {
            let _ = read_exclusive(&stand_in, |_| {
                held_tx.send(()).unwrap();
                let _ = never_rx.recv();
            });
        });
        held_rx.recv().unwrap();
        let started = std::time::Instant::now();
        assert_eq!(read_exclusive(&lock, |value| *value), Err(Busy));
        assert!(started.elapsed() < std::time::Duration::from_millis(100));
    }

    // With no read in flight, a read runs.
    #[test]
    fn a_read_with_the_reader_free_runs() {
        let lock = Mutex::new(7u32);
        assert_eq!(read_exclusive(&lock, |value| *value + 1), Ok(8));
    }
}
