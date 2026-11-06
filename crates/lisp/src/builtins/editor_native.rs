//! Natives added for the editor toolkit (`std/editor/*`), registered LAST: registration
//! order feeds the intern table, and a primitive inserted mid-list reshuffles small-map
//! key order image-wide (see `syntax_scan::register`). What lives here:
//!
//! `%ui-harvest` — the per-frame pass under `editor/ui`'s memoised view fragments
//! (ADR-336). A `view` marks a fragment it memoised as `[:ui/memo key deps ops]` inside
//! the frame; the loop needs the frame WITHOUT the markers (what the frontend paints)
//! and a table `{key -> [deps ops]}` of them (what the next turn's `ui-memo` reads).
//! One walk of the frame produces both. It is the same shape as `%span-runs`: plain
//! data in, plain data out, and it runs on every frame — where a Brood-level walk of a
//! few hundred ops costs about a millisecond, more than the paint it exists to save.
//!
//! `%gui-text-contrast!` (ADR-337) and `string/width->index` — the inverse of
//! `string/display-width`, the cells -> chars half of the editor's mouse-column mapping —
//! were appended later, for the same order reason.

use super::args::arg;
use super::sequences::{ambient_glyphless, tab_layout};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value, ValueRef};
use crate::error::{LispError, LispResult};
use crate::eval::InterruptPoll;

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::{Arity, Tag};
    use crate::types::{Sig, Ty};
    primitives.def(
        "%ui-harvest",
        Arity::range(1, 2),
        Sig::with_optional(vec![any], vec![any], vec_ty),
        &["frame", "keep-markers?"],
        "[frame' table]: frame (a vector or list of render ops) with every [:ui/memo key deps ops] (or [:ui/memo key deps ops reused?]) marker replaced by its ops — or, when keep-markers? is truthy, frame itself, markers and all (for a display that takes them: the GUI window parses a reused fragment once, ADR-470) — and a map key -> [deps ops] of the markers. Recurses into a marker's ops and into the ops a block op carries — [:scroll-region frac ops], [:cell-region x y w h px ops] and [:above-seam ops] (whose ops come back as a vector, as they went in). The pass behind editor/ui's memoised view fragments (ADR-336), native because it runs once per frame over every op.",
        ui_harvest,
    );
    // The text contrast exponent (`gui-text-contrast!`, ADR-337): partial glyph coverage
    // lifted where text is lighter than its ground — light-on-dark text under a
    // linear-light blend reads thin, and this is the knob every such renderer grows.
    primitives.def(
        "%gui-text-contrast!",
        Arity::exact(1),
        Sig::new(vec![Ty::of_tags(&[Tag::Int, Tag::Float])], nil_ty),
        &["gamma"],
        "Set the text contrast exponent γ (1.0 by default, clamped to 0.5..3.0): a monochrome glyph's partial coverage is lifted to cov^(1/γ) where the text is lighter than the pixel it lands on, so light-on-dark text reads fuller without touching a glyph's interior, its exterior, dark-on-light text or colour emoji. A TASTE knob: text composites in sRGB space, the encoded space a rasteriser's coverage is made for, so 1.0 already puts a partly covered pixel at the fraction it covers — above 1.0 is deliberately bolder, not a correction. (It was a correction once: compositing text in linear light put a half-covered pixel at 188/255 instead of 128, every rim glowed, and the value that looked right was BELOW 1.0. Hence the old 1.4 default.) Applies to every open window and the default for ones opened later; a pure repaint. Needs --features gui. Returns nil.",
        gui_text_contrast,
    );
    // The inverse of `string/display-width` (cells -> chars). Lives here rather than
    // beside its sibling in `sequences.rs` for the registration-order reason above.
    primitives.def(
        "string/width->index",
        Arity::range(2, 4),
        Sig::with_optional(vec![string, int], vec![int, int], int),
        &["s", "cell", "start-col", "tab-width"],
        "The character index in s of the grapheme cluster occupying display cell `cell` (0-based), or (string/length s) when cell is at or past the string's width — the inverse of string/display-width. A cell inside a 2-cell glyph (emoji, CJK) gives that glyph's start, a cell inside a tab's span the tab, a cell inside a glyphless character's `<200b>` escape (under *glyphless-display* :hex-code) that character's cluster, and a combining mark rides with its base, so mapping a mouse click back to a character lands BEFORE a wide glyph, never inside it. The cells -> chars half of the editor's column geometry; string/display-width is the chars -> cells half, and the optional start-col (default 0) / tab-width (default the dynamic *tab-width*, 8) are the same tab layout it takes.\n\n    (string/width->index \"a😀b\" 2)   ;=> 1\n    (string/width->index \"a😀b\" 3)   ;=> 2\n    (string/width->index \"a\\tb\" 5)   ;=> 1",
        string_width_to_index,
    );
    // The third leg of the tab layout: the string a frontend is handed, tabs already
    // spaced to their stops — a raw tab in a render op has no column to measure from.
    primitives.def(
        "string/expand-tabs",
        Arity::range(1, 3),
        Sig::with_optional(vec![string], vec![int, int], string),
        &["s", "start-col", "tab-width"],
        "s with every tab replaced by the spaces that carry it to its next tab stop, laid out from column start-col (default 0) with a stop every tab-width columns (default the dynamic *tab-width*, 8) — so the result occupies exactly (string/display-width s start-col tab-width) cells, with no tab left for a frontend to misplace (a render op has no column to expand one from; a terminal would use the SCREEN column). A view lays each chunk of a line out with the column it starts at, so the stops stay the line's. When the dynamic *glyphless-display* is :hex-code, every glyphless character (string/glyphless-indices — a zero width space, a bidi control) is replaced by its visible escape too, `<200b>`, which is what string/display-width counts it as (ADR-556). Returns s itself when there is nothing to replace.\n\n    (string/expand-tabs \"a\\tb\")     ;=> \"a       b\"\n    (string/expand-tabs \"\\tx\" 3)    ;=> \"     x\"\n    (string/expand-tabs \"\\tx\" 0 4)  ;=> \"    x\"\n    (binding (*tab-width* 2) (string/expand-tabs \"\\tx\"))   ;=> \"  x\"\n    (binding (*glyphless-display* :hex-code) (string/expand-tabs \"a\\u{200b}b\"))   ;=> \"a<200b>b\"",
        string_expand_tabs,
    );
    // What changed between two ropes (ADR-471): the line-state highlight cache asks it once
    // per edit to find the first line to re-lex, without materialising either text.
    primitives.def(
        "%rope-changed-range",
        Arity::exact(2),
        Sig::new(vec![rope, rope], vec_ty.union(nil_ty)),
        &["old", "new"],
        "nil when ropes old and new hold the same text, else [start old-end new-end] in CHAR offsets: chars [start, old-end) of old became [start, new-end) of new, everything before start and after the ends being common to both. Found by comparing the two texts chunk by chunk from each end (a memcmp per chunk run, no string built) — exact for any single contiguous change (a keystroke, a paste, an undo), and correct but wider than necessary for a scattered one. The edit shape treesit's update-tree takes.",
        rope_changed_range,
    );
}

/// `string/glyphless-indices` (ADR-556), registered after every other primitive for the
/// intern-order reason in the module doc.
pub(super) fn register_glyphless(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::{Sig, Ty};
    primitives.def(
        "string/glyphless-indices",
        Arity::exact(1),
        Sig::new(vec![string], Ty::vector_of(int)),
        &["s"],
        "A vector of the character index of every GLYPHLESS character in s, in order: the invisible format characters — U+200B zero width space, U+2060 word joiner and the invisible math operators, the bidi controls (U+200E/F, U+202A–202E, U+2066–2069, U+061C), the deprecated format characters U+206A–206F, U+FEFF, U+FFF9–FFFB, the tag characters U+E0001 and U+E0020–E007F — and not the joiners (U+200C/D) or the other format characters a script's shaping consumes. What the cell layout shows as `<200b>` when *glyphless-display* is :hex-code (ADR-556); a view lays its escape face over these.\n\n    (string/glyphless-indices \"a\\u{200b}b\\u{202e}\")   ;=> [1 3]\n    (string/glyphless-indices \"plain\")   ;=> []",
        string_glyphless_indices,
    );
}

/// `(string/glyphless-indices s)` — see the registration; `text_width::glyphless_indices`.
fn string_glyphless_indices(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let indices = match arg(args, 0) {
        Value::Str(id) => crate::host::text_width::glyphless_indices(heap.string(id)),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "string/glyphless-indices",
                "string",
                other,
            ));
        }
    };
    let items = indices
        .into_iter()
        .map(|index| Value::int(index as i64))
        .collect();
    Ok(heap.alloc_vector(items))
}

struct Tags {
    memo: value::Symbol,
    scroll_region: value::Symbol,
    cell_region: value::Symbol,
    above_seam: value::Symbol,
}

/// Where an op that CARRIES ops keeps them — `Some(index)` for `[:scroll-region frac ops]`,
/// `[:cell-region x y w h px ops]` and `[:above-seam ops]`, `None` for a leaf. The harvest has
/// to recurse into those (a memo marker nested in one is still a marker) and rebuild them around the result.
fn nested_ops_at(tag: value::Symbol, tags: &Tags, len: usize) -> Option<usize> {
    if tag == tags.scroll_region && len == 3 {
        Some(2)
    } else if tag == tags.cell_region && len == 7 {
        Some(6)
    } else if tag == tags.above_seam && len == 2 {
        Some(1)
    } else {
        None
    }
}

/// Walk `ops`, appending the flattened ops to `out` and every marker to `table`.
fn harvest_into(
    heap: &mut Heap,
    ops: Vec<Value>,
    tags: &Tags,
    out: &mut Vec<Value>,
    table: &mut Vec<(Value, Value)>,
    poll: &mut InterruptPoll,
) -> Result<(), LispError> {
    for op in ops {
        poll.tick()?;
        let ValueRef::Vector(id) = op.unpack() else {
            out.push(op);
            continue;
        };
        let parts = heap.vector(id).to_vec();
        let tag = match parts.first() {
            Some(Value::Keyword(s)) => *s,
            _ => {
                out.push(op);
                continue;
            }
        };
        if tag == tags.memo && (parts.len() == 4 || parts.len() == 5) {
            let (key, deps, inner) = (parts[1], parts[2], parts[3]);
            let Some(inner_ops) = heap.list_or_vector_items(inner) else {
                continue; // a marker whose ops are not a sequence paints nothing
            };
            let mut flat = Vec::with_capacity(inner_ops.len());
            harvest_into(heap, inner_ops, tags, &mut flat, table, poll)?;
            // The entry keeps the ops AS EMITTED — inner markers included — not the
            // flattened ones. A hit replays them inside a fresh marker, so this walk sees
            // the inner markers again and every inner fragment stays in the next table.
            // Storing them flattened dropped the inner entries the first time the outer one
            // hit: a frame whose whole-pane fragment then missed had to recompute every
            // row, so nesting a memo around rows could never pay.
            let entry = heap.alloc_vector2(deps, inner);
            table.push((key, entry));
            out.extend(flat);
        } else if let Some(at) = nested_ops_at(tag, tags, parts.len()) {
            // A block op that CARRIES ops (scroll-region, cell-region): harvest inside it
            // and rebuild it around the flattened ops, so a memoised fragment works just
            // as well inside a region as outside one.
            let Some(inner_ops) = heap.list_or_vector_items(parts[at]) else {
                out.push(op);
                continue;
            };
            let mut flat = Vec::with_capacity(inner_ops.len());
            harvest_into(heap, inner_ops, tags, &mut flat, table, poll)?;
            let flat_vec = heap.alloc_vector(flat);
            let mut rebuilt = parts.clone();
            rebuilt[at] = flat_vec;
            let region = heap.alloc_vector(rebuilt);
            out.push(region);
        } else {
            out.push(op);
        }
    }
    Ok(())
}

fn ui_harvest(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let frame = arg(args, 0);
    let Some(ops) = heap.list_or_vector_items(frame) else {
        return Err(LispError::wrong_type(
            heap,
            "%ui-harvest",
            "vector or list (a frame)",
            frame,
        ));
    };
    let tags = Tags {
        memo: value::intern("ui/memo"),
        scroll_region: value::intern("scroll-region"),
        cell_region: value::intern("cell-region"),
        above_seam: value::intern("above-seam"),
    };
    let mut table = Vec::new();
    let keep_markers = crate::eval::truthy(arg(args, 1));
    let frame = if keep_markers {
        collect_markers(heap, ops, &tags, &mut table, &mut InterruptPoll::new())?;
        frame
    } else {
        let mut out = Vec::with_capacity(ops.len());
        harvest_into(
            heap,
            ops,
            &tags,
            &mut out,
            &mut table,
            &mut InterruptPoll::new(),
        )?;
        heap.alloc_vector(out)
    };
    let map = heap.map_from_pairs(table);
    Ok(heap.alloc_vector2(frame, map))
}

/// `harvest_into` without the splice: every marker of `ops` into `table`, the ops left
/// as they are — the walk for a display that takes the markers (ADR-470).
fn collect_markers(
    heap: &mut Heap,
    ops: Vec<Value>,
    tags: &Tags,
    table: &mut Vec<(Value, Value)>,
    poll: &mut InterruptPoll,
) -> Result<(), LispError> {
    for op in ops {
        poll.tick()?;
        let ValueRef::Vector(id) = op.unpack() else {
            continue;
        };
        let parts = heap.vector(id).to_vec();
        let Some(Value::Keyword(tag)) = parts.first().copied() else {
            continue;
        };
        let nested = if tag == tags.memo && (parts.len() == 4 || parts.len() == 5) {
            let entry = heap.alloc_vector2(parts[2], parts[3]);
            table.push((parts[1], entry));
            Some(parts[3])
        } else {
            nested_ops_at(tag, tags, parts.len()).map(|at| parts[at])
        };
        if let Some(inner) = nested.and_then(|inner| heap.list_or_vector_items(inner)) {
            collect_markers(heap, inner, tags, table, poll)?;
        }
    }
    Ok(())
}

/// `(%gui-text-contrast! gamma)` — see the registration. GUI only; returns nil.
fn gui_text_contrast(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let gamma = match arg(args, 0) {
        Value::Int(n) => n as f32,
        Value::Float(f) => f as f32,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%gui-text-contrast!",
                "a number (the contrast exponent, 1.0 = plain)",
                other,
            ));
        }
    };
    crate::host::gui::text_contrast(gamma).map_err(LispError::runtime)?;
    Ok(Value::nil())
}

/// `(string/width->index s cell &optional start-col tab-width)` — see the
/// registration; `text_width::index_at_cell_from`.
fn string_width_to_index(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = match arg(args, 0) {
        Value::Str(id) => heap.string(id),
        other => {
            return Err(LispError::wrong_type(
                heap,
                "string/width->index",
                "string",
                other,
            ));
        }
    };
    let cell = match arg(args, 1) {
        Value::Int(n) => n.max(0) as usize,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "string/width->index",
                "int (a display cell)",
                other,
            ));
        }
    };
    let (start_col, tab_width) = tab_layout(heap, "string/width->index", args, 2)?;
    let glyphless = ambient_glyphless(heap);
    Ok(Value::int(crate::host::text_width::index_at_cell_from(
        s, cell, start_col, tab_width, glyphless,
    ) as i64))
}

/// `(string/expand-tabs s &optional start-col tab-width)` — see the registration;
/// `text_width::expand_tabs`. A tab-free `s` comes back as the same heap string.
fn string_expand_tabs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = match arg(args, 0) {
        Value::Str(id) => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "string/expand-tabs",
                "string",
                other,
            ));
        }
    };
    let (start_col, tab_width) = tab_layout(heap, "string/expand-tabs", args, 1)?;
    let glyphless = ambient_glyphless(heap);
    let expanded = match crate::host::text_width::expand_tabs(
        heap.string(id),
        start_col,
        tab_width,
        glyphless,
    ) {
        std::borrow::Cow::Borrowed(_) => return Ok(Value::Str(id)),
        std::borrow::Cow::Owned(expanded) => expanded,
    };
    Ok(heap.alloc_string(&expanded))
}

/// `(%rope-changed-range old new)` — see the registration.
fn rope_changed_range(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let old = super::args::expect_rope_ref(heap, "%rope-changed-range", arg(args, 0))?;
    let new = super::args::expect_rope_ref(heap, "%rope-changed-range", arg(args, 1))?;
    let Some((start, old_end, new_end)) = changed_chars(old, new) else {
        return Ok(Value::nil());
    };
    let items = vec![
        Value::int(start as i64),
        Value::int(old_end as i64),
        Value::int(new_end as i64),
    ];
    Ok(heap.alloc_vector(items))
}

/// The char range `(start, old_end, new_end)` that differs between `old` and `new`, or
/// `None` when they are equal: the common prefix and (non-overlapping) common suffix, in
/// bytes, rounded outward to whole chars.
pub(super) fn changed_chars(old: &ropey::Rope, new: &ropey::Rope) -> Option<(usize, usize, usize)> {
    let prefix = common_bytes(old.chunks(), new.chunks(), usize::MAX, false);
    if prefix == old.len_bytes() && prefix == new.len_bytes() {
        return None;
    }
    let limit = old.len_bytes().min(new.len_bytes()) - prefix;
    let suffix = common_bytes(
        old.chunks_at_byte(old.len_bytes()).0.reversed(),
        new.chunks_at_byte(new.len_bytes()).0.reversed(),
        limit,
        true,
    );
    // A byte offset inside a multi-byte char: the char it is in is not common.
    let start = old.byte_to_char(prefix);
    let end_char = |rope: &ropey::Rope, byte: usize| {
        let at = rope.byte_to_char(byte);
        if rope.char_to_byte(at) < byte {
            at + 1
        } else {
            at
        }
    };
    let old_end = end_char(old, old.len_bytes() - suffix).max(start);
    let new_end = end_char(new, new.len_bytes() - suffix).max(start);
    Some((start, old_end, new_end))
}

/// How many bytes the chunk streams `a` and `b` agree on, at most `limit`: from the front,
/// or (`from_end`, the streams reversed) from the back — a slice comparison per run of
/// chunk, never a loop per byte until the run that differs. Two runs at the same address
/// are the same bytes: an edited rope shares every leaf the edit did not touch with the
/// rope it was made from, so those agree without a comparison — an unchanged rope costs a
/// step per chunk, an edited one the bytes of the chunks around the edit.
fn common_bytes<'r>(
    mut a: impl Iterator<Item = &'r str>,
    mut b: impl Iterator<Item = &'r str>,
    limit: usize,
    from_end: bool,
) -> usize {
    let (mut run_a, mut run_b): (&[u8], &[u8]) = (&[], &[]);
    let mut total = 0usize;
    while total < limit {
        if run_a.is_empty() {
            match a.next() {
                Some(chunk) => run_a = chunk.as_bytes(),
                None => break,
            }
            continue;
        }
        if run_b.is_empty() {
            match b.next() {
                Some(chunk) => run_b = chunk.as_bytes(),
                None => break,
            }
            continue;
        }
        let n = run_a.len().min(run_b.len()).min(limit - total);
        let (part_a, part_b) = if from_end {
            (&run_a[run_a.len() - n..], &run_b[run_b.len() - n..])
        } else {
            (&run_a[..n], &run_b[..n])
        };
        // one shared leaf is equal, uncompared
        let shared = part_a.as_ptr() == part_b.as_ptr();
        #[cfg(test)]
        if !shared {
            COMPARED_BYTES.with(|compared| compared.set(compared.get() + n));
        }
        if !shared && part_a != part_b {
            let mut pairs = part_a.iter().zip(part_b);
            let same = if from_end {
                pairs.rev().position(|(x, y)| x != y)
            } else {
                pairs.position(|(x, y)| x != y)
            };
            return total + same.unwrap_or(n);
        }
        total += n;
        if from_end {
            run_a = &run_a[..run_a.len() - n];
            run_b = &run_b[..run_b.len() - n];
        } else {
            run_a = &run_a[n..];
            run_b = &run_b[n..];
        }
    }
    total.min(limit)
}

#[cfg(test)]
thread_local! {
    /// Bytes `common_bytes` compared on this thread — what the shared-leaf skip saves.
    static COMPARED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod changed_range_tests {
    use super::changed_chars;
    use ropey::Rope;

    /// An edit in a large rope compares the bytes around it, not the document: the leaves
    /// the edit did not touch are shared with the rope it was made from.
    #[test]
    fn shared_leaves_are_not_compared() {
        let old = Rope::from_str(&"0123456789abcdef\n".repeat(200_000));
        let mut new = old.clone();
        new.insert(1_600_000, "x");
        super::COMPARED_BYTES.with(|compared| compared.set(0));
        assert_eq!(
            changed_chars(&old, &new),
            Some((1_600_000, 1_600_000, 1_600_001))
        );
        let compared = super::COMPARED_BYTES.with(|compared| compared.get());
        assert!(
            compared < 64 * 1024,
            "compared {compared} of {} bytes",
            old.len_bytes()
        );
        super::COMPARED_BYTES.with(|compared| compared.set(0));
        assert_eq!(changed_chars(&old, &old.clone()), None);
        assert_eq!(super::COMPARED_BYTES.with(|compared| compared.get()), 0);
    }

    fn diff(old: &str, new: &str) -> Option<(usize, usize, usize)> {
        changed_chars(&Rope::from_str(old), &Rope::from_str(new))
    }

    #[test]
    fn equal_texts_have_no_change() {
        assert_eq!(diff("", ""), None);
        assert_eq!(diff("abc\ndef", "abc\ndef"), None);
    }

    #[test]
    fn an_insertion_a_deletion_and_a_replacement_are_their_char_ranges() {
        assert_eq!(diff("abcd", "abXcd"), Some((2, 2, 3)));
        assert_eq!(diff("abcd", "ad"), Some((1, 3, 1)));
        assert_eq!(diff("abcd", "aXYd"), Some((1, 3, 3)));
        // a repeated char: the suffix never overlaps the prefix
        assert_eq!(diff("aaa", "aaaa"), Some((3, 3, 4)));
        assert_eq!(diff("", "x"), Some((0, 0, 1)));
    }

    #[test]
    fn offsets_are_chars_and_a_split_multibyte_char_is_inside_the_change() {
        // é and è share their first UTF-8 byte: the change is the whole char
        assert_eq!(diff("aéb", "aèb"), Some((1, 2, 2)));
        assert_eq!(diff("😀x", "😀yx"), Some((1, 1, 2)));
    }

    #[test]
    fn a_change_across_many_chunks_is_found_from_both_ends() {
        let base: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        let mut edited = base.clone();
        edited.insert_str(20_000, "XYZ");
        let at = base[..20_000].chars().count();
        assert_eq!(diff(&base, &edited), Some((at, at, at + 3)));
    }
}
