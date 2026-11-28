//! The GUI threads' exit gate: held while a thread that paints (the GUI thread, the offscreen
//! painter) handles an event, and shut once at process exit — before a GPU driver's own C
//! exit handler tears its state down — so no queued frame is painted into a driver that is
//! going away.

use super::*;
use std::cell::Cell;
use std::sync::atomic::AtomicBool;

/// `std::process::exit` — `nest run --for` running out, a game quitting, any CLI exit —
/// runs the C exit handlers on its own thread while a painting thread may still be painting
/// a queued frame, and a GPU driver's handler tears its state down under it (NVIDIA's
/// `vkCreateBuffer` segfaulted in a `write_buffer`, every `nest run --for` on the GPU
/// target). `close_at_exit` shuts the gate before the driver's handler runs.
static EXIT_GATE: ExitGate = ExitGate::new();

thread_local! {
    /// This thread is inside an event of the gate's: an exit called from here waits for no one.
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

/// How long the exit waits for the event in hand to finish.
#[cfg(feature = "gui-gpu")]
const EVENT_IN_HAND_WAIT: Duration = Duration::from_secs(5);

/// The gate for one event: `None` once the process is exiting, when the event is dropped.
pub(super) fn enter_gate() -> Option<Entered<'static>> {
    EXIT_GATE.enter()
}

/// Inside the gate, for as long as it is kept.
pub(super) struct Entered<'gate> {
    _held: std::sync::MutexGuard<'gate, ()>,
}

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        HOLDING.with(|holding| holding.set(false));
    }
}

/// The exiting flag is read before an event starts, and the lock is held through the event.
/// The flag is what keeps a new event out: a painter with frames queued lets go of the lock
/// and takes it again at once for the next one, so an exit that waited to win the lock
/// first lost that race for as long as it waited, gave up, and the driver was torn down
/// under a painter still painting (the offscreen painter's SIGSEGVs at exit, 2026-11-28,
/// every one on `brood-offscreen`, at about 15 an hour with several sessions drawing).
struct ExitGate {
    exiting: AtomicBool,
    held: Mutex<()>,
}

impl ExitGate {
    const fn new() -> ExitGate {
        ExitGate {
            exiting: AtomicBool::new(false),
            held: Mutex::new(()),
        }
    }

    fn enter(&self) -> Option<Entered<'_>> {
        if self.exiting.load(Ordering::SeqCst) {
            return None;
        }
        let held = lock(&self.held);
        // The exit may have come while this thread waited for the lock.
        if self.exiting.load(Ordering::SeqCst) {
            return None;
        }
        HOLDING.with(|holding| holding.set(true));
        Some(Entered { _held: held })
    }

    /// Shut the gate: no event starts from now on, and the one in hand is waited out, for
    /// `wait` at most. False when it was still in hand at the end of `wait`.
    #[cfg(any(feature = "gui-gpu", test))]
    fn shut(&self, wait: Duration) -> bool {
        self.exiting.store(true, Ordering::SeqCst);
        // An exit called from inside an event (the GUI thread, mid-event) never returns to
        // it, and every other thread is kept out now.
        if HOLDING.with(Cell::get) {
            return true;
        }
        let deadline = Instant::now() + wait;
        loop {
            match self.held.try_lock() {
                Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => return true,
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(std::sync::TryLockError::WouldBlock) => return false,
            }
        }
    }
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
    // Bounded: an event blocked on something the exit stopped would hold the exit for ever.
    if !EXIT_GATE.shut(EVENT_IN_HAND_WAIT) {
        eprintln!(
            "brood gui: exiting while a frame is still being painted ({} s); the GPU driver may crash under it",
            EVENT_IN_HAND_WAIT.as_secs()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A painter with frames queued holds the gate nearly all the time, letting go of it only
    /// between one frame and the next: the exit still shuts the gate at once, and the painter
    /// paints nothing more.
    #[test]
    fn an_exit_shuts_the_gate_on_a_painter_that_is_never_idle() {
        let gate = Arc::new(ExitGate::new());
        let painter = {
            let gate = Arc::clone(&gate);
            std::thread::spawn(move || {
                let mut frames = 0u64;
                while let Some(_entered) = gate.enter() {
                    let painting = Instant::now() + Duration::from_millis(5);
                    while Instant::now() < painting {
                        std::hint::spin_loop();
                    }
                    frames += 1;
                }
                frames
            })
        };
        std::thread::sleep(Duration::from_millis(20));
        let started = Instant::now();
        assert!(
            gate.shut(Duration::from_secs(1)),
            "the painter kept the gate"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "shut took {:?}",
            started.elapsed()
        );
        assert!(painter.join().unwrap() > 0);
        assert!(gate.enter().is_none());
    }

    /// An exit from inside an event does not wait on itself.
    #[test]
    fn an_exit_from_inside_an_event_shuts_the_gate_at_once() {
        let gate = ExitGate::new();
        let entered = gate.enter().expect("open");
        let started = Instant::now();
        assert!(gate.shut(Duration::from_secs(2)));
        assert!(started.elapsed() < Duration::from_millis(100));
        drop(entered);
        assert!(gate.enter().is_none());
    }
}
