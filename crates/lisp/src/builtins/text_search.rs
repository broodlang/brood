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

use super::args::{arg, expect_int, expect_rope_ref, expect_string_ref};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::InterruptPoll;

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%fold-case",
        Arity::exact(1),
        Sig::new(vec![string], string),
        &["s"],
        "s case-folded for comparison, ONE CHARACTER PER CHARACTER — so an index into the result is an index into s. A character whose lowercase is not a single character (İ, U+0130) is left as it is: that is the simple case fold, and it is what keeps a case-insensitive search's offsets meaningful. std/string's fold-case is this.\n\n    (%fold-case \"AbÇ\")   ;=> \"abç\"",
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
    primitives.def(
        "%rope-skip",
        Arity::exact(5),
        Sig::new(vec![rope, int, any, string, any], int),
        &["r", "at", "forward?", "set", "inside?"],
        "Where a run of characters reaches from `at` in rope r: with inside? the run of characters IN set, else of characters NOT in it. Forward (forward? true) it answers the first index at or after `at` whose character ends the run; backward, the smallest index at or before `at` such that every character between it and `at` is in the run. Reads only the run, not the rope: a word motion over a large buffer costs the word. `at` is clamped to the rope.\n\n    (%rope-skip (text/string-> \"  ab cd\") 0 true \" \" true)   ;=> 2",
        rope_skip,
    );
    primitives.def(
        "%skip-word",
        Arity::exact(5),
        Sig::new(
            vec![rope_or_string, int, any, string, any],
            int,
        ),
        &["text", "at", "forward?", "word-chars", "inside?"],
        "Where a run of word characters (inside? true) or of non-word characters (inside? false) reaches from `at` in text, a rope or a string. A word character is a Unicode letter, a number, a combining mark, or one of word-chars — the word syntax of ADR-774. Forward (forward? true) it answers the first index at or after `at` whose character ends the run; backward, the smallest index at or before `at` such that every character between it and `at` is in the run. On a rope it reads only the run, not the text. `at` is clamped to the text.\n\n    (%skip-word \"foo_bar baz\" 0 true \"\" true)   ;=> 3",
        skip_word,
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
/// Chunk by chunk, with the standard library's substring search over each leaf of the rope
/// and the last `needle.len() - 1` characters of the previous leaf carried in front, so a
/// match spanning two leaves is found. No slice of the whole text is ever built. A
/// character-at-a-time walk with a ring buffer read ~200 MB/s, which made a missed search
/// over a 1.5 MB buffer (isearch typing into a failing search, query-replace's last step)
/// cost ~8 ms; the substring search is several times that.
///
/// Folding is one char to one char ([`fold_char`]), so a folded leaf has exactly the char
/// count of the original and a char index found in it is an index into the rope.
fn find_from(
    rope: &ropey::Rope,
    needle: &[char],
    from: usize,
    fold: bool,
) -> Result<Option<usize>, LispError> {
    let n = rope.len_chars();
    let m = needle.len();
    if m == 0 {
        return Ok(Some(from.min(n)));
    }
    if from >= n || n - from < m {
        return Ok(None);
    }
    // a missed search over a large rope reads all of it: poll per leaf (ADR-491)
    let mut poll = InterruptPoll::new();
    let pattern: String = needle.iter().collect();
    let (chunks, _, chunk_char_start, _) = rope.chunks_at_char(from);
    // `window` is the carried tail of the previous leaf followed by this leaf (from `from`
    // on); `window_start` is the char index of its first character.
    let mut carry = String::new();
    let mut window_start = from;
    let mut first = true;
    for chunk in chunks {
        poll.tick()?;
        let piece: &str = if first {
            first = false;
            // the first leaf may begin before `from`
            let skip = from - chunk_char_start;
            match chunk.char_indices().nth(skip) {
                Some((byte, _)) => &chunk[byte..],
                None => "",
            }
        } else {
            chunk
        };
        let mut window = std::mem::take(&mut carry);
        if fold && piece.is_ascii() {
            // an ASCII leaf (most source): `fold_char` is `to_ascii_lowercase` there, done whole
            window.push_str(&piece.to_ascii_lowercase());
        } else if fold {
            window.extend(piece.chars().map(fold_char));
        } else {
            window.push_str(piece);
        }
        if let Some(byte) = window.find(&pattern) {
            return Ok(Some(window_start + window[..byte].chars().count()));
        }
        // Keep the last m-1 chars: a match that starts in them can finish in the next leaf.
        let window_chars = window.chars().count();
        let keep = (m - 1).min(window_chars);
        let cut = window
            .char_indices()
            .nth(window_chars - keep)
            .map_or(window.len(), |(byte, _)| byte);
        carry = window[cut..].to_string();
        window_start += window_chars - keep;
    }
    Ok(None)
}

/// The last occurrence starting strictly before `before`.
///
/// Read BACKWARD from `before` a slice at a time, each slice searched with the standard
/// library's `rfind`, so the cost is the distance to the match rather than the offset of
/// `before`. A forward rescan from offset 0 (what this was) cost 40 ms at the end of a
/// 40 MB mailbox on every "previous message", and bedit carried a chunked backward search
/// of its own until this did the same. Each slice reaches `needle.len() - 1` characters
/// past its start into the slice after it, so a match straddling the boundary is whole in
/// one of them; any match a slice holds starts before `before`, because the first slice
/// ends `needle.len() - 1` characters past it.
fn find_last_before(
    rope: &ropey::Rope,
    needle: &[char],
    before: usize,
    fold: bool,
) -> Result<Option<usize>, LispError> {
    const SLICE_CHARS: usize = 65_536;
    if before == 0 {
        return Ok(None);
    }
    // An empty needle occurs at every index 0..=len.
    if needle.is_empty() {
        return Ok(Some((before - 1).min(rope.len_chars())));
    }
    let n = rope.len_chars();
    let m = needle.len();
    let before = before.min(n);
    let pattern: String = needle.iter().collect();
    let width = SLICE_CHARS.max(4 * m);
    let mut end = (before + m - 1).min(n);
    loop {
        // a slice is 64K characters: probe for an interrupt before each (ADR-491)
        crate::eval::check_interrupt()?;
        let start = end.saturating_sub(width);
        if end - start >= m {
            let slice = rope.slice(start..end);
            let text: String = if fold {
                slice.chars().map(fold_char).collect()
            } else {
                slice.to_string()
            };
            if let Some(byte) = text.rfind(&pattern) {
                return Ok(Some(start + text[..byte].chars().count()));
            }
        }
        if start == 0 {
            return Ok(None);
        }
        end = start + m - 1;
    }
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
        find_from(rope, &needle, from, fold)?.map_or(-1, |i| i as i64),
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
        find_last_before(rope, &needle, before, fold)?.map_or(-1, |i| i as i64),
    ))
}

/// How many characters of `chars` continue a run (`continues`) before the first that does
/// not. Polls per character: a run can be the whole document, a file of spaces (ADR-491).
fn run_length(
    chars: impl Iterator<Item = char>,
    continues: impl Fn(char) -> bool,
) -> Result<usize, LispError> {
    let mut poll = InterruptPoll::new();
    let mut length = 0;
    for character in chars {
        poll.tick()?;
        if !continues(character) {
            break;
        }
        length += 1;
    }
    Ok(length)
}

/// How far a run of characters that `continues` reaches from `at` in a rope: forward, the
/// first index ≥ `at` whose character does not continue the run; backward, the smallest
/// index ≤ `at` such that every character between it and `at` does. Reads only the run
/// itself (`chars_at` walks the rope's leaves, and `prev` steps back through them), so a
/// word motion costs the word, not the document.
fn skip_run(
    rope: &ropey::Rope,
    at: usize,
    forward: bool,
    continues: impl Fn(char) -> bool,
) -> Result<usize, LispError> {
    let at = at.min(rope.len_chars());
    let mut chars = rope.chars_at(at);
    if forward {
        Ok(at + run_length(chars, continues)?)
    } else {
        Ok(at - run_length(std::iter::from_fn(|| chars.prev()), continues)?)
    }
}

/// [`skip_run`] over a string, for the short texts (a prompt's line, one character) that
/// are not ropes.
fn skip_run_in_string(
    text: &str,
    at: usize,
    forward: bool,
    continues: impl Fn(char) -> bool,
) -> Result<usize, LispError> {
    let at_byte = text
        .char_indices()
        .nth(at)
        .map_or(text.len(), |(byte, _)| byte);
    let at = text[..at_byte].chars().count();
    if forward {
        Ok(at + run_length(text[at_byte..].chars(), continues)?)
    } else {
        Ok(at - run_length(text[..at_byte].chars().rev(), continues)?)
    }
}

/// `(%rope-skip r at forward? set inside?)`
fn rope_skip(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%rope-skip";
    let forward = truthy(arg(args, 2));
    let inside = truthy(arg(args, 4));
    let set: Vec<char> = expect_string_ref(heap, who, arg(args, 3))?
        .chars()
        .collect();
    let at = expect_int(heap, who, arg(args, 1))?.max(0) as usize;
    let rope = expect_rope_ref(heap, who, arg(args, 0))?;
    let continues = |character: char| set.contains(&character) == inside;
    Ok(Value::Int(skip_run(rope, at, forward, continues)? as i64))
}

/// Is `character` a word constituent under a word syntax whose extra word characters are
/// `word_chars` (ADR-774)? Emacs's `w` class: a letter or a number in any script, plus a
/// combining mark so a decomposed `é` stays inside its word, plus what the mode adds.
fn is_word_char(character: char, word_chars: &[char]) -> bool {
    character.is_alphanumeric()
        || unicode_normalization::char::is_combining_mark(character)
        || word_chars.contains(&character)
}

/// `(%skip-word text at forward? word-chars inside?)`
fn skip_word(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%skip-word";
    let forward = truthy(arg(args, 2));
    let inside = truthy(arg(args, 4));
    let word_chars: Vec<char> = expect_string_ref(heap, who, arg(args, 3))?
        .chars()
        .collect();
    let at = expect_int(heap, who, arg(args, 1))?.max(0) as usize;
    let continues = |character: char| is_word_char(character, &word_chars) == inside;
    let index = match arg(args, 0) {
        Value::Str(id) => skip_run_in_string(heap.string(id), at, forward, continues)?,
        text => skip_run(expect_rope_ref(heap, who, text)?, at, forward, continues)?,
    };
    Ok(Value::Int(index as i64))
}

#[cfg(test)]
mod find_tests {
    use super::*;

    /// The naive answer: flatten, fold, search, convert the byte offset to a char index.
    fn naive(text: &str, needle: &str, from: usize, fold: bool) -> Option<usize> {
        let folded: String = if fold {
            text.chars().map(fold_char).collect()
        } else {
            text.to_string()
        };
        let needle: String = if fold {
            needle.chars().map(fold_char).collect()
        } else {
            needle.to_string()
        };
        let start_byte = folded
            .char_indices()
            .nth(from)
            .map_or(folded.len(), |(b, _)| b);
        folded[start_byte..]
            .find(&needle)
            .map(|b| from + folded[start_byte..start_byte + b].chars().count())
    }

    /// Searching a rope of many leaves agrees with searching the flattened text: at every
    /// start offset, for needles that straddle leaf boundaries, repeat their own prefix
    /// ("aab" in "aaab"), contain multi-byte characters, or do not occur — folded and not.
    #[test]
    fn chunked_search_agrees_with_a_flat_search() {
        let unit = "lorem IPSUM aaab ταχύ Ωmega (def x) ";
        let text: String = unit.repeat(600) + "THE-END";
        let rope = ropey::Rope::from_str(&text);
        assert!(
            rope.chunks().count() > 4,
            "the text must span several leaves"
        );
        let total = rope.len_chars();
        for needle in [
            "aab",
            "ipsum",
            "IPSUM",
            "ταχύ",
            "ωmega (def",
            "the-end",
            "x) lorem",
            "absent!",
        ] {
            for fold in [false, true] {
                let needle_chars: Vec<char> = if fold {
                    needle.chars().map(fold_char).collect()
                } else {
                    needle.chars().collect()
                };
                for from in (0..total).step_by(97).chain([total - 7, total - 1]) {
                    assert_eq!(
                        find_from(&rope, &needle_chars, from, fold).unwrap(),
                        naive(&text, needle, from, fold),
                        "needle {needle:?} fold {fold} from {from}"
                    );
                }
            }
        }
    }

    /// The backward search agrees with "the last forward match before `before`" over a text
    /// several slices long: matches straddling a slice boundary, overlapping matches ("aa"
    /// in "aaa"), multi-byte characters, a needle that does not occur, folded and not.
    #[test]
    fn backward_search_agrees_with_the_last_forward_match() {
        let unit = "lorem IPSUM aaab ταχύ Ωmega (def x) ";
        let text: String = unit.repeat(5_000) + "THE-END";
        let rope = ropey::Rope::from_str(&text);
        let total = rope.len_chars();
        assert!(total > 2 * 65_536, "the text must span several slices");
        for needle in [
            "aa",
            "ipsum",
            "ταχύ",
            "ωmega (def",
            "the-end",
            "x) lorem",
            "absent!",
        ] {
            for fold in [false, true] {
                let needle_chars: Vec<char> = if fold {
                    needle.chars().map(fold_char).collect()
                } else {
                    needle.chars().collect()
                };
                // every match start, once: `naive` refolds the whole text per call
                let haystack: Vec<char> = if fold {
                    text.chars().map(fold_char).collect()
                } else {
                    text.chars().collect()
                };
                let starts: Vec<usize> = (0..haystack.len())
                    .filter(|&at| haystack[at..].starts_with(&needle_chars))
                    .collect();
                assert_eq!(starts.first().copied(), naive(&text, needle, 0, fold));
                for before in (0..=total).step_by(4_099).chain([65_536, 65_537, total]) {
                    let expected = starts.iter().rev().find(|&&at| at < before).copied();
                    assert_eq!(
                        find_last_before(&rope, &needle_chars, before, fold).unwrap(),
                        expected,
                        "needle {needle:?} fold {fold} before {before}"
                    );
                }
            }
        }
    }

    /// A word scan over a rope of many leaves answers what the same scan over the flat
    /// string answers, at every offset, in both directions, for both runs, and for a word
    /// syntax with and without extra word characters (ADR-774).
    #[test]
    fn word_scan_over_a_rope_agrees_with_the_string_scan() {
        let unit = "don't foo_bar ταχύ e\u{301}te x2 (a-b) 50% ";
        let text: String = unit.repeat(400);
        let rope = ropey::Rope::from_str(&text);
        assert!(
            rope.chunks().count() > 4,
            "the text must span several leaves"
        );
        for word_chars in [vec![], vec!['\'', '%']] {
            for inside in [false, true] {
                let continues = |character: char| is_word_char(character, &word_chars) == inside;
                for at in (0..=rope.len_chars()).step_by(7) {
                    for forward in [false, true] {
                        assert_eq!(
                            skip_run(&rope, at, forward, continues).unwrap(),
                            skip_run_in_string(&text, at, forward, continues).unwrap(),
                            "at {at} forward {forward} inside {inside} extras {word_chars:?}"
                        );
                    }
                }
            }
        }
    }

    /// Letters and digits of any script and combining marks are word characters; `_`, `-`,
    /// `'` and `.` are not, unless the word syntax adds them.
    #[test]
    fn a_word_is_letters_digits_and_the_declared_extras() {
        for character in ['a', 'Z', '7', 'é', 'λ', '中', '\u{301}'] {
            assert!(is_word_char(character, &[]), "{character:?}");
        }
        for character in ['_', '-', '\'', '.', ' ', '(', '/'] {
            assert!(!is_word_char(character, &[]), "{character:?}");
        }
        assert!(is_word_char('\'', &['\'']));
        assert_eq!(
            skip_run_in_string("foo_bar", 0, true, |character| is_word_char(character, &[]))
                .unwrap(),
            3
        );
        assert_eq!(
            skip_run_in_string("don't x", 0, true, |character| is_word_char(
                character,
                &['\'']
            ))
            .unwrap(),
            5
        );
        assert_eq!(
            skip_run_in_string("ab", 9, true, |character| is_word_char(character, &[])).unwrap(),
            2
        );
        assert_eq!(
            skip_run_in_string("ab cd", 5, false, |character| is_word_char(character, &[]))
                .unwrap(),
            3
        );
    }
}
