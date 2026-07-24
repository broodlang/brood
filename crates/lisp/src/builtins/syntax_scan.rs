// Editor syntax-scanning / span / highlight / clipboard builtins — extracted from
// sequences.rs (these are editor tooling, not string ops).
#![allow(unused_imports)]
use super::*;
use super::sequences::*;
use super::numeric::{arg, expect_int, expect_string};
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
pub(super) fn scan_form_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let s = expect_string(heap, "scan-form-start", arg(args, 0))?;
    let pos = expect_int(heap, "scan-form-start", arg(args, 1))?;
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n == 0 {
        return Ok(Value::int(0));
    }
    let pos = pos.clamp(0, (n - 1) as i64) as usize;
    let mut best = 0usize;
    let mut i = 0usize;
    while i < n && i <= pos {
        match chars[i] {
            '"' => {
                // skip the string body: \ escapes the next char; an unterminated
                // string swallows the rest (nothing below it can be a form start)
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
                best = i;
                i += 1;
            }
            _ => i += 1,
        }
    }
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
