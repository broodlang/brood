//! The GUI font natives added by ADR-442, registered last for the intern-order reason
//! `editor_native` gives: `%gui-font-info` (where a family's faces came from — the file
//! an installed name resolved to, or that it was not found — and the fallback fonts
//! loaded so far) and `%gui-ligatures!` (shape punctuation runs together, so a
//! programming font's ligatures form). `%gui-font-register`'s `:family-name` form lives
//! with the rest of that primitive in `terminal/native.rs`.

use super::args::arg;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    primitives.def(
        "%gui-font-info",
        Arity::range(0, 1),
        Sig::with_optional(vec![], vec![Ty::of_tags(&[Tag::Keyword, Tag::Nil])], map_ty),
        &["family"],
        "Where font family `family` (a keyword; the window default when omitted or nil) got its faces, as a map: :name (the family shaping uses), :requested (the installed name a :family-name registration asked for, or nil), :origin (:bundled, :files, :system, or :not-found — asked for by name, not installed, the bundled DejaVu standing in), :regular / :bold / :italic / :bold-italic (the file each style resolved to), :fallbacks (a vector of {:name :file} — the installed fonts loaded so far to draw glyphs the primary font lacks, in the order first needed), :system-scanned (whether the installed fonts have been scanned yet: only on demand) and :ligatures. Answered on the GUI thread after every registration sent before it. Errors headless. Needs --features gui.",
        gui_font_info,
    );
    primitives.def(
        "%gui-ligatures!",
        Arity::exact(1),
        Sig::new(vec![any], nil_ty),
        &["on"],
        "Turn programming ligatures on (any truthy value) or off (nil / false, the default): with them on, a run of two or more punctuation chars in a :text op (-> != === <=> |> :: /* &&) is shaped as one unit so the font's ligatures form, still occupying exactly one cell per char — the grid never moves. Fonts without ligatures draw the same either way. Every open window and the default for ones opened later; a pure repaint. Needs --features gui. Returns nil.",
        gui_ligatures,
    );
}

/// `%gui-headless?` (ADR-557), registered after every other primitive for the intern-order
/// reason in the module doc.
pub(super) fn register_headless(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%gui-headless?",
        Arity::exact(0),
        Sig::new(vec![], bool_ty),
        &[],
        "True when this runtime's GUI is HEADLESS (BROOD_GUI_HEADLESS set in a --features gui build): windows open as fakes with a fixed grid and draw nothing, and there is no font engine — so %gui-font-info and %gui-cell-size error. False in a windowed runtime, and without --features gui (where %gui-compiled? is the question).",
        gui_headless,
    );
}

/// `(%gui-headless?)` — see the registration; `host::gui::headless`.
fn gui_headless(_args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::boolean(crate::host::gui::headless()))
}

/// `(%gui-font-info [family])` — see the registration.
fn gui_font_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let family = match arg(args, 0) {
        Value::Nil => None,
        Value::Keyword(name) => Some(name),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-font-info",
                "a family keyword or nil",
                other,
            ));
        }
    };
    let info = crate::host::gui::font_info(family).map_err(LispError::runtime)?;
    let string_or_nil = |heap: &mut Heap, text: &str| {
        if text.is_empty() {
            Value::nil()
        } else {
            heap.alloc_string(text)
        }
    };
    let mut fallbacks = Vec::with_capacity(info.fallbacks.len());
    for (name, file) in &info.fallbacks {
        let name_value = heap.alloc_string(name);
        let file_value = heap.alloc_string(file);
        fallbacks.push(heap.map_from_pairs(vec![
            (value::kw("name"), name_value),
            (value::kw("file"), file_value),
        ]));
    }
    let fallbacks = heap.alloc_vector(fallbacks);
    let name = heap.alloc_string(&info.name);
    let requested = match &info.requested {
        Some(text) => heap.alloc_string(text),
        None => Value::nil(),
    };
    let [regular, bold, italic, bold_italic] = &info.faces;
    let regular = string_or_nil(heap, regular);
    let bold = string_or_nil(heap, bold);
    let italic = string_or_nil(heap, italic);
    let bold_italic = string_or_nil(heap, bold_italic);
    Ok(heap.map_from_pairs(vec![
        (value::kw("name"), name),
        (value::kw("requested"), requested),
        (value::kw("origin"), value::kw(info.origin)),
        (value::kw("regular"), regular),
        (value::kw("bold"), bold),
        (value::kw("italic"), italic),
        (value::kw("bold-italic"), bold_italic),
        (value::kw("fallbacks"), fallbacks),
        (
            value::kw("system-scanned"),
            Value::boolean(info.system_scanned),
        ),
        (value::kw("ligatures"), Value::boolean(info.ligatures)),
    ]))
}

/// `(%gui-ligatures! on)` — see the registration. GUI only; returns nil.
fn gui_ligatures(args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    let on = !matches!(arg(args, 0), Value::Nil | Value::Bool(false));
    crate::host::gui::ligatures(on).map_err(LispError::runtime)?;
    Ok(Value::nil())
}
