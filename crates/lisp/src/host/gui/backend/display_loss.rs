//! The display going away (ADR-631): when the compositor or X server dies, every open
//! window's app is sent `[:hangup :display-lost]` once, a draw to the lost display becomes
//! a no-op, and Xlib's I/O error handler is replaced so it cannot `exit` the runtime.

use super::*;

// ---- the display going away (ADR-631) ---------------------------------------------------
// When the compositor or X server dies, the event loop cannot go on. Without this, an app
// was told nothing: on Wayland winit's `run_app` returned an error nobody read and the
// window's app waited for input forever; on X11 Xlib's default I/O error handler called
// `exit(1)` on the GUI thread, ending the whole runtime mid-turn. Either way an editor lost
// every unsaved buffer. Now every open window's app is sent `[:hangup :display-lost]` —
// the terminal's hang-up vocabulary (ADR-586), so `ui-run` runs the app's `:on-hangup` and
// ends the loop — and a draw to the lost display is a no-op, so the loop reaches it.

/// Set once the display connection is gone; never cleared (winit cannot restart a loop).
static DISPLAY_LOST: AtomicBool = AtomicBool::new(false);

/// Whether the display this runtime's windows were on is gone.
pub(super) fn display_lost() -> bool {
    DISPLAY_LOST.load(Ordering::SeqCst)
}

/// What every window's app is sent when its display goes away: `[:hangup :display-lost]`.
fn display_lost_message() -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("hangup")),
        Message::Keyword(value::intern("display-lost")),
    ])
}

/// The processes to tell: each open window's subscriber, once each, in pid order.
pub(super) fn lost_subscribers(windows: &HashMap<u64, WinHandle>) -> Vec<u64> {
    let mut pids: Vec<u64> = windows.values().map(|w| w.subscriber).collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// The display is gone (`why` says how): tell every open window's app, once. Safe from
/// any thread, including the GUI thread inside Xlib's I/O error handler.
pub fn lose_display(why: &str) {
    if DISPLAY_LOST.swap(true, Ordering::SeqCst) {
        return;
    }
    let pids = lost_subscribers(&lock(windows()));
    eprintln!(
        "brood: gui: the display is gone ({why}); {} window app(s) told [:hangup :display-lost]",
        pids.len()
    );
    for pid in pids {
        deliver(pid, display_lost_message());
    }
}

/// Xlib's I/O error handler — the X server connection is gone. Xlib calls `exit` when a
/// handler RETURNS, so this one does not: it tells the apps and parks the GUI thread for
/// good, leaving the rest of the runtime to save and end on its own terms.
#[cfg(all(unix, not(target_os = "macos")))]
extern "C" fn x_io_error(_display: *mut std::ffi::c_void) -> std::ffi::c_int {
    lose_display("the X server connection was lost");
    loop {
        std::thread::park();
    }
}

/// Install [`x_io_error`] if winit's X11 backend loaded libX11 (it opens it lazily, only on
/// an X11 session). Looked up with `RTLD_NOLOAD`, so a Wayland session loads nothing.
#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn install_x_io_error_handler() {
    type Handler = extern "C" fn(*mut std::ffi::c_void) -> std::ffi::c_int;
    for name in [c"libX11.so.6", c"libX11.so"] {
        // SAFETY: dlopen/dlsym with NUL-terminated names; RTLD_NOLOAD only returns a
        // handle to a library already loaded, which winit keeps loaded for the process.
        unsafe {
            let lib = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
            if lib.is_null() {
                continue;
            }
            let set = libc::dlsym(lib, c"XSetIOErrorHandler".as_ptr());
            if !set.is_null() {
                let set: extern "C" fn(Option<Handler>) -> Option<Handler> =
                    std::mem::transmute(set);
                set(Some(x_io_error));
            }
            return;
        }
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
pub(super) fn install_x_io_error_handler() {}
