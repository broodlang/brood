//! Case folding and searching, over ropes and strings, without materialising anything.
//!
//! Incremental search is the operation that made these necessary. `buffer-search-forward`
//! read as innocent Brood — `(index-of (string/fold-case (buffer-text buf)) needle from)` —
//! and on a 5,700-line file it cost **148 ms per keystroke**, because that line does three
//! whole-buffer passes: flatten the rope to a `String`, fold it to a second `String`, then
//! scan. `string/fold-case` was itself an interpreted `map` over every character in the
//! document. Search a 10 MB file and the editor stops.
//!
//! So: `%fold-case` does the per-character fold in one pass, and `%rope-find` / `%rope-rfind`
//! search the rope in place, folding as they stream. Nothing whole-buffer is allocated, and
//! the buffer's text is never flattened at all.
//!
//! **The fold is SIMPLE case folding, one character per character** — the contract
//! `std/string`'s `fold-case` documents and the one a text editor's case-insensitive search
//! needs: an index into the folded text has to be an index into the original. `İ` (U+0130)
//! lowercases to two characters, so it is left alone rather than shifting every offset after
//! it. Both halves of that rule live in `fold_char`, and the search uses the same function,
//! so a match can never land on the wrong characters.

use super::numeric::{arg, expect_int, expect_rope_ref, expect_string_ref};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::LispResult;

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%fold-case",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "s case-folded for comparison, ONE CHARACTER PER CHARACTER — so an index into the result is an index into s. A character whose lowercase is not a single character (İ, U+0130) is left as it is: that is the simple case fold, and it is what keeps a case-insensitive search's offsets meaningful. std/string's fold-case is this.\n\n    (%fold-case \"AbÇ\")   → \"abç\"",
        fold_case,
    );
    primitives.def(
        "%rope-find",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, any], int),
        &["r", "needle", "from", "fold?"],
        "The character index of the first occurrence of needle in rope r at or after from, or -1. With fold? the comparison is simple case folding (see %fold-case). Searches the rope IN PLACE: nothing whole-buffer is flattened or folded, which is what makes incremental search over a large file cost the scan rather than two copies of the document per keystroke. An empty needle answers from.",
        rope_find,
    );
    primitives.def(
        "%rope-rfind",
        Arity::exact(4),
        Sig::new(vec![rope, string, int, any], int),
        &["r", "needle", "before", "fold?"],
        "The character index of the LAST occurrence of needle in rope r starting strictly before `before`, or -1. With fold? the comparison is simple case folding. The backward counterpart of %rope-find, and like it, allocates nothing per call.",
        rope_rfind,
    );
}

/// Simple case folding for one character: its lowercase when that is exactly one
/// character, else the character unchanged. See the module note — this rule is the whole
/// reason an index into folded text still means something.
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut lowered = c.to_lowercase();
    match (lowered.next(), lowered.next()) {
        (Some(first), None) => first,
        _ => c,
    }
}

fn truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

/// `(%fold-case s)`
fn fold_case(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let folded: String = {
        let s = expect_string_ref(heap, "%fold-case", arg(args, 0))?;
        s.chars().map(fold_char).collect()
    };
    Ok(heap.alloc_string(&folded))
}

/// The first char index at or after `from` where `needle` occurs, or `None`.
///
/// One streaming pass with a ring buffer of the last `needle.len()` characters: the rope is
/// read through `chars_at`, which walks its leaves, so no slice is ever built. The last-
/// character test in front of the full comparison is what keeps the common position at one
/// comparison rather than `m`.
fn find_from(rope: &ropey::Rope, needle: &[char], from: usize, fold: bool) -> Option<usize> {
    let n = rope.len_chars();
    let m = needle.len();
    if m == 0 {
        return Some(from.min(n));
    }
    if from >= n || n - from < m {
        return None;
    }
    let mut ring = vec!['\0'; m];
    let mut slot = 0usize;
    let mut filled = 0usize;
    let mut index = from;
    for raw in rope.chars_at(from) {
        let c = if fold { fold_char(raw) } else { raw };
        ring[slot] = c;
        slot = (slot + 1) % m;
        if filled < m {
            filled += 1;
        }
        index += 1;
        if filled == m && c == needle[m - 1] {
            let oldest = slot; // the ring's head is the next slot to overwrite
            if (0..m).all(|k| ring[(oldest + k) % m] == needle[k]) {
                return Some(index - m);
            }
        }
    }
    None
}

/// The last occurrence starting strictly before `before`.
///
/// Forward scans that resume where the last match began, keeping the newest. Each scan
/// starts one character past the previous match, so together they read the text up to
/// `before` once — the same order as the forward search, without a second, backwards
/// streaming path to get subtly wrong.
fn find_last_before(
    rope: &ropey::Rope,
    needle: &[char],
    before: usize,
    fold: bool,
) -> Option<usize> {
    if before == 0 {
        return None;
    }
    let mut last = None;
    let mut start = 0usize;
    while let Some(at) = find_from(rope, needle, start, fold) {
        if at >= before {
            break;
        }
        last = Some(at);
        start = at + 1;
    }
    last
}

fn needle_of(
    heap: &Heap,
    who: &str,
    v: Value,
    fold: bool,
) -> Result<Vec<char>, crate::error::LispError> {
    let s = expect_string_ref(heap, who, v)?;
    Ok(if fold {
        s.chars().map(fold_char).collect()
    } else {
        s.chars().collect()
    })
}

/// `(%rope-find r needle from fold?)`
fn rope_find(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-find";
    let fold = truthy(arg(args, 3));
    let needle = needle_of(heap, who, arg(args, 1), fold)?;
    let from = expect_int(heap, who, arg(args, 2))?.max(0) as usize;
    let rope = expect_rope_ref(heap, who, arg(args, 0))?;
    Ok(Value::Int(
        find_from(&rope, &needle, from, fold).map_or(-1, |i| i as i64),
    ))
}

/// `(%rope-rfind r needle before fold?)`
fn rope_rfind(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-rfind";
    let fold = truthy(arg(args, 3));
    let needle = needle_of(heap, who, arg(args, 1), fold)?;
    let before = expect_int(heap, who, arg(args, 2))?.max(0) as usize;
    let rope = expect_rope_ref(heap, who, arg(args, 0))?;
    Ok(Value::Int(
        find_last_before(&rope, &needle, before, fold).map_or(-1, |i| i as i64),
    ))
}
