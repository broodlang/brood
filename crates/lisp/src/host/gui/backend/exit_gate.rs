//! The GUI thread's exit gate: held while the thread handles an event, and shut once at
//! process exit — before a GPU driver's own C exit handler tears its state down — so no
//! queued frame is painted into a driver that is going away.

use super::*;

/// The GUI thread holds this while it handles an event; `true` once the process is exiting.
/// `std::process::exit` — `nest run --for` running out, a game quitting, any CLI exit —
/// runs the C exit handlers on its own thread while this one may still be painting a
/// queued frame, and a GPU driver's handler tears its state down under it (NVIDIA's
/// `vkCreateBuffer` segfaulted in a `write_buffer`, every `nest run --for` on the GPU
/// target). `close_at_exit` shuts the gate before the driver's handler runs.
static EXIT_GATE: Mutex<bool> = Mutex::new(false);

/// The gate for one event: `None` once the process is exiting, when the event is dropped.
pub(super) fn enter_gate() -> Option<std::sync::MutexGuard<'static, bool>> {
    let guard = lock(&EXIT_GATE);
    if *guard { None } else { Some(guard) }
}

/// Shut the gate when the process exits, waiting out the event in hand. C exit handlers
/// run last-registered first, so this is called once the GPU driver is loaded (a GPU
/// window's device made): it then runs before the driver's own teardown.
#[cfg(feature = "gui-gpu")]
pub(crate) fn close_at_exit() {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    // `atexit` is the C runtime's on Windows as on Unix.
    REGISTERED.call_once(|| {
        // SAFETY: `atexit` takes a plain `extern "C" fn()` that lives for the process.
        unsafe {
            libc::atexit(close_gate);
        }
    });
}

#[cfg(feature = "gui-gpu")]
extern "C" fn close_gate() {
    // Bounded: an exit called FROM the GUI thread, mid-event, holds the gate itself.
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match EXIT_GATE.try_lock() {
            Ok(mut exiting) => {
                *exiting = true;
                return;
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                *poisoned.into_inner() = true;
                return;
            }
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(std::sync::TryLockError::WouldBlock) => return,
        }
    }
}
