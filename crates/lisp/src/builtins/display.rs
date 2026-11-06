//! What the terminal and the window read alike from the display protocol (ADR-046): a frame's
//! ops, a face map, a colour, a coordinate, a key. One reading here means the two frontends
//! cannot disagree on what `:red` or an underline spec is; each then turns it into its own
//! style.

use crate::core::heap::Heap;
use crate::core::value::{self, Value};
use crate::error::LispError;

/// What a face map says, before a frontend makes its own style of it: the colours still as
/// values (a palette keyword means one thing to a terminal and another to a framebuffer).
pub(super) struct FaceFields {
    pub(super) fg: Option<Value>,
    pub(super) bg: Option<Value>,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) reverse: bool,
    pub(super) underline: Option<UnderlineSpec>,
}

/// A truthy `:underline`: a flag, a colour (`"#ff5f59"`, `:red`), or `{:color c :style :wave}`.
pub(super) struct UnderlineSpec {
    pub(super) colour: Option<Value>,
    pub(super) wave: bool,
}

/// Read a face map (`{:fg :red :bg :blue :bold true :reverse true}`); `None` for a non-map
/// face, which is the frontend's default style.
pub(super) fn read_face(heap: &Heap, face: Value) -> Option<FaceFields> {
    let Value::Map(id) = face else { return None };
    let k = &*FACE_KEYS;
    let flag = |key| heap.map_get(id, key).is_some_and(face_truthy);
    let underline = heap
        .map_get(id, k.underline)
        .filter(|v| face_truthy(*v))
        .map(|underline| match underline {
            Value::Map(spec) => UnderlineSpec {
                colour: heap.map_get(spec, k.color),
                wave: matches!(
                    (heap.map_get(spec, k.style), k.wave),
                    (Some(Value::Keyword(a)), Value::Keyword(b)) if a == b
                ),
            },
            Value::Bool(_) => UnderlineSpec {
                colour: None,
                wave: false,
            },
            colour => UnderlineSpec {
                colour: Some(colour),
                wave: false,
            },
        });
    Some(FaceFields {
        fg: heap.map_get(id, k.fg),
        bg: heap.map_get(id, k.bg),
        bold: flag(k.bold),
        italic: flag(k.italic),
        reverse: flag(k.reverse),
        underline,
    })
}

/// Parse a frame value (the op-vector `term-draw`/`term-emit`/`gui-draw` all
/// take) into `(tag, parts)` pairs: the frame must be a `Vector` (else a
/// `wrong_type` attributed to `who`); each op that is itself a `Vector` whose
/// first element is a `Keyword` yields `(that-keyword, the-op-parts)`; any op
/// that isn't a keyword-led vector is silently skipped (forward-compatible —
/// unknown ops are no-ops). This is the one extraction shared verbatim by the
/// three frame dispatchers; they deliberately *diverge downstream* (e.g. gui-draw
/// clamps coords at parse time, term-draw at use), so they must not drift on this
/// shared prologue — keep it here, in one place.
pub(in crate::builtins) fn frame_ops(
    heap: &Heap,
    frame: Value,
    who: &str,
    expected: &str,
) -> Result<Vec<(value::Symbol, Vec<Value>)>, LispError> {
    let ops: Vec<Value> = match frame {
        Value::Vector(id) => heap.vector(id).to_vec(),
        other => return Err(LispError::wrong_type(heap, who, expected, other)),
    };
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let parts: Vec<Value> = match op {
            Value::Vector(id) => heap.vector(id).to_vec(),
            _ => continue,
        };
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => continue,
        };
        out.push((tag, parts));
    }
    Ok(out)
}

/// The face-map keys (`:fg`/`:bg`/`:bold`/…) interned once for the whole process,
/// not re-interned per text op per frame on the render path — the same pre-intern
/// the frame dispatchers do for their op tags. Keyword interning is global and
/// append-only, so these stay valid for the process's life.
pub(super) struct FaceKeys {
    pub(super) fg: Value,
    pub(super) bg: Value,
    pub(super) bold: Value,
    pub(super) italic: Value,
    pub(super) underline: Value,
    pub(super) reverse: Value,
    pub(super) family: Value,
    pub(super) scale: Value,
    pub(super) color: Value,
    pub(super) style: Value,
    pub(super) wave: Value,
}
pub(super) static FACE_KEYS: std::sync::LazyLock<FaceKeys> =
    std::sync::LazyLock::new(|| FaceKeys {
        fg: value::kw("fg"),
        bg: value::kw("bg"),
        bold: value::kw("bold"),
        italic: value::kw("italic"),
        underline: value::kw("underline"),
        reverse: value::kw("reverse"),
        family: value::kw("family"),
        scale: value::kw("scale"),
        color: value::kw("color"),
        style: value::kw("style"),
        wave: value::kw("wave"),
    });

/// Brood truthiness for a face flag: only `nil`/`false` are falsy.
pub(in crate::builtins) fn face_truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// Clamp a Brood int to a terminal coordinate (crossterm uses `u16`).
///
/// The ceiling is `u16::MAX - 1`, not `u16::MAX`: crossterm's `MoveTo` writes
/// **1-based** ANSI coordinates, so it does `self.0 + 1` on the `u16` we hand it and
/// `65535` panics there with "attempt to add with overflow" under debug-assertions —
/// on a scheduler worker, where no `try` can catch it. One unreachable cell column is
/// not worth a reachable panic.
pub(in crate::builtins) fn clamp_u16(n: i64) -> u16 {
    n.clamp(0, u16::MAX as i64 - 1) as u16
}

/// The largest render *extent* (a `w`/`h` in cells) a frame op may ask for.
///
/// Coordinates go through `clamp_u16`, but an extent needs its own bound and for a
/// different reason: `w` sizes an *allocation* and `h` sizes a *loop*, so an
/// unbounded `i64` straight out of a frame is an allocator abort (SIGABRT, no crash
/// dump, uncatchable by Brood `try`) rather than a wrong pixel. `u16::MAX` on both
/// at once would still be a 4 GB fill, so the cap is the render bound instead: no
/// terminal is 4096 cells wide or tall, and nothing beyond the terminal renders, so
/// clamping here costs nothing real and turns a hostile frame into a no-op.
const MAX_EXTENT: i64 = 4096;

/// Clamp a Brood int to a drawable extent — see [`MAX_EXTENT`].
pub(in crate::builtins) fn clamp_extent(n: i64) -> u16 {
    n.clamp(0, MAX_EXTENT) as u16
}

/// A face colour keyword (`:red`, `:dark-grey`, …) to an RGB triple for the GUI
/// framebuffer. The same palette `color_of` maps to crossterm `Color`s, so the
/// two frontends agree on what `:red` looks like.
pub(in crate::builtins) fn color_rgb(v: Value) -> Option<[u8; 3]> {
    let Value::Keyword(s) = v else { return None };
    Some(match value::symbol_name(s).as_str() {
        "black" => [0x00, 0x00, 0x00],
        "red" => [0xcd, 0x31, 0x31],
        "green" => [0x0d, 0xbc, 0x79],
        "yellow" => [0xe5, 0xe5, 0x10],
        "blue" => [0x24, 0x72, 0xc8],
        "magenta" => [0xbc, 0x3f, 0xbc],
        "cyan" => [0x11, 0xa8, 0xcd],
        "white" => [0xe5, 0xe5, 0xe5],
        "grey" | "gray" => [0x80, 0x80, 0x80],
        "dark-grey" | "dark-gray" => [0x50, 0x50, 0x50],
        _ => return None,
    })
}

/// Resolve a face colour VALUE to an RGB triple — the one place every frontend
/// agrees on what a colour means. Accepts a palette keyword (`:red`, via
/// `color_rgb`), an explicit `[r g b]` vector (each channel clamped to 0..255), or
/// a `"#rgb"` / `"#rrggbb"` hex string. Anything else is `None` (the default face).
/// This is what lets a UI curate a soft RGB palette instead of the harsh
/// ANSI-16 keywords — and the `:vspans` fast path shares it too.
pub(in crate::builtins) fn face_rgb(heap: &Heap, v: Value) -> Option<[u8; 3]> {
    match v {
        Value::Keyword(_) => color_rgb(v),
        Value::Vector(id) => {
            let xs = heap.vector(id);
            if xs.len() == 3 {
                let chan = |k: usize| match xs[k] {
                    Value::Int(n) => n.clamp(0, 255) as u8,
                    _ => 0,
                };
                Some([chan(0), chan(1), chan(2)])
            } else {
                None
            }
        }
        Value::Str(id) => parse_hex_color(heap.string(id)),
        _ => None,
    }
}

/// Parse a `"#rgb"` or `"#rrggbb"` hex colour to an RGB triple. `None` for any
/// other shape (no leading `#`, a bad length, or a non-hex digit). The 3-digit
/// shorthand expands each nibble (`#f0a` → `[0xff 0x00 0xaa]`).
pub(in crate::builtins) fn parse_hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#')?;
    let b = h.as_bytes();
    match h.len() {
        3 => {
            let d = |i: usize| (b[i] as char).to_digit(16).map(|n| (n * 17) as u8);
            Some([d(0)?, d(1)?, d(2)?])
        }
        6 => {
            // Index the bytes (like the 3-nibble case) rather than `&h[i..i+2]`: a
            // 6-*byte* `h` can hold a multi-byte char (`#a€bc`), and slicing at a
            // non-char-boundary panics. A non-ASCII byte just fails `to_digit` → `None`.
            let d = |i: usize| (b[i] as char).to_digit(16);
            let p = |i: usize| Some((d(i)? * 16 + d(i + 1)?) as u8);
            Some([p(0)?, p(2)?, p(4)?])
        }
        _ => None,
    }
}

/// A held `gui::Key` as the same Brood value `gui-open` delivers for that press —
/// a 1-char string, else a `:ctrl-…` / `:alt-…` / `:ctrl-meta-…` / named keyword —
/// so an app can compare `(gui-held-key id)` directly against the key it last saw.
pub(in crate::builtins) fn gui_key_to_value(heap: &mut Heap, k: crate::host::gui::Key) -> Value {
    use crate::host::gui::Key;
    match k {
        Key::Char(c) => heap.alloc_string(&c.to_string()),
        Key::Named(s) => Value::keyword(value::intern(s)),
    }
}

/// The cursor style from a `[:cursor row col style]` op's optional 4th element: a
/// `:bar` / `:underline` / `:hollow` keyword, else (`:block`, nil, or anything unknown) the
/// default `Block`. Shared by both frontends so the caret shape agrees.
pub(in crate::builtins) fn cursor_style_from(v: Value) -> crate::host::gui::CursorStyle {
    use crate::host::gui::CursorStyle;
    match v {
        Value::Keyword(s) => match value::symbol_name(s).as_str() {
            "bar" => CursorStyle::Bar,
            "underline" => CursorStyle::Underline,
            "hollow" => CursorStyle::Hollow,
            _ => CursorStyle::Block,
        },
        _ => CursorStyle::Block,
    }
}

#[cfg(test)]
mod color_value_tests {
    use super::{face_rgb, parse_hex_color};
    use crate::core::heap::Heap;
    use crate::core::value::{self, Value};

    #[test]
    fn parses_six_and_three_digit_hex() {
        assert_eq!(parse_hex_color("#61afef"), Some([0x61, 0xaf, 0xef]));
        assert_eq!(parse_hex_color("#f0a"), Some([0xff, 0x00, 0xaa])); // nibble doubling
        assert_eq!(parse_hex_color("#000000"), Some([0, 0, 0]));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(parse_hex_color("61afef"), None); // no leading #
        assert_eq!(parse_hex_color("#12g456"), None); // non-hex digit
        assert_eq!(parse_hex_color("#1234"), None); // bad length
        assert_eq!(parse_hex_color("#"), None);
    }

    #[test]
    fn face_rgb_spans_keyword_vector_and_hex() {
        let mut heap = Heap::new();
        // a palette keyword still resolves via the shared path
        assert_eq!(face_rgb(&heap, value::kw("red")), Some([0xcd, 0x31, 0x31]));
        // an explicit vector, clamped to 0..255
        let v = heap.alloc_vector(vec![Value::int(300), Value::int(-5), Value::int(128)]);
        assert_eq!(face_rgb(&heap, v), Some([255, 0, 128]));
        // a hex string
        let s = heap.alloc_string("#282c34");
        assert_eq!(face_rgb(&heap, s), Some([0x28, 0x2c, 0x34]));
        // anything else is the default face
        assert_eq!(face_rgb(&heap, Value::int(7)), None);
    }
}
