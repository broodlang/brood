//! WASM stub for the terminal builtins (`builtins/terminal/native.rs`).
//!
//! The browser/headless wasm runtime has no TTY (crossterm) and no native window
//! (winit/gui), so every `term-*` / `audio-beep` primitive is present
//! with its native signature but errors at runtime. The registration table in
//! `builtins/terminal.rs` is shared with the native implementation, so the two cannot
//! drift; the `restore_*` callers (io/evaluation/cli) stay unchanged.
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
    term_held_key,
    term_forward_input,
    term_draw,
    term_invalidate,
    term_colors,
    term_bg,
    term_suspend,
    term_refresh,
    term_repaint_due,
    term_raw_enter,
    term_raw_leave,
    term_emit,
    audio_beep,
);

// Terminal-restore hooks called by always-compiled code (io/system/cli). On wasm
// there is no terminal to restore, so these are no-ops.
pub fn restore_raw() {}
pub fn restore_terminal() {}
pub fn restore_terminal_on_exit() {}
