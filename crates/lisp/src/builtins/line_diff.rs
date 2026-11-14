//! `%diff-line-hunks` — the line diff under `std/diff`'s `line-hunks` / `line-status`, native
//! (ADR-468).
//!
//! `diff/seq` is an O(m·n) LCS table in Brood: right for a few hundred items, and the reason
//! an editor's change gutter shelled out to `git diff --no-index` (four subprocesses and two
//! temp files per idle beat) rather than diff a source file in the image. This is the same
//! answer git gives, computed here: Myers' O((N+M)·D) diff over interned lines, in linear
//! space (the middle-snake bisection), then git's own compaction — every slidable change
//! group moved to where `git diff`'s default indent heuristic puts it, so a line added in a
//! run of closing brackets or blank lines is marked where a reader sees it added, not at the
//! earliest position that is equally minimal.
//!
//! The heuristic is xdiff's (`xdiffi.c`: `xdl_change_compact`, `measure_split`,
//! `score_add_split`), ported with its constants; `tests/diff_test.blsp` pins the cases it
//! exists for. Mechanism only: what a hunk MEANS to a gutter is `diff/line-status`, in Brood.

use super::args::arg;
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};
use std::collections::HashMap;

pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%diff-line-hunks",
        Arity::exact(2),
        Sig::new(vec![string.union(rope), string.union(rope)], vec_ty),
        &["old", "new"],
        "The line diff of old and new (strings or ropes) as git's zero-context hunks: a vector of [old-start old-count new-start new-count], 0-based line numbers, in order. Lines of old-start..+old-count became lines of new-start..+new-count; a count of 0 is a pure insertion or deletion, its start the line the change sits before. A final newline ends the last line rather than starting an empty one. Myers' diff with git's indent-heuristic compaction, so a change lands where `git diff` puts it.\n\n    (%diff-line-hunks \"a\\nb\\nc\\n\" \"a\\nx\\nc\\n\")   ;=> [[1 1 1 1]]",
        diff_line_hunks,
    );
}

fn text_of(heap: &Heap, who: &str, v: Value) -> Result<String, LispError> {
    match v {
        Value::Rope(id) => Ok(heap.rope(id).to_string()),
        Value::Str(_) => Ok(super::args::expect_string_ref(heap, who, v)?.to_string()),
        other => Err(LispError::wrong_type(heap, who, "string or rope", other)),
    }
}

pub(super) fn diff_line_hunks(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%diff-line-hunks";
    let old = text_of(heap, who, arg(args, 0))?;
    let new = text_of(heap, who, arg(args, 1))?;
    let hunks = line_hunks(&old, &new);
    // an interrupt cut the diff short: what it answered is not the diff, so raise it
    crate::eval::check_interrupt()?;
    let rows: Vec<Value> = hunks
        .into_iter()
        .map(|(old_start, old_count, new_start, new_count)| {
            heap.alloc_vector(vec![
                Value::int(old_start as i64),
                Value::int(old_count as i64),
                Value::int(new_start as i64),
                Value::int(new_count as i64),
            ])
        })
        .collect();
    Ok(heap.alloc_vector(rows))
}

/// `text`'s lines: split on `\n`, a final newline ending the last line.
fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.trim_suffix('\n').split('\n').collect()
}

/// The hunks `(old-start old-count new-start new-count)` turning `old`'s lines into `new`'s.
pub(crate) fn line_hunks(old: &str, new: &str) -> Vec<(usize, usize, usize, usize)> {
    let old_lines = lines_of(old);
    let new_lines = lines_of(new);
    // Intern every distinct line once, so the diff compares integers.
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let mut a: Vec<u32> = Vec::with_capacity(old_lines.len());
    let mut b: Vec<u32> = Vec::with_capacity(new_lines.len());
    for (lines, out) in [(&old_lines, &mut a), (&new_lines, &mut b)] {
        for &line in lines.iter() {
            let next = ids.len() as u32;
            out.push(*ids.entry(line).or_insert(next));
        }
    }
    let mut changed_a = vec![false; a.len()];
    let mut changed_b = vec![false; b.len()];
    diff_range(&a, &b, 0, 0, &mut changed_a, &mut changed_b);
    let indents_a: Vec<i32> = old_lines.iter().map(|l| indent_of(l)).collect();
    let indents_b: Vec<i32> = new_lines.iter().map(|l| indent_of(l)).collect();
    compact(&a, &indents_a, &mut changed_a, &changed_b);
    compact(&b, &indents_b, &mut changed_b, &changed_a);
    hunks_of(&changed_a, &changed_b)
}

/// The work past which a region is reported as replaced whole rather than diffed: D times
/// the region's length. A gutter over a file rewritten top to bottom wants "all of this
/// changed" in milliseconds, not a minimal script in seconds — git caps the same search.
const COST_LIMIT: u64 = 50_000_000;

/// Mark the changed lines of `a` against `b` (each offset into the whole files by
/// `a_at` / `b_at`), recursing on the middle snake.
fn diff_range(
    a: &[u32],
    b: &[u32],
    a_at: usize,
    b_at: usize,
    changed_a: &mut [bool],
    changed_b: &mut [bool],
) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[prefix..], &b[prefix..]);
    let (a_at, b_at) = (a_at + prefix, b_at + prefix);
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
    if a.is_empty() || b.is_empty() {
        changed_a[a_at..a_at + a.len()].fill(true);
        changed_b[b_at..b_at + b.len()].fill(true);
        return;
    }
    match middle_snake(a, b) {
        // a split at either corner would recurse on the same region: never from a region
        // trimmed of its common ends, but a guard costs nothing against an infinite loop
        Some((x, y)) if (x, y) != (0, 0) && (x, y) != (a.len(), b.len()) => {
            diff_range(&a[..x], &b[..y], a_at, b_at, changed_a, changed_b);
            diff_range(&a[x..], &b[y..], a_at + x, b_at + y, changed_a, changed_b);
        }
        _ => {
            changed_a[a_at..a_at + a.len()].fill(true);
            changed_b[b_at..b_at + b.len()].fill(true);
        }
    }
}

/// Myers' bisection: a point `(x, y)` on an optimal path from `(0, 0)` to `(n, m)`, found
/// by running the search from both ends until they overlap — or `None` when the region has
/// nothing in common or the search passed [`COST_LIMIT`]. `a` and `b` are non-empty and
/// share no prefix or suffix, so the split is strictly inside and the recursion shrinks.
fn middle_snake(a: &[u32], b: &[u32]) -> Option<(usize, usize)> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max_d = (n + m + 1) / 2;
    let offset = max_d + 1;
    let width = (2 * max_d + 3) as usize;
    let mut forward = vec![-1isize; width];
    let mut backward = vec![-1isize; width];
    forward[(offset + 1) as usize] = 0;
    backward[(offset + 1) as usize] = 0;
    let delta = n - m;
    let odd = delta % 2 != 0;
    let (mut k1_start, mut k1_end, mut k2_start, mut k2_end) = (0isize, 0isize, 0isize, 0isize);
    for d in 0..max_d {
        if (d as u64) * ((n + m) as u64) > COST_LIMIT {
            return None;
        }
        // An interrupt (ADR-491) ends the search the way the cost limit does: every region
        // still to diff then gives up at once, and `diff_line_hunks` raises the interrupt.
        if d % 64 == 0 && crate::process::current_interrupt_due() {
            return None;
        }
        let mut k1 = -d + k1_start;
        while k1 <= d - k1_end {
            let index = (offset + k1) as usize;
            let mut x1 = if k1 == -d || (k1 != d && forward[index - 1] < forward[index + 1]) {
                forward[index + 1]
            } else {
                forward[index - 1] + 1
            };
            let mut y1 = x1 - k1;
            while x1 < n && y1 < m && a[x1 as usize] == b[y1 as usize] {
                x1 += 1;
                y1 += 1;
            }
            forward[index] = x1;
            if x1 > n {
                k1_end += 2;
            } else if y1 > m {
                k1_start += 2;
            } else if odd {
                let k2_index = offset + delta - k1;
                if k2_index >= 0 && (k2_index as usize) < width && backward[k2_index as usize] != -1
                {
                    let x2 = n - backward[k2_index as usize];
                    if x1 >= x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2_start;
        while k2 <= d - k2_end {
            let index = (offset + k2) as usize;
            let mut x2 = if k2 == -d || (k2 != d && backward[index - 1] < backward[index + 1]) {
                backward[index + 1]
            } else {
                backward[index - 1] + 1
            };
            let mut y2 = x2 - k2;
            while x2 < n && y2 < m && a[(n - x2 - 1) as usize] == b[(m - y2 - 1) as usize] {
                x2 += 1;
                y2 += 1;
            }
            backward[index] = x2;
            if x2 > n {
                k2_end += 2;
            } else if y2 > m {
                k2_start += 2;
            } else if !odd {
                let k1_index = offset + delta - k2;
                if k1_index >= 0 && (k1_index as usize) < width && forward[k1_index as usize] != -1
                {
                    let x1 = forward[k1_index as usize];
                    let y1 = offset + x1 - k1_index;
                    if x1 >= n - x2 {
                        return Some((x1 as usize, y1 as usize));
                    }
                }
            }
            k2 += 2;
        }
    }
    None
}

// ---- git's compaction: where a slidable change group goes --------------------------------

/// A line's indentation in columns (a tab to the next multiple of 8), or -1 for a line of
/// nothing but whitespace — xdiff's `get_indent`.
fn indent_of(line: &str) -> i32 {
    let mut columns = 0;
    for c in line.chars() {
        if !c.is_whitespace() {
            return columns;
        }
        if c == ' ' {
            columns += 1;
        } else if c == '\t' {
            columns += 8 - columns % 8;
        }
        if columns >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    -1
}

const MAX_INDENT: i32 = 200;
const MAX_BLANKS: i32 = 20;
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_WEIGHT: i32 = -30;
const POST_BLANK_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;
const INDENT_HEURISTIC_MAX_SLIDING: usize = 100;

/// What surrounds a split before line `split` — xdiff's `split_measurement`.
struct Split {
    end_of_file: bool,
    indent: i32,
    pre_blank: i32,
    pre_indent: i32,
    post_blank: i32,
    post_indent: i32,
}

fn measure_split(indents: &[i32], split: usize) -> Split {
    let (end_of_file, indent) = match indents.get(split) {
        Some(&indent) => (false, indent),
        None => (true, -1),
    };
    let mut pre_blank = 0;
    let mut pre_indent = -1;
    for &line in indents[..split.min(indents.len())].iter().rev() {
        if line != -1 {
            pre_indent = line;
            break;
        }
        pre_blank += 1;
        if pre_blank == MAX_BLANKS {
            pre_indent = 0;
            break;
        }
    }
    let mut post_blank = 0;
    let mut post_indent = -1;
    for &line in indents.iter().skip(split + 1) {
        if line != -1 {
            post_indent = line;
            break;
        }
        post_blank += 1;
        if post_blank == MAX_BLANKS {
            post_indent = 0;
            break;
        }
    }
    Split {
        end_of_file,
        indent,
        pre_blank,
        pre_indent,
        post_blank,
        post_indent,
    }
}

/// xdiff's `split_score`: lower is better.
#[derive(Clone, Copy, Default)]
struct Score {
    effective_indent: i32,
    penalty: i32,
}

fn score_add_split(split: &Split, score: &mut Score) {
    if split.pre_indent == -1 && split.pre_blank == 0 {
        score.penalty += START_OF_FILE_PENALTY;
    }
    if split.end_of_file {
        score.penalty += END_OF_FILE_PENALTY;
    }
    let post_blank = if split.indent == -1 {
        1 + split.post_blank
    } else {
        0
    };
    let total_blank = split.pre_blank + post_blank;
    score.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    score.penalty += POST_BLANK_WEIGHT * post_blank;
    let indent = if split.indent != -1 {
        split.indent
    } else {
        split.post_indent
    };
    let any_blanks = total_blank != 0;
    score.effective_indent += indent;
    if indent == -1 || split.pre_indent == -1 || indent == split.pre_indent {
        // no adjustment
    } else if indent > split.pre_indent {
        score.penalty += if any_blanks {
            RELATIVE_INDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_INDENT_PENALTY
        };
    } else if split.post_indent != -1 && split.post_indent > indent {
        score.penalty += if any_blanks {
            RELATIVE_OUTDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_OUTDENT_PENALTY
        };
    } else {
        score.penalty += if any_blanks {
            RELATIVE_DEDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_DEDENT_PENALTY
        };
    }
}

fn score_cmp(first: Score, second: Score) -> i32 {
    let indents = (first.effective_indent > second.effective_indent) as i32
        - (first.effective_indent < second.effective_indent) as i32;
    INDENT_WEIGHT * indents + (first.penalty - second.penalty)
}

/// The changed-line groups of one file, walked the way xdiff's `xdl_change_compact` walks
/// them: `start..end` the group, `unchanged_before` the unchanged lines above it — which
/// names the matching group of the OTHER file, since unchanged lines pair one to one.
struct Group<'a> {
    lines: &'a [u32],
    changed: &'a mut [bool],
    start: usize,
    end: usize,
    unchanged_before: usize,
}

impl Group<'_> {
    /// Move the group one line up — possible when the line above it equals its last line —
    /// absorbing a group it now touches.
    fn slide_up(&mut self) -> bool {
        if self.start == 0 || self.lines[self.start - 1] != self.lines[self.end - 1] {
            return false;
        }
        self.start -= 1;
        self.changed[self.start] = true;
        self.end -= 1;
        self.changed[self.end] = false;
        while self.start > 0 && self.changed[self.start - 1] {
            self.start -= 1;
        }
        self.unchanged_before -= 1;
        true
    }

    /// Move the group one line down — possible when the line below it equals its first.
    fn slide_down(&mut self) -> bool {
        if self.end >= self.lines.len() || self.lines[self.start] != self.lines[self.end] {
            return false;
        }
        self.changed[self.start] = false;
        self.start += 1;
        self.changed[self.end] = true;
        self.end += 1;
        while self.end < self.lines.len() && self.changed[self.end] {
            self.end += 1;
        }
        self.unchanged_before += 1;
        true
    }
}

/// Is the other file's group after `k` of its unchanged lines non-empty?
fn other_group_nonempty(other_unchanged: &[usize], other_len: usize, k: usize) -> bool {
    let start = if k == 0 {
        0
    } else {
        other_unchanged.get(k - 1).map_or(other_len, |&at| at + 1)
    };
    let end = other_unchanged.get(k).copied().unwrap_or(other_len);
    end > start
}

/// Slide every change group of one file to git's position, the other file fixed.
fn compact(lines: &[u32], indents: &[i32], changed: &mut [bool], other_changed: &[bool]) {
    let other_unchanged: Vec<usize> = other_changed
        .iter()
        .enumerate()
        .filter(|(_, changed)| !**changed)
        .map(|(at, _)| at)
        .collect();
    let other_len = other_changed.len();
    let nonempty = |k: usize| other_group_nonempty(&other_unchanged, other_len, k);
    let mut end = 0;
    while end < changed.len() && changed[end] {
        end += 1;
    }
    let mut group = Group {
        lines,
        changed,
        start: 0,
        end,
        unchanged_before: 0,
    };
    loop {
        if group.end != group.start {
            let mut earliest_end;
            let mut end_matching_other;
            loop {
                let size = group.end - group.start;
                end_matching_other = None;
                while group.slide_up() {}
                earliest_end = group.end;
                if nonempty(group.unchanged_before) {
                    end_matching_other = Some(group.end);
                }
                while group.slide_down() {
                    if nonempty(group.unchanged_before) {
                        end_matching_other = Some(group.end);
                    }
                }
                if size == group.end - group.start {
                    break;
                }
            }
            if group.end == earliest_end {
                // nowhere to slide
            } else if end_matching_other.is_some() {
                // line the group up with the other file's change
                while !nonempty(group.unchanged_before) {
                    group.slide_up();
                }
            } else {
                let size = group.end - group.start;
                let mut shift = earliest_end;
                if group.end > size + 1 && group.end - size - 1 > shift {
                    shift = group.end - size - 1;
                }
                if group.end > INDENT_HEURISTIC_MAX_SLIDING
                    && group.end - INDENT_HEURISTIC_MAX_SLIDING > shift
                {
                    shift = group.end - INDENT_HEURISTIC_MAX_SLIDING;
                }
                let mut best: Option<(usize, Score)> = None;
                while shift <= group.end {
                    let mut score = Score::default();
                    score_add_split(&measure_split(indents, shift), &mut score);
                    score_add_split(&measure_split(indents, shift - size), &mut score);
                    if best.is_none_or(|(_, best_score)| score_cmp(score, best_score) <= 0) {
                        best = Some((shift, score));
                    }
                    shift += 1;
                }
                if let Some((best_shift, _)) = best {
                    while group.end > best_shift {
                        group.slide_up();
                    }
                }
            }
        }
        // the next group, past one unchanged line
        if group.end >= group.lines.len() {
            break;
        }
        group.start = group.end + 1;
        group.end = group.start;
        while group.end < group.lines.len() && group.changed[group.end] {
            group.end += 1;
        }
        group.unchanged_before += 1;
    }
}

/// The hunks of two change maps: each maximal run of changed lines, paired across the files
/// by the unchanged lines between them.
fn hunks_of(changed_a: &[bool], changed_b: &[bool]) -> Vec<(usize, usize, usize, usize)> {
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < changed_a.len() || j < changed_b.len() {
        let a_changed = i < changed_a.len() && changed_a[i];
        let b_changed = j < changed_b.len() && changed_b[j];
        if a_changed || b_changed {
            let (i0, j0) = (i, j);
            while i < changed_a.len() && changed_a[i] {
                i += 1;
            }
            while j < changed_b.len() && changed_b[j] {
                j += 1;
            }
            hunks.push((i0, i - i0, j0, j - j0));
        } else {
            i += 1;
            j += 1;
        }
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::line_hunks;

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(line_hunks("a\nb\n", "a\nb").is_empty());
        assert!(line_hunks("", "").is_empty());
    }

    #[test]
    fn a_replaced_line_is_one_hunk_of_one_and_one() {
        assert_eq!(line_hunks("a\nb\nc\n", "a\nx\nc\n"), vec![(1, 1, 1, 1)]);
    }

    #[test]
    fn insertions_and_deletions_sit_before_the_line_they_precede() {
        assert_eq!(line_hunks("a\nc\n", "a\nb\nc\n"), vec![(1, 0, 1, 1)]);
        assert_eq!(line_hunks("a\nb\nc\n", "a\nc\n"), vec![(1, 1, 1, 0)]);
        assert_eq!(line_hunks("", "a\n"), vec![(0, 0, 0, 1)]);
    }

    /// The case the heuristic exists for: a function added after another in a file of
    /// blank-separated functions is the whole new function, not the tail of the old one.
    #[test]
    fn an_added_block_lands_where_git_puts_it() {
        let old = "def a\n  1\nend\n\ndef c\n  3\nend\n";
        let new = "def a\n  1\nend\n\ndef b\n  2\nend\n\ndef c\n  3\nend\n";
        assert_eq!(line_hunks(old, new), vec![(4, 0, 4, 4)]);
    }

    #[test]
    fn an_insertion_in_a_run_of_equal_lines_slides_to_its_end() {
        assert_eq!(
            line_hunks("A\np\nx\nx\nB\n", "C\np\nx\nx\nx\nB\n"),
            vec![(0, 1, 0, 1), (4, 0, 4, 1)]
        );
    }

    #[test]
    fn a_wholly_rewritten_file_is_one_hunk() {
        let old: String = (0..3000).map(|i| format!("old {i}\n")).collect();
        let new: String = (0..3000).map(|i| format!("new {i}\n")).collect();
        assert_eq!(line_hunks(&old, &new), vec![(0, 3000, 0, 3000)]);
    }

    /// Every hunk set rebuilds `new` from `old` — over edits made at many places.
    #[test]
    fn hunks_rebuild_the_new_text() {
        let old: Vec<String> = (0..200).map(|i| format!("line {}", i % 7)).collect();
        let mut new = old.clone();
        new.remove(150);
        new.insert(120, "fresh".to_string());
        new[60] = "changed".to_string();
        new.insert(3, "line 3".to_string());
        new.truncate(190);
        let old_text = old.join("\n");
        let new_text = new.join("\n");
        let mut rebuilt: Vec<&str> = Vec::new();
        let mut at = 0;
        for (old_start, old_count, new_start, new_count) in line_hunks(&old_text, &new_text) {
            rebuilt.extend(old[at..old_start].iter().map(String::as_str));
            rebuilt.extend(
                new[new_start..new_start + new_count]
                    .iter()
                    .map(String::as_str),
            );
            at = old_start + old_count;
        }
        rebuilt.extend(old[at..].iter().map(String::as_str));
        assert_eq!(rebuilt, new.iter().map(String::as_str).collect::<Vec<_>>());
    }
}
