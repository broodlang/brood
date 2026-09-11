//! The OS clipboard primitives the editor's kill/yank ride on. Mechanism is
//! `crate::host::clipboard`; both degrade to no-ops without a display or the feature.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::LispResult;

use super::numeric::{arg, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%clipboard-get",
        Arity::exact(0),
        Sig::nullary(any),
        &[],
        "The OS clipboard's text, or nil when empty / non-text / unavailable (no display server, or a build without the clipboard feature).",
        clipboard_get);
    primitives.def(
        "%clipboard-set",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "Copy string s to the OS clipboard so other apps can paste it; returns s. A no-op (still returns s) when no clipboard is available or the clipboard feature is off.",
        clipboard_set);
}

/// `(clipboard-get)` — the OS clipboard's text, or nil when it's empty / non-text /
/// unavailable (no display server, or a build without the `clipboard` feature). The
/// editor's yank consults this so text copied in another app pastes in.
pub(super) fn clipboard_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    match crate::host::clipboard::get_text() {
        Some(s) => Ok(heap.alloc_string(&s)),
        None => Ok(Value::nil()),
    }
}

/// `(clipboard-set s)` — copy string `s` to the OS clipboard so other apps can paste
/// it; returns `s` (so it threads). A no-op (still returns `s`) when no clipboard is
/// available or the `clipboard` feature is off, so callers needn't special-case headless
/// builds. The editor's kill/copy commands call this so a kill is system-wide.
pub(super) fn clipboard_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "clipboard-set", arg(args, 0))?;
    crate::host::clipboard::set_text(&s);
    Ok(arg(args, 0))
}
