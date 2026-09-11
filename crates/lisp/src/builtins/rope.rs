//! The rope primitives (ADR-045): the `ropey`-backed editor buffer text — O(log n)
//! insert/delete/slice with line and char indexing. `std/editor/buffer.blsp` is the
//! immutable-buffer policy over these `%rope-*` names.

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};
use super::numeric::{expect_rope, expect_rope_ref};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // rope — the editor buffer's text storage (ADR-045). The irreducible text
    // mechanism: a `ropey::Rope` gives O(log n) edits + char/line indexing that
    // Brood can't bootstrap over flat strings. Immutable like every value —
    // `rope-insert`/`rope-delete` return a *fresh* rope (cheap structural share).
    // Points, marks, regions, search, the buffer process itself: all Brood above.
    primitives.def(
        "%string->rope",
        Arity::exact(1),
        Sig::new(vec![string], rope),
        &["s"],
        "A rope (editor buffer text) holding the characters of string s.",
        string_to_rope,
    );
    primitives.def(
        "%rope->string",
        Arity::exact(1),
        Sig::new(vec![rope], string),
        &["r"],
        "The full text of rope r as a string.",
        rope_to_string,
    );
    primitives.def(
        "%rope-length",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        &["r"],
        "The number of characters in rope r.",
        rope_length,
    );
    primitives.def(
        "%rope-line-count",
        Arity::exact(1),
        Sig::new(vec![rope], int),
        &["r"],
        "The number of lines in rope r (a trailing newline ends a line; \"\" is 1 line).",
        rope_line_count,
    );
    primitives.def(
        "%rope-insert",
        Arity::exact(3),
        Sig::new(vec![rope, int, string], rope),
        &["r", "idx", "s"],
        "A fresh rope with string s inserted at character index idx.",
        rope_insert,
    );
    primitives.def(
        "%rope-delete",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], rope),
        &["r", "start", "end"],
        "A fresh rope with characters [start, end) removed.",
        rope_delete,
    );
    primitives.def(
        "%rope-slice",
        Arity::exact(3),
        Sig::new(vec![rope, int, int], string),
        &["r", "start", "end"],
        "The text of characters [start, end) of rope r, as a string.",
        rope_slice,
    );
    primitives.def(
        "%rope-line",
        Arity::exact(2),
        Sig::new(vec![rope, int], string),
        &["r", "n"],
        "The text of line n (0-based) of rope r, including any trailing newline.",
        rope_line,
    );
    primitives.def(
        "%rope-char->line",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        &["r", "idx"],
        "The 0-based line index containing character idx.",
        rope_char_to_line,
    );
    primitives.def(
        "%rope-line->char",
        Arity::exact(2),
        Sig::new(vec![rope, int], int),
        &["r", "n"],
        "The character index where line n (0-based) begins.",
        rope_line_to_char,
    );
}

// ---------- rope (editor buffer text — ADR-045) ----------
//
// All indices are **character** indices (matching the language's char-based
// string indexing), not bytes. Edits return a *fresh* rope (immutability):
// ropey clones share structure, so `clone()`-then-edit only copies touched
// B-tree nodes. Out-of-range indices raise a clean E-code error rather than
// letting ropey panic.

/// Raise a uniform out-of-range error attributed to `who`.
pub(super) fn rope_oob(who: &str, what: &str, got: i64, max: usize) -> LispError {
    LispError::runtime(format!(
        "{}: {} {} out of bounds (valid 0..={})",
        who, what, got, max
    ))
    .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
}

/// `(string->rope s)` — a rope holding the text of string `s`.
pub(super) fn string_to_rope(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "%string->rope", arg(args, 0))?;
    Ok(heap.alloc_rope(ropey::Rope::from_str(&s)))
}

/// `(rope->string r)` — the full text of rope `r` as a string.
pub(super) fn rope_to_string(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope->string", arg(args, 0))?;
    Ok(heap.alloc_string(&r.to_string()))
}

/// `(rope-length r)` — the number of characters in `r`.
pub(super) fn rope_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-length", arg(args, 0))?;
    Ok(Value::int(r.len_chars() as i64))
}

/// `(rope-line-count r)` — the number of lines in `r` (ropey counts a trailing
/// newline as ending a line, so `"a\n"` is 2 lines and `""` is 1).
pub(super) fn rope_line_count(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line-count", arg(args, 0))?;
    Ok(Value::int(r.len_lines() as i64))
}

/// `(rope-insert r idx s)` — a fresh rope with string `s` inserted at character
/// index `idx` (0..=length).
pub(super) fn rope_insert(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "%rope-insert", arg(args, 0))?;
    let idx = expect_int(heap, "%rope-insert", arg(args, 1))?;
    let s = expect_string(heap, "%rope-insert", arg(args, 2))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("%rope-insert", "index", idx, len));
    }
    r.insert(idx as usize, &s);
    Ok(heap.alloc_rope(r))
}

/// `(rope-delete r start end)` — a fresh rope with characters `[start, end)`
/// removed.
pub(super) fn rope_delete(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut r = expect_rope(heap, "%rope-delete", arg(args, 0))?;
    let start = expect_int(heap, "%rope-delete", arg(args, 1))?;
    let end = expect_int(heap, "%rope-delete", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("%rope-delete", "range end", end, len));
    }
    r.remove(start as usize..end as usize);
    Ok(heap.alloc_rope(r))
}

/// `(rope-slice r start end)` — the text of characters `[start, end)` as a string.
pub(super) fn rope_slice(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-slice", arg(args, 0))?;
    let start = expect_int(heap, "%rope-slice", arg(args, 1))?;
    let end = expect_int(heap, "%rope-slice", arg(args, 2))?;
    let len = r.len_chars();
    if start < 0 || end < start || end as usize > len {
        return Err(rope_oob("%rope-slice", "range end", end, len));
    }
    let s = r.slice(start as usize..end as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-line r n)` — the text of line `n` (0-based), including its trailing
/// newline if present. The viewport-rendering primitive.
pub(super) fn rope_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line", arg(args, 0))?;
    let n = expect_int(heap, "%rope-line", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize >= lines {
        return Err(rope_oob("%rope-line", "line", n, lines.saturating_sub(1)));
    }
    let s = r.line(n as usize).to_string();
    Ok(heap.alloc_string(&s))
}

/// `(rope-char->line r idx)` — the 0-based line index containing character `idx`.
pub(super) fn rope_char_to_line(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-char->line", arg(args, 0))?;
    let idx = expect_int(heap, "%rope-char->line", arg(args, 1))?;
    let len = r.len_chars();
    if idx < 0 || idx as usize > len {
        return Err(rope_oob("%rope-char->line", "index", idx, len));
    }
    Ok(Value::int(r.char_to_line(idx as usize) as i64))
}

/// `(rope-line->char r n)` — the character index where line `n` (0-based) begins.
pub(super) fn rope_line_to_char(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let r = expect_rope_ref(heap, "%rope-line->char", arg(args, 0))?;
    let n = expect_int(heap, "%rope-line->char", arg(args, 1))?;
    let lines = r.len_lines();
    if n < 0 || n as usize > lines {
        return Err(rope_oob("%rope-line->char", "line", n, lines));
    }
    Ok(Value::int(r.line_to_char(n as usize) as i64))
}
