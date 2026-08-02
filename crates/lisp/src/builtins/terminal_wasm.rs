//! WASM stub for the terminal + GUI builtins (`builtins/terminal.rs`).
//!
//! The browser/headless wasm runtime has no TTY (crossterm) and no native window
//! (winit/gui), so every `term-*` / `gui-*` / `audio-beep` primitive is present
//! with its native signature but errors at runtime. The registration block in
//! `builtins/mod.rs` and the `restore_*` callers (io/system/cli) stay unchanged.
//! Interactive display isn't part of the in-browser playground. See `docs/wasm.md`.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

/// Every terminal/GUI builtin shares this arity; the stub reports the primitive is
/// unavailable rather than silently no-op'ing (so a mis-run TUI/GUI app fails loud).
macro_rules! wasm_unsupported_builtins {
    ($($name:ident),* $(,)?) => {
        $(
            pub(super) fn $name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
                Err(LispError::runtime(format!(
                    "{}: terminal/GUI primitives are not available in the wasm runtime",
                    stringify!($name).replace('_', "-")
                )))
            }
        )*
    };
}

wasm_unsupported_builtins!(
    term_enter,
    term_leave,
    term_size,
    term_poll,
    term_draw,
    term_raw_enter,
    term_raw_leave,
    term_emit,
    audio_beep,
    gui_open,
    gui_close,
    gui_title,
    gui_icon,
    gui_focus,
    gui_grab_cursor,
    gui_fullscreen,
    gui_minimize,
    gui_drag_move,
    gui_drag_resize,
    gui_maximize,
    gui_size,
    gui_held_key,
    gui_draw,
    gui_font,
    gui_inset,
    gui_bg,
    gui_font_register,
);

// Terminal-restore hooks called by always-compiled code (io/system/cli). On wasm
// there is no terminal to restore, so these are no-ops.
pub fn restore_raw() {}
pub fn restore_terminal() {}
pub fn restore_terminal_on_exit() {}
