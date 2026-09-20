//! `%fuzzy-top` — the ranking pass under `std/fuzzy`'s `top` / `filter`, native.
//!
//! ADR-375 amends ADR-357, which decided this loop stays Brood so that the scoring rules
//! stay redefinable at runtime (`*fuzzy-scorer*`). That decision is kept: this primitive
//! implements the DEFAULT rules only, and `fuzzy/top` calls it only while `*fuzzy-scorer*`
//! is still `fuzzy/scorer`. Bind your own and every ranking in the image goes back through
//! the Brood walk, exactly as before. What changes is the price of NOT redefining them,
//! which is what an editor pays on every keystroke: ranking 27,310 repo paths measured
//! 334 ms in one Brood process and 117 ms sharded across eight — against single-digit ms
//! here, with no shard fan-out and so no copy of the candidate set across process
//! boundaries per keystroke.
//!
//! The rules are `std/fuzzy.blsp`'s, ported term for term — `fuzzy-bonus` (match, boundary,
//! contiguous, gap, lead), the greedy `fuzzy-walk`, the contiguous-run second alignment
//! (`fuzzy-contiguous`, kept when it scores higher) and the `[-score length cand]` ordering.
//! `tests/fuzzy_native_test.blsp` pins them equal over a corpus; that test is the contract,
//! not this comment.
//!
//! One deliberate difference. The walk indexes the ORIGINAL candidate (case is what a
//! camelCase hump is made of) with positions found in its LOWERED form, which is only sound
//! while lowering preserves length. `string/lower` is full Unicode lowering, where a few
//! codepoints expand (`İ` → `i̇`) and the two run out of step; this lowers per character
//! instead, so they cannot. On the paths, symbols and command names anything ranks, the two
//! agree exactly.

use super::numeric::{expect_int, expect_string_ref};
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value, ValueRef};
use crate::error::{LispError, LispResult};

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%fuzzy-top",
        Arity::range(2, 3),
        Sig::with_optional(vec![string, seqable], vec![any], list_ty),
        &["query", "cands", "limit"],
        "The best `limit` of `cands` for `query` under std/fuzzy's built-in rules, best score first (ties: shorter candidate, then lexicographic) — a nil or non-positive limit means all of them. The ranking pass behind fuzzy/top and fuzzy/filter, native because a completion UI runs it over a whole project on every keystroke; fuzzy/top calls it only while *fuzzy-scorer* is the default, so redefining the rules still moves every ranking (ADR-375 amending ADR-357). Every candidate must be a string.\n\n    (%fuzzy-top \"ab\" [\"axb\" \"ab\" \"zz\"] 1)   → (\"ab\")",
        fuzzy_top,
    );
}

// The four terms of the score, and the two penalties — `std/fuzzy.blsp`'s constants.
const MATCH_SCORE: i64 = 1;
const BOUNDARY_BONUS: i64 = 8;
const CONTIGUOUS_BONUS: i64 = 6;
const GAP_PENALTY: i64 = 3;
const GAP_EXTEND_PENALTY: i64 = 1;
const LEAD_PENALTY: i64 = 3;

/// A match just after one of these starts a word — `fuzzy-separators`.
const SEPARATORS: [char; 5] = ['-', '_', '/', '.', ' '];

/// `fuzzy-upper?`: true when lowering `c` changes it, which is how the Brood rule spells
/// "upper case" (`(not (= (string/lower ch) ch))`). A codepoint whose lowering expands to
/// several counts as upper, as it does there.
fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_uppercase();
    }
    let mut lowered = c.to_lowercase();
    match (lowered.next(), lowered.next()) {
        (Some(first), None) => first != c,
        _ => true,
    }
}

/// `c` lowered to exactly one character — see the module note on why this is per-character
/// rather than `str::to_lowercase`.
///
/// The ASCII branch is not a micro-optimisation: this runs once per character of every
/// candidate, so on a 27k-path project it is ~1.1M calls per keystroke, and `to_lowercase`
/// is a Unicode table walk returning an iterator. Taking the ASCII path first measured the
/// whole ranking 26 ms → 6 ms, and paths, module names and symbols are ASCII almost always.
fn lower_char(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    c.to_lowercase().next().unwrap_or(c)
}

/// `fuzzy-boundary?`: char `pos` starts a word in `cand` — index 0, after a separator, or a
/// camelCase hump (an upper preceded by a non-upper).
fn boundary(cand: &[char], pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    let prev = cand[pos - 1];
    SEPARATORS.contains(&prev) || (is_upper(cand[pos]) && !is_upper(prev))
}

/// `fuzzy-gap-cost`: what skipping `gap` characters between two matched ones costs.
fn gap_cost(gap: i64) -> i64 {
    if gap <= 0 {
        0
    } else {
        -(GAP_PENALTY + GAP_EXTEND_PENALTY * (gap - 1))
    }
}

/// `fuzzy-bonus`: what matching a query character at `pos` of `cand` is worth, given the
/// previous match's position.
fn bonus(cand: &[char], pos: usize, prev: Option<usize>) -> i64 {
    let boundary_term = if boundary(cand, pos) {
        BOUNDARY_BONUS
    } else {
        0
    };
    let contiguous_term = match prev {
        Some(p) if pos == p + 1 => CONTIGUOUS_BONUS,
        _ => 0,
    };
    let distance_term = match prev {
        None => -LEAD_PENALTY.min(pos as i64),
        Some(p) => gap_cost(pos as i64 - (p as i64 + 1)),
    };
    MATCH_SCORE + boundary_term + contiguous_term + distance_term
}

/// `fuzzy-walk`: the greedy left-to-right alignment of `query` (already lowered) inside
/// `clow`, scored against `cand` — `None` when the query is not a subsequence from `start`.
fn walk(query: &[char], clow: &[char], cand: &[char], start: usize) -> Option<i64> {
    let mut at = start;
    let mut prev: Option<usize> = None;
    let mut score = 0i64;
    for &qc in query {
        let found = clow[at..].iter().position(|&c| c == qc)? + at;
        score += bonus(cand, found, prev);
        prev = Some(found);
        at = found + 1;
    }
    Some(score)
}

/// The first index at which `needle` appears in `haystack` as an unbroken run.
fn find_run(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// `fuzzy-run`: the better of the greedy alignment and the contiguous one. The contiguous
/// pass can only raise a candidate, and is paid only by one that already matched.
fn score_candidate(query: &[char], clow: &[char], cand: &[char]) -> Option<i64> {
    let greedy = walk(query, clow, cand, 0)?;
    match find_run(clow, query).and_then(|at| walk(query, clow, cand, at)) {
        Some(run) if run > greedy => Some(run),
        _ => Some(greedy),
    }
}

/// The candidates of `seq` (a vector or list) as raw values.
fn candidates_of(heap: &Heap, seq: Value) -> Option<Vec<Value>> {
    match seq.unpack() {
        ValueRef::Vector(id) => Some(heap.vector(id).to_vec()),
        ValueRef::Nil | ValueRef::Pair(_) => heap.list_to_vec(seq).ok(),
        _ => None,
    }
}

/// `(%fuzzy-top query cands &optional limit)` — see the registration.
fn fuzzy_top(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%fuzzy-top";
    let query: Vec<char> = expect_string_ref(heap, who, super::numeric::arg(args, 0))?
        .chars()
        .map(lower_char)
        .collect();
    let cands_value = super::numeric::arg(args, 1);
    let Some(cands) = candidates_of(heap, cands_value) else {
        return Err(LispError::wrong_type(
            heap,
            who,
            "vector or list of strings",
            cands_value,
        ));
    };
    let limit = match args.get(2).copied().unwrap_or(Value::nil()) {
        Value::Nil => 0,
        v => expect_int(heap, who, v)?.max(0) as usize,
    };
    // An empty query matches everything at score 0 and keeps the given order, which is what
    // the Brood `top` answers without ranking at all. Mirrored here so the primitive is
    // usable on its own.
    if query.is_empty() {
        let kept = if limit > 0 && limit < cands.len() {
            cands[..limit].to_vec()
        } else {
            cands
        };
        return Ok(heap.list(kept));
    }

    // `[score, length, index]` per match. The candidate rides as its index, so the ordering
    // never copies a string and the result hands back the caller's own values.
    let mut ranked: Vec<(i64, usize, usize)> = Vec::new();
    let mut lowered: Vec<char> = Vec::new();
    let mut original: Vec<char> = Vec::new();
    for (index, cand) in cands.iter().enumerate() {
        let text = expect_string_ref(heap, who, *cand)?;
        original.clear();
        original.extend(text.chars());
        lowered.clear();
        lowered.extend(original.iter().copied().map(lower_char));
        drop(text);
        if let Some(score) = score_candidate(&query, &lowered, &original) {
            ranked.push((score, original.len(), index));
        }
    }

    // Best score first, then the shorter candidate, then lexicographic — `fuzzy-key`.
    let order = |heap: &Heap, a: &(i64, usize, usize), b: &(i64, usize, usize)| {
        b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then_with(|| {
            let (left, right) = (cands[a.2], cands[b.2]);
            match (left.unpack(), right.unpack()) {
                (ValueRef::Str(l), ValueRef::Str(r)) => heap.string(l).cmp(&*heap.string(r)),
                _ => std::cmp::Ordering::Equal,
            }
        })
    };
    let keep = if limit > 0 {
        limit.min(ranked.len())
    } else {
        ranked.len()
    };
    if keep < ranked.len() {
        // Only the kept prefix needs ordering: partition once, then sort what is shown.
        ranked.select_nth_unstable_by(keep, |a, b| order(heap, a, b));
        ranked.truncate(keep);
    }
    ranked.sort_unstable_by(|a, b| order(heap, a, b));

    let kept: Vec<Value> = ranked.iter().map(|&(_, _, index)| cands[index]).collect();
    Ok(heap.list(kept))
}
