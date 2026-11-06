//! WASM stub for the GUI builtins (`builtins/gui/native.rs`). The browser/headless wasm
//! runtime has no native window, so every `gui-*` primitive is present with its native
//! signature but errors at runtime. See `docs/wasm.md`.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

/// Every GUI builtin shares this arity; the stub reports the primitive is unavailable rather
/// than silently no-op'ing (so a mis-run GUI app fails loud).
macro_rules! wasm_unsupported_builtins {
    ($($name:ident),* $(,)?) => {
        $(
            pub(in crate::builtins) fn $name(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
                Err(LispError::runtime(format!(
                    "{}: GUI primitives are not available in the wasm runtime",
                    stringify!($name).replace('_', "-")
                )))
            }
        )*
    };
}

wasm_unsupported_builtins!(
    gui_open,
    gui_close,
    gui_title,
    gui_icon,
    gui_focus,
    gui_grab_cursor,
    gui_busy,
    gui_fullscreen,
    gui_minimize,
    gui_drag_move,
    gui_drag_resize,
    gui_maximize,
    gui_size,
    gui_size_px,
    gui_latency,
    gui_latency_histogram,
    gui_key_repeat,
    gui_key_repeats,
    gui_settle,
    gui_texture,
    gui_texture_free,
    gui_input,
    gui_sound,
    gui_sound_free,
    gui_cell_size,
    gui_held_key,
    gui_draw,
    gui_render_frame,
    gui_capture,
    gui_windows,
    gui_font,
    gui_inset,
    gui_bg,
    gui_fg,
    gui_line_height,
    gui_text_aa,
    gui_font_register,
);

/// `(%gui-compiled?)` — deliberately NOT one of the unsupported stubs above. It is the
/// predicate an app asks BEFORE a `gui/*` call it means to skip headlessly, so a version
/// of it that raises defeats its whole purpose: the caller would have to catch, which is
/// the very thing it exists to replace. On wasm the honest answer is simply `false`.
pub(in crate::builtins) fn gui_compiled_p(_: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(false))
}
