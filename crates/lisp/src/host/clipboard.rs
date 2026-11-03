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

/// How long a read may hold the reader before the next one gives up on it: a read takes
/// milliseconds, and arboard's own X11 wait ends at four seconds, so a read past this is one
/// that may never end (a Wayland offer whose pipe is never written).
#[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// One reader handle, and since when a read has held it.
pub(crate) struct Reader<C> {
    handle: std::sync::Mutex<C>,
    since: std::sync::Mutex<Option<std::time::Instant>>,
}

/// Where the current reader lives: none until the first read makes one.
pub(crate) type ReaderSlot<C> = std::sync::Mutex<Option<std::sync::Arc<Reader<C>>>>;

/// The clipboard's READER, as kept in a [`ReaderSlot`]: one handle at a time, replaced when
/// a read has held it past a deadline. A read that never returns keeps its handle (it is
/// still inside arboard) — the handle is abandoned to it, and the next read gets a fresh
/// one, so a wedged owner costs one stuck thread rather than every read until a restart.
pub(crate) struct Readers<'slot, C>(pub(crate) &'slot ReaderSlot<C>);

#[cfg_attr(not(feature = "clipboard"), allow(dead_code))]
impl<C> Readers<'_, C> {
    /// The current reader — made by `make` when there is none (`None` when it cannot be,
    /// no display) — or, with `replacing` the reader a read is stuck in, a fresh one.
    fn reader(
        &self,
        make: &impl Fn() -> Option<C>,
        replacing: Option<&std::sync::Arc<Reader<C>>>,
    ) -> Option<std::sync::Arc<Reader<C>>> {
        let mut current = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stale = match (current.as_ref(), replacing) {
            (None, _) => true,
            (Some(now), Some(stuck)) => std::sync::Arc::ptr_eq(now, stuck),
            (Some(_), None) => false,
        };
        if stale {
            *current = make().map(|handle| {
                std::sync::Arc::new(Reader {
                    handle: std::sync::Mutex::new(handle),
                    since: std::sync::Mutex::new(None),
                })
            });
        }
        current.clone()
    }

    /// Run `read` on the reader: `Ok(None)` with no clipboard to read, [`Busy`] at once
    /// while another read holds it — never a wait behind one that may never end — and, once
    /// that other read has held it past `deadline`, on a fresh reader instead.
    pub(crate) fn read<R>(
        &self,
        make: impl Fn() -> Option<C>,
        deadline: std::time::Duration,
        read: impl FnOnce(&mut C) -> R,
    ) -> Result<Option<R>, Busy> {
        let Some(reader) = self.reader(&make, None) else {
            return Ok(None);
        };
        match Self::run(&reader, read) {
            Ok(value) => Ok(Some(value)),
            Err(read) => {
                let stuck = reader
                    .since
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_some_and(|since| since.elapsed() >= deadline);
                if !stuck {
                    return Err(Busy);
                }
                match self.reader(&make, Some(&reader)) {
                    None => Ok(None),
                    Some(fresh) => Self::run(&fresh, read).map(Some).map_err(|_| Busy),
                }
            }
        }
    }

    /// `read` on `reader` when no other read holds it (its start stamped while it runs), or
    /// `read` handed back untouched.
    fn run<R, F: FnOnce(&mut C) -> R>(reader: &Reader<C>, read: F) -> Result<R, F> {
        let mut handle = match reader.handle.try_lock() {
            Ok(handle) => handle,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Err(read),
        };
        let stamp = |value| {
            *reader
                .since
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = value
        };
        stamp(Some(std::time::Instant::now()));
        let value = read(&mut handle);
        stamp(None);
        Ok(value)
    }
}

#[cfg(feature = "clipboard")]
mod enabled {
    use super::{Busy, READ_DEADLINE, Reader, Readers};
    use arboard::{Clipboard, ImageData};
    use std::sync::Arc;
    use std::sync::{Mutex, OnceLock};

    /// The selection owner: every write, and the text it serves to other apps.
    static CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    /// The reader: a read holds only this, so a wedged one blocks no write — and is given
    /// up on past [`READ_DEADLINE`], so it blocks no later read for good either.
    static READERS: Mutex<Option<Arc<Reader<Clipboard>>>> = Mutex::new(None);

    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CLIPBOARD
            .get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }

    fn fresh_reader() -> Option<Clipboard> {
        Clipboard::new().ok()
    }

    pub fn get_text() -> Result<Option<String>, Busy> {
        Readers(&READERS)
            .read(fresh_reader, READ_DEADLINE, |clipboard| {
                clipboard.get_text().ok()
            })
            .map(Option::flatten)
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
        Readers(&READERS)
            .read(fresh_reader, READ_DEADLINE, |clipboard| {
                clipboard
                    .get_image()
                    .ok()
                    .map(|image| (image.width, image.height, image.bytes.into_owned()))
            })
            .map(Option::flatten)
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
    use super::{Busy, ReaderSlot, Readers};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    /// A reader slot of the test's own, for as long as the process lasts (a wedged read
    /// holds it): leaked rather than a `static`, so it is no process-wide table.
    fn slot() -> &'static ReaderSlot<u32> {
        Box::leak(Box::new(Mutex::new(None)))
    }

    /// Readers whose handles are numbered by `made`, the count of handles made so far.
    fn numbered(made: &Arc<AtomicU32>) -> impl Fn() -> Option<u32> + Send + 'static {
        let made = Arc::clone(made);
        move || Some(made.fetch_add(1, Ordering::SeqCst))
    }

    /// Start a read on `readers` that holds its handle and never returns (a stand-in owner
    /// that never answers), and wait until it holds it.
    fn wedge(slot: &'static ReaderSlot<u32>, made: &Arc<AtomicU32>) -> mpsc::Sender<()> {
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let make = numbered(made);
        std::thread::spawn(move || {
            let _ = Readers(slot).read(make, Duration::from_secs(60), |_| {
                held_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
        });
        held_rx.recv().unwrap();
        // the sender, kept by the caller; while it lives the read never returns
        release_tx
    }

    // A read whose owner never answers holds the reader. The next read does not queue
    // behind it: it is told Busy at once.
    #[test]
    fn a_read_behind_one_that_never_answers_is_busy_at_once() {
        let readers = slot();
        let made = Arc::new(AtomicU32::new(0));
        let _held = wedge(readers, &made);
        let started = std::time::Instant::now();
        assert_eq!(
            Readers(readers).read(numbered(&made), Duration::from_secs(60), |handle| *handle),
            Err(Busy)
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    // Once the wedged read has held the reader past the deadline it is given up on: the next
    // read runs on a fresh handle, and so do the reads after it — the stuck one no longer
    // costs every read until a restart.
    #[test]
    fn past_the_deadline_a_fresh_reader_replaces_a_wedged_one() {
        let readers = slot();
        let made = Arc::new(AtomicU32::new(0));
        let _held = wedge(readers, &made);
        std::thread::sleep(Duration::from_millis(60));
        let deadline = Duration::from_millis(50);
        assert_eq!(
            Readers(readers).read(numbered(&made), deadline, |handle| *handle),
            Ok(Some(1))
        );
        assert_eq!(
            Readers(readers).read(numbered(&made), deadline, |handle| *handle),
            Ok(Some(1))
        );
        assert_eq!(made.load(Ordering::SeqCst), 2);
    }

    // With no read in flight, a read runs on the one handle; with no clipboard to make a
    // handle from, it reads nothing.
    #[test]
    fn a_read_with_the_reader_free_runs() {
        let readers = Readers(slot());
        assert_eq!(
            readers.read(|| Some(7), Duration::from_secs(1), |handle| *handle + 1),
            Ok(Some(8))
        );
        let none = Readers(slot());
        assert_eq!(
            none.read(|| None, Duration::from_secs(1), |handle| *handle),
            Ok(None)
        );
    }
}
