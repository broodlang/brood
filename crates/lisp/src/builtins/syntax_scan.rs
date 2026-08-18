// Editor syntax-scanning / span / highlight / clipboard builtins — extracted from
// sequences.rs (these are editor tooling, not string ops).
#![allow(unused_imports)]
use super::numeric::{arg, expect_int, expect_string, expect_string_ref};
use super::sequences::*;
use super::*;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::syntax::printer;

pub(super) fn scan_atom_kind(t: &str) -> &'static str {
    if t.starts_with(':') || t == "nil" || t == "true" || t == "false" {
        "keyword"
    } else if t.parse::<i64>().is_ok() || t.parse::<f64>().is_ok() {
        "number"
    } else {
        "symbol"
    }
}

/// `(scan-tokens s)` — lexically tokenize Brood source `s` into a vector of
/// `[start end kind text]` tokens (char offsets, end-exclusive; whitespace and commas
/// skipped between tokens). `kind` is `:comment`, `:string`, `:number`, `:keyword`,
/// `:symbol`, `:open`, or `:close`. The lossless token stream a fontifier / structural
/// tool walks — the per-character scanning (a render hot path in interpreted Brood) runs
/// here in Rust, leaving the consumer to apply policy (faces, head-position) over
/// O(tokens), not O(chars). Strings honour `\\` escapes; a comment runs to end-of-line.
pub(super) fn scan_tokens(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "scan-tokens", arg(args, 0))?;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let kw = |k: &'static str| Value::keyword(value::intern(k));
    let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | ',');
    let is_delim = |c: char| is_ws(c) || matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';');
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0usize;
    while i < n {
        if is_ws(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        let (end, kind): (usize, &'static str) = match chars[i] {
            ';' => {
                let mut j = i + 1;
                while j < n && chars[j] != '\n' {
                    j += 1;
                }
                (j, "comment")
            }
            '"' => {
                let mut j = i + 1;
                loop {
                    if j >= n {
                        break;
                    }
                    match chars[j] {
                        '\\' => j += 2, // escape: skip the backslash and the next char
                        '"' => {
                            j += 1;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                (j.min(n), "string")
            }
            '(' | '[' | '{' => (start + 1, "open"),
            ')' | ']' | '}' => (start + 1, "close"),
            // `|…|` bar-quoted symbol / `:|…|` keyword — one token, scanned to the
            // closing bar (honouring `\|`/`\\`), so a space inside doesn't split it.
            // Mirrors the reader/CST so the token stream agrees with what parses.
            '|' => (scan_bar(&chars, n, i + 1), "symbol"),
            ':' if i + 1 < n && chars[i + 1] == '|' => (scan_bar(&chars, n, i + 2), "keyword"),
            _ => {
                let mut j = i;
                while j < n && !is_delim(chars[j]) {
                    j += 1;
                }
                let text: String = chars[start..j].iter().collect();
                (j, scan_atom_kind(&text))
            }
        };
        let text: String = chars[start..end].iter().collect();
        let tv = heap.alloc_string(&text);
        let tok = heap.alloc_vector(vec![
            Value::int(start as i64),
            Value::int(end as i64),
            kw(kind),
            tv,
        ]);
        out.push(tok);
        i = end;
    }
    Ok(heap.alloc_vector(out))
}

/// `(scan-form-start s pos)` — the greatest char offset ≤ `pos` of a column-0 open
/// bracket (`(`/`[`/`{`) in `s` lying OUTSIDE any string or `;` comment, else 0. The
/// string/comment-aware `beginning-of-defun` primitive behind `highlight/safe-restart`
/// and `tool/sexp`'s narrowing window: correctness requires a forward lexical pass from
/// the top (a backward scan cannot know whether a bracket sits inside a string without
/// the lexer state a forward pass carries), and that pass is O(pos) — ruinous per
/// keystroke in interpreted Brood on a large file (eldoc / fontify-restart / structural
/// motion all sit on it), trivial here. Strings honour `\\` escapes; a comment runs to
/// end-of-line — the same lexical rules as `scan-tokens`.
/// `(scan-form-start-2 s pos)` — `[prev start]`: the greatest column-0 form-start offset
/// `<= pos` (`start`, as [`scan_form_start`]) **and** the one before it (`prev`, i.e. the
/// greatest such offset `< start`), both from a SINGLE forward pass.
///
/// Exists because `tool/sexp`'s `narrow` wants exactly this pair, and computing it as two
/// `scan-form-start` calls runs the O(pos) lexical pass twice over the same prefix — the
/// second call re-walking ground the first already covered. One pass halves the dominant
/// cost of every structural motion. `prev` is 0 when there is no earlier form start, which
/// matches what the second call returned in that case (`scan-form-start` of a position
/// before any form start is 0), so the pair is a drop-in for the two calls.
///
/// The sequence of motions is still O(n^2) overall — see `scan_form_start`'s note on why the
/// forward pass from the top is required. This makes the constant twice as good; it does not
/// change the shape. The real fix is resumable lexer state, which needs somewhere to live.
// ---- the column-0 form-start lexer, and its safepoint table -----------------
//
// `scan-form-start` / `-2` answer "where does the top-level form containing `pos` begin"
// — the beginning-of-defun primitive under `tool/sexp`'s narrowing and
// `editor/highlight`'s `safe-restart`. Correctness requires a FORWARD lexical pass from
// offset 0: a backward scan cannot know whether a `(` sits inside a string or a comment.
// That makes one call O(pos) and a SEQUENCE of motions over one buffer O(n^2), which is
// exactly what `scale_sweep.blsp`'s `sexp motions` row measured (2.3 s at 3200 forms,
// 18.9 s at 12800).
//
// Two things fix it, and only the second changes the shape:
//
//  1. The pass runs over BYTES, not a `Vec<char>`. It used to materialise `pos + 1`
//     `char`s — 4 bytes each — per call, so a motion near the bottom of a 1 MB file
//     allocated 4 MB before scanning it. Every character this lexer cares about (`"`,
//     `\`, `;`, `\n`, the brackets) is ASCII, so byte matching is exact; the char index
//     the language speaks is carried alongside, incremented per char boundary.
//
//  2. A **safepoint table** cached against the string value ([`Heap::str_scan_table`]),
//     holding the lexer's answer every `SCAN_POINT_STRIDE` bytes. A query then resumes
//     from the last safepoint at or before `pos` instead of from 0, so a motion costs
//     O(stride) and a sequence over ONE text value is linear. Safepoints are only placed
//     where the lexer is between tokens, so the resumed state is always "in code" — a
//     string or comment is skipped atomically within one step, and one inside a long
//     literal simply gets no safepoint of its own.
//
// What this does NOT fix, deliberately recorded because it looks like it should: the
// buffer-command path (`sexp/forward` and friends) calls `(buffer-text buf)`, which is
// `rope->string` — a FRESH string value per motion, so this table (and the char↔byte
// index) can never hit there. The residual cost is that MISS, not the copy: measured
// 2026-08-07, the `rope->string` copy is ~14% of a buffer-path motion, while the table
// miss makes `scan-form-start-2` re-scan O(pos) from the top every keystroke — so a
// buffer-path motion stays O(buffer) even though `narrow`'s forward window scan is now
// native (`scan-form-end`) and the held-text path is flat. Making the buffer path flat
// needs the cache to survive across motions — rope-native scanning, or a GC-safe
// `rope->string` memo so an unchanged rope yields the same string value — a separate
// piece of work. The shapes this DOES fix are the ones that hold one text value across
// many queries: the sweep row, `nest check`-style tooling, and an LSP/eldoc pass over an
// unchanged document.

/// Bytes between safepoints. A 1 MB source gets ~256 of them (~5 KB of table), and a
/// query's residual scan is bounded by one stride rather than by `pos`.
const SCAN_POINT_STRIDE: usize = 4096;

/// Below this many bytes a query just scans from 0: the whole pass is already short, and
/// a table (plus its allocation) would cost more than it saves.
const SCAN_TABLE_MIN_BYTES: usize = 4096;

/// The lexer's state at a point it can resume from. `at_bol` is what decides whether a
/// bracket is in column 0; `best`/`prev` are the answer so far, so a resume needs no
/// history before this point.
#[derive(Clone, Copy)]
struct ScanPoint {
    byte: u32,
    ch: u32,
    at_bol: bool,
    best: u32,
    prev: u32,
}

impl ScanPoint {
    /// The state at the start of any text: offset 0 is column 0, and "no form start seen"
    /// reports as 0 — which is also what a form start *at* char 0 reports, exactly as the
    /// pre-table implementation did.
    fn start() -> ScanPoint {
        ScanPoint {
            byte: 0,
            ch: 0,
            at_bol: true,
            best: 0,
            prev: 0,
        }
    }
}

/// Safepoints for one string value, ascending in both `byte` and `ch`. Cached through
/// [`Heap::str_scan_table`], so it travels with the string (including across a GC copy)
/// and is shared by every process that reads the same RUNTIME/PRELUDE string.
struct FormScanIndex {
    points: Vec<ScanPoint>,
}

/// Width in bytes of the UTF-8 character whose lead byte is `b`.
#[inline]
fn utf8_width(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else {
        2
    }
}

/// The lexical pass, resumed from `from` and stopped once the char index passes `upto`.
/// `emit` is called at each safepoint-eligible position (between tokens) so one pass can
/// both answer a query and build the table. Returns the state reached.
///
/// The rules are the pre-existing ones, unchanged: `"` opens a string body in which `\`
/// escapes the next character and which an unterminated literal runs to the end of; `;`
/// runs to end-of-line; and `(`/`[`/`{` in column 0 outside both is a form start, where
/// the previous `best` becomes `prev`.
fn form_scan(s: &str, from: ScanPoint, upto: usize, mut emit: impl FnMut(ScanPoint)) -> ScanPoint {
    let b = s.as_bytes();
    let len = b.len();
    let mut st = from;
    // Advance one whole character, keeping the byte and char cursors in step.
    macro_rules! step {
        () => {{
            st.byte += utf8_width(b[st.byte as usize]) as u32;
            st.ch += 1;
        }};
    }
    while (st.byte as usize) < len && (st.ch as usize) <= upto {
        emit(st);
        match b[st.byte as usize] {
            b'"' => {
                step!();
                while (st.byte as usize) < len && (st.ch as usize) <= upto {
                    match b[st.byte as usize] {
                        b'\\' => {
                            step!();
                            if (st.byte as usize) < len {
                                step!();
                            }
                        }
                        b'"' => {
                            step!();
                            break;
                        }
                        _ => step!(),
                    }
                }
                st.at_bol = false;
            }
            b';' => {
                while (st.byte as usize) < len && b[st.byte as usize] != b'\n' {
                    step!();
                }
                st.at_bol = false;
            }
            b'(' | b'[' | b'{' if st.at_bol => {
                if st.ch > 0 {
                    st.prev = st.best;
                }
                st.best = st.ch;
                step!();
                st.at_bol = false;
            }
            c => {
                st.at_bol = c == b'\n';
                step!();
            }
        }
    }
    st
}

/// `(best, prev)` for char index `pos` in `s`, resuming from `table`'s last safepoint at
/// or before `pos` when there is one.
fn form_start_pair(s: &str, table: Option<&FormScanIndex>, pos: usize) -> (usize, usize) {
    let from = match table {
        Some(t) => {
            let k = t.points.partition_point(|p| (p.ch as usize) <= pos);
            if k == 0 {
                ScanPoint::start()
            } else {
                t.points[k - 1]
            }
        }
        None => ScanPoint::start(),
    };
    let end = form_scan(s, from, pos, |_| {});
    (end.best as usize, end.prev as usize)
}

/// The safepoint table for string `id`, or `None` for a text short enough to rescan. Built
/// once per string value and cached on it.
fn form_scan_table(heap: &Heap, v: Value, len: usize) -> Option<std::sync::Arc<FormScanIndex>> {
    if len < SCAN_TABLE_MIN_BYTES {
        return None;
    }
    let Value::Str(id) = v else { return None };
    let any = heap.str_scan_table(id, |s| {
        let mut points: Vec<ScanPoint> = Vec::with_capacity(s.len() / SCAN_POINT_STRIDE + 1);
        let mut next_at = 0usize;
        form_scan(s, ScanPoint::start(), usize::MAX, |p| {
            if (p.byte as usize) >= next_at {
                points.push(p);
                next_at = p.byte as usize + SCAN_POINT_STRIDE;
            }
        });
        std::sync::Arc::new(FormScanIndex { points })
    });
    any.downcast::<FormScanIndex>().ok()
}

pub(super) fn scan_form_start_2(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pos = expect_int(heap, "scan-form-start-2", arg(args, 1))?;
    let (best, prev) = {
        let h: &Heap = heap;
        let v = arg(args, 0);
        let s = expect_string_ref(h, "scan-form-start-2", v)?;
        if s.is_empty() || pos < 0 {
            (0, 0)
        } else {
            let table = form_scan_table(h, v, s.len());
            form_start_pair(&s, table.as_deref(), pos as usize)
        }
    };
    let items = vec![Value::int(prev as i64), Value::int(best as i64)];
    Ok(heap.alloc_vector(items))
}

/// Forward from byte offset `start_byte` (whose char index is `start_ch`) in `s`,
/// skipping string and `;`-comment content and tracking bracket depth. Returns the char
/// offset just after `nforms` top-level (depth-returns-to-0) forms have completed, or the
/// char length of `s` if it ends first. A depth-0 open bracket begins a form; the close
/// that returns depth to 0 completes it. Every character this cares about (`"`, `\`, `;`,
/// `\n`, the brackets) is ASCII, so the pass matches bytes and carries the char index
/// alongside — the same technique as [`form_scan`].
///
/// This is the byte-native form of `tool/sexp`'s interpreted `sexp-scan`: same lexical
/// rules as `scan-tokens`/`scan-form-start` (`"` opens a string in which `\` escapes the
/// next char and which runs to the end if unterminated; `;` runs to end-of-line), but one
/// native pass over the ~3-form window instead of an O(window) `char-at` loop in Brood,
/// which was ~85% of every structural motion.
fn scan_form_end_bytes(s: &str, start_byte: usize, start_ch: usize, nforms: i64) -> usize {
    let b = s.as_bytes();
    let len = b.len();
    let mut byte = start_byte.min(len);
    let mut ch = start_ch;
    let mut depth: i64 = 0;
    let mut left = nforms;
    macro_rules! step {
        () => {{
            byte += utf8_width(b[byte]) as usize;
            ch += 1;
        }};
    }
    while byte < len {
        if depth == 0 && left <= 0 {
            break;
        }
        match b[byte] {
            b'"' => {
                step!(); // past the opening quote
                while byte < len {
                    match b[byte] {
                        b'\\' => {
                            step!();
                            if byte < len {
                                step!();
                            }
                        }
                        b'"' => {
                            step!();
                            break;
                        }
                        _ => step!(),
                    }
                }
            }
            b';' => {
                while byte < len && b[byte] != b'\n' {
                    step!();
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                step!();
            }
            b')' | b']' | b'}' => {
                depth = (depth - 1).max(0);
                if depth == 0 {
                    left -= 1;
                }
                step!();
            }
            _ => step!(),
        }
    }
    ch
}

/// `(scan-form-end s from n-forms)` — the char offset just after `n-forms` top-level forms
/// starting at char offset `from`, skipping strings/comments and tracking bracket depth, or
/// `(string/length s)` if the text ends first. The forward window-end companion to
/// `scan-form-start`: `tool/sexp`'s `narrow` uses the pair to bound structural motion to the
/// neighbourhood of point (previous, enclosing, next form) in one native pass, where it used
/// an interpreted `char-at` loop — the dominant cost of every keystroke-driven motion.
pub(super) fn scan_form_end(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = expect_int(heap, "scan-form-end", arg(args, 1))?.max(0) as usize;
    let nforms = expect_int(heap, "scan-form-end", arg(args, 2))?;
    let h: &Heap = heap;
    let v = arg(args, 0);
    let s = expect_string_ref(h, "scan-form-end", v)?;
    // `from` is a char index; convert to a byte offset in O(1)+bounded via the string's
    // ADR-213 char↔byte index (a fresh `Vec<char>` walk would reintroduce the O(pos) cost
    // this primitive exists to remove). A non-`Str` value can't reach here — the ref check
    // already errored — but fall back to a linear conversion to keep the match total.
    let start_byte = match v {
        Value::Str(id) => h.str_char_to_byte(id, from),
        _ => s.char_indices().nth(from).map_or(s.len(), |(b, _)| b),
    };
    let end = scan_form_end_bytes(&s, start_byte, from, nforms);
    Ok(Value::int(end as i64))
}

pub(super) fn scan_form_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let pos = expect_int(heap, "scan-form-start", arg(args, 1))?;
    let h: &Heap = heap;
    let v = arg(args, 0);
    let s = expect_string_ref(h, "scan-form-start", v)?;
    if s.is_empty() || pos < 0 {
        return Ok(Value::int(0));
    }
    let table = form_scan_table(h, v, s.len());
    let (best, _) = form_start_pair(&s, table.as_deref(), pos as usize);
    Ok(Value::int(best as i64))
}

/// Append the run `[lo, hi)` (absolute offsets; `base` is the text's first char) in
/// `face` to `runs`, coalescing into the previous run when the faces are `equal` — the
/// runs partition the line contiguously, so coalescing just extends the last run's end.

pub(super) fn span_runs_push(
    runs: &mut Vec<(usize, usize, Value)>,
    base: i64,
    lo: i64,
    hi: i64,
    face: Value,
    heap: &Heap,
) {
    if hi <= lo {
        return;
    }
    // `lo`/`hi` are absolute offsets >= `base` by construction; `saturating_sub`
    // keeps the relative index non-negative even if a caller ever violated that,
    // so the host can't panic on an underflow.
    let lhi = hi.saturating_sub(base) as usize;
    if let Some(last) = runs.last_mut() {
        if heap.equal(last.2, face) {
            last.1 = lhi;
            return;
        }
    }
    runs.push((lo.saturating_sub(base) as usize, lhi, face));
}

/// Merge face `b` over face `a` (`b` wins on key conflict), as Brood's `(into a b)` —
/// the overlay-merge the fontifier does to paint a region/isearch face on top of a
/// syntax face. A nil face is the identity; two maps merge `b`'s entries into `a`.
pub(super) fn merge_faces(heap: &mut Heap, a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Nil, _) => b,
        (_, Value::Nil) => a,
        (Value::Map(ai), Value::Map(bi)) => {
            let entries = heap.map_entries(bi);
            heap.map_from_pairs_into(ai, entries)
        }
        _ => b,
    }
}

/// Read a `[start end face]` span/range list into `(start, end, face)` tuples (handles
/// at offsets outside the window are kept; the tilers clip them).
pub(super) fn read_spans(
    heap: &Heap,
    who: &str,
    v: Value,
) -> Result<Vec<(i64, i64, Value)>, LispError> {
    let items = heap.seq_items(v)?;
    let mut out = Vec::with_capacity(items.len());
    for sv in &items {
        let parts = match sv {
            Value::Vector(id) => heap.vector(*id).to_vec(),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be a [start end face] vector",
                    who
                )))
            }
        };
        match (parts.first(), parts.get(1), parts.get(2)) {
            (Some(Value::Int(s)), Some(Value::Int(e)), Some(f)) => out.push((*s, *e, *f)),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be [int int face]",
                    who
                )))
            }
        }
    }
    Ok(out)
}

/// `(span-runs text base spans [ranges])` — tile `text` (its first char at offset
/// `base`) into a list of `[substring face]` runs. From ascending, non-overlapping
/// `[start end face]` `spans`: each gap is a nil-faced run, each span its text in its
/// face. With an optional overlay `ranges` channel (`[lo hi face]`, may overlap /
/// be unordered), each char's face is its span face with every covering range face
/// merged on top (later ranges win) — the region / isearch / bracket overlays. Adjacent
/// equal-face runs coalesce. This is the fontifier's span→runs tiler (`std/editor/
/// highlight`'s `fontify-runs`) in Rust — it runs per visible line every frame. Faces
/// are opaque maps, merged via `into` semantics and compared with `equal` to coalesce.
pub(super) fn span_runs(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = expect_string(heap, "span-runs", arg(args, 0))?;
    let base = expect_int(heap, "span-runs", arg(args, 1))?;
    let spans = read_spans(heap, "span-runs", arg(args, 2))?;
    let ranges = match args.get(3) {
        Some(r) => read_spans(heap, "span-runs", *r)?,
        None => Vec::new(),
    };
    let chars: Vec<char> = text.chars().collect();
    // `base` is caller-controlled (any i64); guard the absolute end against i64
    // overflow so a Lisp program can't panic the host. With a valid `end`, every
    // `lo`/`hi` handed to `span_runs_push` is provably in `[base, end]`.
    let end = base.checked_add(chars.len() as i64).ok_or_else(|| {
        LispError::runtime(format!(
            "span-runs: base {base} plus text length {} overflows i64",
            chars.len()
        ))
        .with_code(crate::error::error_codes::INDEX_OUT_OF_RANGE)
    })?;
    let mut runs: Vec<(usize, usize, Value)> = Vec::new();

    if ranges.is_empty() {
        // fast path: no overlay merge — emit gaps + spans left-to-right.
        let mut cur = base;
        for (s, e, f) in spans {
            if e <= base {
                continue;
            }
            if s >= end {
                break; // ascending spans: the rest are past the window
            }
            let lo = s.max(cur);
            let hi = e.min(end);
            if lo > cur {
                span_runs_push(&mut runs, base, cur, lo, Value::Nil, heap);
            }
            span_runs_push(&mut runs, base, lo, hi, f, heap);
            cur = hi;
        }
        if cur < end {
            span_runs_push(&mut runs, base, cur, end, Value::Nil, heap);
        }
    } else {
        // overlay path: tile by the union of span + range edges, merging faces per
        // segment. O(segments) — segments, not chars — so a region over the viewport is
        // as cheap as plain syntax, not a per-character merge.
        let mut bounds: Vec<i64> = vec![base, end];
        for (s, e, _) in spans.iter().chain(ranges.iter()) {
            if *e > base && *s < end {
                bounds.push((*s).max(base));
                bounds.push((*e).min(end));
            }
        }
        bounds.sort_unstable();
        bounds.dedup();
        let mut si = 0usize; // monotonic span cursor (spans are ascending)
        for w in bounds.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b <= a {
                continue;
            }
            while si < spans.len() && spans[si].1 <= a {
                si += 1;
            }
            let span_face = if si < spans.len() && spans[si].0 <= a && a < spans[si].1 {
                spans[si].2
            } else {
                Value::nil()
            };
            let mut rf = Value::nil();
            for (lo, hi, f) in &ranges {
                if *lo <= a && a < *hi {
                    rf = merge_faces(heap, rf, *f);
                }
            }
            let face = merge_faces(heap, span_face, rf);
            span_runs_push(&mut runs, base, a, b, face, heap);
        }
    }

    let n = chars.len();
    let out: Vec<Value> = runs
        .iter()
        .map(|&(lo, hi, f)| {
            // Clamp defensively: the run bounds are in-range by construction, but a
            // slice past `chars.len()` would panic the host — never let it.
            let seg: String = chars[lo.min(n)..hi.min(n)].iter().collect();
            let sv = heap.alloc_string(&seg);
            heap.alloc_vector(vec![sv, f])
        })
        .collect();
    Ok(heap.list_from_slice(&out))
}

/// OS clipboard access (the `clipboard` feature, via `arboard`). The handle lives in a
/// `OnceLock` for the whole process: on X11/Wayland the selection *owner* must stay
/// alive to answer paste requests, so a fresh handle per call would lose the copied text
/// the moment it dropped. Init failure (no display server) is cached as `None`, so the
/// builtins degrade to no-ops rather than retrying.
#[cfg(feature = "clipboard")]
mod clipboard {
    use arboard::Clipboard;
    use std::sync::{Mutex, OnceLock};
    static CB: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();
    fn handle() -> Option<&'static Mutex<Clipboard>> {
        CB.get_or_init(|| Clipboard::new().ok().map(Mutex::new))
            .as_ref()
    }
    pub fn get_text() -> Option<String> {
        handle()?.lock().ok()?.get_text().ok()
    }
    pub fn set_text(s: &str) {
        if let Some(m) = handle() {
            if let Ok(mut cb) = m.lock() {
                let _ = cb.set_text(s.to_owned());
            }
        }
    }
}

/// `(clipboard-get)` — the OS clipboard's text, or nil when it's empty / non-text /
/// unavailable (no display server, or a build without the `clipboard` feature). The
/// editor's yank consults this so text copied in another app pastes in.
pub(super) fn clipboard_get(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    #[cfg(feature = "clipboard")]
    if let Some(s) = clipboard::get_text() {
        return Ok(heap.alloc_string(&s));
    }
    #[cfg(not(feature = "clipboard"))]
    let _ = &heap;
    Ok(Value::nil())
}

/// `(clipboard-set! s)` — copy string `s` to the OS clipboard so other apps can paste
/// it; returns `s` (so it threads). A no-op (still returns `s`) when no clipboard is
/// available or the `clipboard` feature is off, so callers needn't special-case headless
/// builds. The editor's kill/copy commands call this so a kill is system-wide.
pub(super) fn clipboard_set(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "clipboard-set!", arg(args, 0))?;
    #[cfg(feature = "clipboard")]
    clipboard::set_text(&s);
    #[cfg(not(feature = "clipboard"))]
    let _ = &s;
    Ok(arg(args, 0))
}

#[cfg(test)]
mod form_scan_tests {
    use super::*;

    /// The pre-table implementation, kept verbatim as the oracle: a `Vec<char>` walk
    /// truncated at `pos + 1`. Every case below asserts the byte-level scan and its
    /// safepoint resume agree with THIS at every position — the definition of the
    /// primitive is what it used to answer, not what the new code thinks it should.
    fn oracle(text: &str, pos: usize) -> (usize, usize) {
        let chars: Vec<char> = text.chars().take(pos + 1).collect();
        let n = chars.len();
        if n == 0 {
            return (0, 0);
        }
        let pos = pos.min(n - 1);
        let (mut best, mut prev) = (0usize, 0usize);
        let mut i = 0usize;
        while i < n && i <= pos {
            match chars[i] {
                '"' => {
                    let mut j = i + 1;
                    while j < n {
                        match chars[j] {
                            '\\' => j += 2,
                            '"' => {
                                j += 1;
                                break;
                            }
                            _ => j += 1,
                        }
                    }
                    i = j.min(n);
                }
                ';' => {
                    while i < n && chars[i] != '\n' {
                        i += 1;
                    }
                }
                '(' | '[' | '{' if i == 0 || chars[i - 1] == '\n' => {
                    if i > 0 {
                        prev = best;
                    }
                    best = i;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        (best, prev)
    }

    /// Both engines at every char index of `text`: no table (the short-text path), and
    /// with a table built at a deliberately tiny stride so a resume happens between
    /// almost every pair of positions — the case a 4 KB stride would never reach in a
    /// test-sized fixture.
    fn agrees_everywhere(text: &str) {
        let nchars = text.chars().count();
        let mut points: Vec<ScanPoint> = Vec::new();
        let mut next_at = 0usize;
        form_scan(text, ScanPoint::start(), usize::MAX, |p| {
            if (p.byte as usize) >= next_at {
                points.push(p);
                next_at = p.byte as usize + 3; // 3-byte stride: resume constantly
            }
        });
        let table = FormScanIndex { points };
        for pos in 0..=nchars + 1 {
            let want = oracle(text, pos);
            assert_eq!(
                form_start_pair(text, None, pos),
                want,
                "no table, pos {} of {:?}",
                pos,
                text
            );
            assert_eq!(
                form_start_pair(text, Some(&table), pos),
                want,
                "resumed from a safepoint, pos {} of {:?}",
                pos,
                text
            );
        }
    }

    /// The shapes that make this lexer non-trivial: a column-0 bracket inside a string or
    /// a comment must NOT count, escapes must not end a string early, an unterminated
    /// string swallows the rest, and multi-byte text must not shift the char indices the
    /// language sees.
    #[test]
    fn agrees_with_the_pre_table_scan() {
        for text in [
            "",
            "(",
            "(a)\n(b)\n(c)\n",
            "(a)\n  (b)\n(c)",
            "[v]\n{m}\n(l)\n",
            "(a \"\n(not-a-form)\n\")\n(b)\n",
            "(a \";\n\")\n(b)\n",
            "; (not-a-form)\n(b)\n",
            "(a) ; trailing (comment)\n(b)\n",
            "(a \"\\\"\n(b)\n",             // escaped quote keeps the string open
            "(a \"\\\\\")\n(b)\n",          // escaped backslash closes it
            "(a \"unterminated\n(b)\n",     // runs to the end
            "(café)\n(naïve \"é\")\n(b)\n", // multi-byte, incl. inside a string
            "(a \"\\é\")\n(b)\n",           // escaped MULTI-BYTE char: 1 char, 2 bytes
            "🙂\n(a)\n🙂(b)\n",
            "\n\n(a)\n\n(b)\n\n",
            ";;\n;;\n(a)\n",
        ] {
            agrees_everywhere(text);
        }
    }

    /// A fixture past the real stride, so the production table (not just the 3-byte test
    /// one) is exercised, including a form start that lands exactly on a stride boundary.
    #[test]
    fn agrees_across_a_real_stride() {
        let mut text = String::new();
        while text.len() < SCAN_POINT_STRIDE * 3 {
            text.push_str(
                "(defn f (a b)\n  \"doc with a (bracket) and a ; semicolon\"\n  (+ a b))\n\n",
            );
        }
        let nchars = text.chars().count();
        let mut points: Vec<ScanPoint> = Vec::new();
        let mut next_at = 0usize;
        form_scan(&text, ScanPoint::start(), usize::MAX, |p| {
            if (p.byte as usize) >= next_at {
                points.push(p);
                next_at = p.byte as usize + SCAN_POINT_STRIDE;
            }
        });
        assert!(points.len() >= 3, "the fixture spans several strides");
        let table = FormScanIndex { points };
        // Every 7th position (an exhaustive sweep of 4 K chars x the oracle is needless).
        for pos in (0..nchars).step_by(7) {
            assert_eq!(
                form_start_pair(&text, Some(&table), pos),
                oracle(&text, pos),
                "pos {}",
                pos
            );
        }
    }
}
