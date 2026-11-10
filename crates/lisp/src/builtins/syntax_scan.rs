//! Lexical scanning over strings and ropes for editors: token runs, highlight spans and
//! form-boundary search, resumable from a safepoint.
use super::args::{arg, expect_int, expect_string, expect_string_ref};
use super::string::scan_bar;
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::{Sig, Ty};
    primitives.def(
        "%scan-tokens",
        Arity::exact(1),
        Sig::new(vec![string], vec_ty),
        &["s"],
        "Lexically tokenize Brood source s into a vector of [start end kind text] tokens (char offsets, end-exclusive; whitespace skipped). kind is :comment, :string, :number, :keyword, :symbol, :open, or :close. The lossless token stream a fontifier / structural tool walks — the per-char scan runs natively, leaving policy (faces, head-position) to the consumer over O(tokens).",
        scan_tokens);
    primitives.def(
        "%span-runs",
        Arity::range(3, 4),
        Sig::with_rest(vec![string, int, any], any, list_ty),
        &["text", "base", "spans", "ranges"],
        "Tile text (first char at offset base) into a list of [substring face] runs from ascending, non-overlapping [start end face] spans: gaps are nil-faced, each span its text in its face. With optional overlay ranges ([lo hi face], may overlap/be unordered) each char's face is its span face with every covering range face merged on top (later wins). Adjacent equal-face runs coalesce. The highlight span->runs tiler (fontify-runs), in Rust. Faces are opaque maps.",
        span_runs);
    // Registered LAST (not beside its scan-tokens sibling) on purpose: registration
    // order feeds the keyword/symbol intern table, and small-map key iteration order is
    // currently downstream of intern ids — an insertion mid-list reshuffles map key
    // order image-wide (record_test's field-order assertion catches it). Appending
    // preserves every existing id. The real fix is insertion-ordered map iteration.
    primitives.def(
        "%scan-form-start",
        Arity::exact(2),
        Sig::new(vec![string.union(rope), int], int),
        &["s", "pos"],
        "The greatest char offset <= pos of a top-level (bracket depth 0) open bracket in s lying OUTSIDE any string or ; comment, else 0 — the string/comment-aware beginning-of-defun behind highlight/safe-restart and tool/sexp narrowing. Depth-tracked, not the Emacs column-0 heuristic, so a mis-indented open bracket at column 0 inside an unclosed form is not a form start. The required forward lexical pass (a backward scan cannot know string state) runs natively: O(pos) at native speed instead of interpreted per-char cost on every eldoc/fontify-restart in a large buffer. s may be a string or a rope (ADR-488): a rope is read in place, and its safepoint table is carried across edits from the last ropes scanned, so a query after a keystroke re-scans about one form rather than the document.",
        scan_form_start);
    primitives.def(
        "%scan-form-start-2",
        Arity::exact(2),
        Sig::new(vec![string.union(rope), int], Ty::vector_of(int)),
        &["s", "pos"],
        "[prev start] — the greatest top-level (depth-0) form-start offset <= pos AND the one before it, from a SINGLE forward pass. What tool/sexp narrowing actually wants: computing the pair as two scan-form-start calls runs the O(pos) lexical pass twice over the same prefix. prev is 0 when there is no earlier form start, matching what the second call returned there. s may be a string or a rope, as for scan-form-start.",
        scan_form_start_2);
    primitives.def(
        "%scan-form-end",
        Arity::exact(3),
        Sig::new(vec![string.union(rope), int, int], int),
        &["s", "from", "n-forms"],
        "The char offset just after n-forms top-level forms starting at char offset from, skipping strings/comments and tracking bracket depth, or the length of s if the text ends first. s may be a string or a rope, read in place from from. The forward window-end companion to scan-form-start: tool/sexp narrowing uses the pair to bound structural motion to the neighbourhood of point in ONE native pass, replacing an interpreted char-at loop that was the dominant cost of every keystroke-driven motion.",
        scan_form_end);
}

/// The token kind a fontifier paints an atom in. Asks the reader's own classifier
/// (`atom::classify`) rather than keeping a second opinion on what a number looks
/// like: the previous `str::parse::<i64>() || parse::<f64>()` test could not see a
/// radix literal (`0xFF`), a ratio (`1/2`) or an `M`-decimal (`1.50M`), and took
/// Rust's `infinity` for a number when the reader reads it as a symbol (ADR-334).
///
/// A digit-led token the reader REFUSES (`0xZZ`, `1e`, `1_000` — reserved numeric
/// syntax, ADR-169) still colours as a number: it is number-shaped and malformed,
/// which is how every editor treats a bad literal, and the LSP carries the error.
/// Painting it as a name would be the one wrong answer.
pub(super) fn scan_atom_kind(t: &str) -> &'static str {
    use crate::syntax::atom::AtomKind;
    match crate::syntax::atom::classify(t) {
        AtomKind::Nil | AtomKind::Bool(_) | AtomKind::Keyword => "keyword",
        AtomKind::Symbol => "symbol",
        AtomKind::Int(_)
        | AtomKind::Float(_)
        | AtomKind::IntOverflow
        | AtomKind::Decimal
        | AtomKind::DecimalInvalid
        | AtomKind::Ratio
        | AtomKind::RatioInvalid
        | AtomKind::FloatInvalid
        | AtomKind::RadixOverflow
        | AtomKind::RadixInvalid
        | AtomKind::ReservedNumeric => "number",
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
    let s = expect_string(heap, "reflect/scan-tokens", arg(args, 0))?;
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

/// `(scan-form-start s pos)` — the greatest char offset ≤ `pos` of a **top-level**
/// (bracket depth 0) open bracket (`(`/`[`/`{`) in `s` lying OUTSIDE any string or `;`
/// comment, else 0. Depth-tracked, not the Emacs column-0 heuristic, so a mis-indented
/// open bracket at column 0 inside an unclosed form is not a form start (and an indented
/// top-level open is). The
/// string/comment-aware `beginning-of-defun` primitive behind `highlight/safe-restart`
/// and `tool/sexp`'s narrowing window: correctness requires a forward lexical pass from
/// the top (a backward scan cannot know whether a bracket sits inside a string without
/// the lexer state a forward pass carries), and that pass is O(pos) — ruinous per
/// keystroke in interpreted Brood on a large file (eldoc / fontify-restart / structural
/// motion all sit on it), trivial here. Strings honour `\\` escapes; a comment runs to
/// end-of-line — the same lexical rules as `scan-tokens`.
/// `(scan-form-start-2 s pos)` — `[prev start]`: the greatest top-level form-start offset
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
// ---- the top-level form-start lexer, and its safepoint table ----------------
//
// `scan-form-start` / `-2` answer "where does the top-level form containing `pos` begin"
// — the beginning-of-defun primitive under `tool/sexp`'s narrowing and
// `editor/highlight`'s `safe-restart`. Correctness requires a FORWARD lexical pass from
// offset 0: a backward scan cannot know whether a `(` sits inside a string or a comment.
// That makes one call O(pos) and a SEQUENCE of motions over one buffer O(n^2)
// (`scale_sweep.blsp`'s `sexp motions` row).
//
// Two things fix it, and only the second changes the shape:
//
//  1. The pass runs over BYTES, not a `Vec<char>` — materialising `pos + 1` `char`s per
//     call would allocate 4 MB for a motion near the bottom of a 1 MB file. Every
//     character this lexer cares about (`"`, `\`, `;`, `\n`, the brackets) is ASCII, so
//     byte matching is exact; the char index the language speaks is carried alongside,
//     incremented per char boundary.
//
//  2. A **safepoint table** cached against the string value ([`Heap::str_scan_table`]),
//     holding the lexer's answer every `SCAN_POINT_STRIDE` bytes. A query then resumes
//     from the last safepoint at or before `pos` instead of from 0, so a motion costs
//     O(stride) and a sequence over ONE text value is linear. Safepoints are only placed
//     where the lexer is between tokens, so the resumed state is always "in code" — a
//     string or comment is skipped atomically within one step, and one inside a long
//     literal simply gets no safepoint of its own.
//
// A string's table lives on the string, which helps whatever holds one text value across
// many queries: the sweep row, `nest check`-style tooling, an LSP/eldoc pass over an
// unchanged document. An editor is the other shape: its document is a ROPE, a new one per
// keystroke, and handing the scan `(buffer-text buf)` built a fresh string — and so missed
// the table — on every motion. The scans therefore read a rope in place too (ADR-488), and a
// rope's table is carried from the previous rope across the edit rather than cached on a
// value the next keystroke discards (see "the rope's table, carried across edits" below).

/// Bytes between safepoints. A 1 MB source gets ~256 of them (~5 KB of table), and a
/// query's residual scan is bounded by one stride rather than by `pos`.
const SCAN_POINT_STRIDE: usize = 4096;

/// Below this many bytes a query just scans from 0: the whole pass is already short, and
/// a table (plus its allocation) would cost more than it saves.
const SCAN_TABLE_MIN_BYTES: usize = 4096;

/// The lexer's state at a point it can resume from. `depth` is the bracket depth that
/// decides whether an open bracket starts a top-level form; `best`/`prev` are the answer
/// so far, so a resume needs no history before this point.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ScanPoint {
    byte: u32,
    ch: u32,
    depth: u32,
    best: u32,
    prev: u32,
}

impl ScanPoint {
    /// The state at the start of any text: depth 0, and "no form start seen" reports as
    /// 0 — which is also what a form start *at* char 0 reports, exactly as the pre-table
    /// implementation did.
    fn start() -> ScanPoint {
        ScanPoint {
            byte: 0,
            ch: 0,
            depth: 0,
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

/// The text a form scan reads, by byte: a string, or a rope read in place (ADR-488). The
/// lexer is written once against this, so a string and a rope of the same text give the
/// same answer by construction rather than by a second implementation kept in step.
trait ScanText {
    /// The length in bytes.
    fn len(&self) -> usize;
    /// The byte at `at` (`at < len`).
    fn byte(&mut self, at: usize) -> u8;
    /// Whether the character starting at byte `at` (`at < len`) ends an atom.
    fn delimiter_at(&mut self, at: usize) -> bool;
}

impl ScanText for &str {
    #[inline]
    fn len(&self) -> usize {
        str::len(self)
    }
    #[inline]
    fn byte(&mut self, at: usize) -> u8 {
        self.as_bytes()[at]
    }
    #[inline]
    fn delimiter_at(&mut self, at: usize) -> bool {
        self[at..]
            .chars()
            .next()
            .is_some_and(crate::syntax::atom::is_delimiter)
    }
}

/// A rope read through the chunk holding the last byte asked for. A scan moves forward a
/// byte at a time, so it looks a chunk up once per chunk it crosses and never copies the
/// text. ropey splits chunks on character boundaries, so a character never straddles two.
struct RopeCursor<'r> {
    rope: &'r ropey::Rope,
    chunk: &'r str,
    chunk_start: usize,
}

impl<'r> RopeCursor<'r> {
    fn new(rope: &'r ropey::Rope) -> RopeCursor<'r> {
        RopeCursor {
            rope,
            chunk: "",
            chunk_start: 0,
        }
    }

    /// The offset of byte `at` within the current chunk, moving to the chunk holding it.
    #[inline]
    fn local(&mut self, at: usize) -> usize {
        let offset = at.wrapping_sub(self.chunk_start);
        if offset < self.chunk.len() {
            return offset;
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(at);
        self.chunk = chunk;
        self.chunk_start = chunk_start;
        at - chunk_start
    }
}

impl ScanText for RopeCursor<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.rope.len_bytes()
    }
    #[inline]
    fn byte(&mut self, at: usize) -> u8 {
        let offset = self.local(at);
        self.chunk.as_bytes()[offset]
    }
    #[inline]
    fn delimiter_at(&mut self, at: usize) -> bool {
        let offset = self.local(at);
        self.chunk[offset..]
            .chars()
            .next()
            .is_some_and(crate::syntax::atom::is_delimiter)
    }
}

/// What [`lex`] stepped over: a bracket, or anything else (which never changes depth).
enum Lexeme {
    Open,
    Close,
    Other,
}

/// Step the cursor one character.
#[inline]
fn lex_step<T: ScanText>(text: &mut T, byte: &mut usize, ch: &mut usize) {
    *byte += utf8_width(text.byte(*byte));
    *ch += 1;
}

/// Step the cursor past a quoted run to its `close` byte (`\` escaping the next
/// character); an unterminated one runs to the end.
fn lex_quoted<T: ScanText>(text: &mut T, byte: &mut usize, ch: &mut usize, close: u8) {
    let len = text.len();
    while *byte < len {
        let c = text.byte(*byte);
        lex_step(text, byte, ch);
        if c == close {
            break;
        }
        if c == b'\\' && *byte < len {
            lex_step(text, byte, ch);
        }
    }
}

/// Step the cursor (`byte` and its char index `ch`, kept in step) past the token at it,
/// split as the reader splits it: a `"` string (to its closing quote, `\` escaping the
/// next character; an unterminated one runs to the end), a `;` comment (to end of line),
/// a `|…|` bar-quoted symbol or `:|…|` keyword (to the closing bar, `\` escaping), one
/// bracket, one delimiter, or an atom (to the next delimiter). The one lexer both form
/// scans share, so a bracket inside a string, comment or bar symbol never counts — and
/// since an atom is consumed whole, the cursor always rests between tokens, where a scan
/// may safely resume (a `|` *inside* an atom is not a bar). Every byte it matches is
/// ASCII; only an atom decodes characters, to know where it ends.
fn lex<T: ScanText>(text: &mut T, byte: &mut usize, ch: &mut usize) -> Lexeme {
    let len = text.len();
    match text.byte(*byte) {
        b'(' | b'[' | b'{' => {
            lex_step(text, byte, ch);
            return Lexeme::Open;
        }
        b')' | b']' | b'}' => {
            lex_step(text, byte, ch);
            return Lexeme::Close;
        }
        close @ (b'"' | b'|') => {
            lex_step(text, byte, ch);
            lex_quoted(text, byte, ch, close);
        }
        b':' if *byte + 1 < len && text.byte(*byte + 1) == b'|' => {
            lex_step(text, byte, ch);
            lex_step(text, byte, ch);
            lex_quoted(text, byte, ch, b'|');
        }
        b';' => {
            while *byte < len && text.byte(*byte) != b'\n' {
                lex_step(text, byte, ch);
            }
        }
        _ if text.delimiter_at(*byte) => lex_step(text, byte, ch),
        _ => {
            while *byte < len && !text.delimiter_at(*byte) {
                lex_step(text, byte, ch);
            }
        }
    }
    Lexeme::Other
}

/// The lexical pass, resumed from `from` and stopped at the first token past char index
/// `upto`. `emit` is called before each token (a safepoint-eligible position) so one pass
/// can both answer a query and build the table; it returns whether to go on, so a table
/// carried across an edit can stop where the old one takes over. Returns the state
/// reached.
///
/// Tokens are [`lex`]'s; an open bracket at bracket **depth 0** is a form start, where
/// the previous `best` becomes `prev`. Depth — not column 0 — is what makes the answer
/// exact on mis-indented source: an open bracket at column 0 *inside* an unclosed form
/// (the classic Emacs beginning-of-defun trap) is not a start, and an indented top-level
/// open is one. A stray close at depth 0 is ignored (saturating), so unbalanced text
/// degrades gracefully.
fn form_scan<T: ScanText>(
    text: &mut T,
    from: ScanPoint,
    upto: usize,
    mut emit: impl FnMut(ScanPoint) -> bool,
) -> ScanPoint {
    let mut state = from;
    let (mut byte, mut ch) = (from.byte as usize, from.ch as usize);
    while byte < text.len() && ch <= upto {
        (state.byte, state.ch) = (byte as u32, ch as u32);
        if !emit(state) {
            return state;
        }
        match lex(text, &mut byte, &mut ch) {
            Lexeme::Open => {
                if state.depth == 0 {
                    if state.ch > 0 {
                        state.prev = state.best;
                    }
                    state.best = state.ch;
                }
                state.depth += 1;
            }
            Lexeme::Close => state.depth = state.depth.saturating_sub(1),
            Lexeme::Other => {}
        }
    }
    (state.byte, state.ch) = (byte as u32, ch as u32);
    state
}

/// `(best, prev)` for char index `pos` in `text`, resuming from `table`'s last safepoint at
/// or before `pos` when there is one.
fn form_start_pair<T: ScanText>(
    text: &mut T,
    table: Option<&FormScanIndex>,
    pos: usize,
) -> (usize, usize) {
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
    let end = form_scan(text, from, pos, |_| true);
    (end.best as usize, end.prev as usize)
}

/// Every safepoint of `text`, a stride apart, from one pass.
fn build_form_scan_index<T: ScanText>(text: &mut T) -> FormScanIndex {
    let mut points: Vec<ScanPoint> = Vec::with_capacity(text.len() / SCAN_POINT_STRIDE + 1);
    let mut next_at = 0usize;
    form_scan(text, ScanPoint::start(), usize::MAX, |p| {
        if (p.byte as usize) >= next_at {
            points.push(p);
            next_at = p.byte as usize + SCAN_POINT_STRIDE;
        }
        true
    });
    FormScanIndex { points }
}

/// The safepoint table for string `id`, or `None` for a text short enough to rescan. Built
/// once per string value and cached on it.
fn form_scan_table(heap: &Heap, v: Value, len: usize) -> Option<std::sync::Arc<FormScanIndex>> {
    if len < SCAN_TABLE_MIN_BYTES {
        return None;
    }
    let Value::Str(id) = v else { return None };
    let any = heap.str_scan_table(id, |mut s| {
        std::sync::Arc::new(build_form_scan_index(&mut s))
    });
    any.downcast::<FormScanIndex>().ok()
}

// ---- the rope's table, carried across edits (ADR-488) -------------------------
//
// A string's table lives on the string, and an editor never asks the same string twice: its
// document is a rope, and every keystroke makes a new one. Building the table afresh per rope
// would make each query O(document) again — the cost this exists to remove. So the process
// keeps the tables of the last few ropes it scanned (`HeapExtras::form_scan_ropes`), and a new
// rope's table is DERIVED from the nearest of them: the changed range (common prefix and
// suffix, compared chunk by chunk — `%rope-changed-range`'s comparison) says which safepoints
// still hold. Those before the change are kept; the scan resumes at the last of them, runs
// over the change, and stops at the first old safepoint after it whose state it reproduces —
// the same depth, and the same last two form starts once moved by the edit. From an identical
// state over identical text the rest of the pass is identical, so the old table's remainder is
// taken, shifted. A keystroke inside a form re-scans to the start of the next form plus at
// most one stride, whatever the document's size. An edit that unbalances the text (a lone
// `(`) never reconverges and re-scans to the end: one native pass, exact, and no worse than
// building the table.

/// How many ropes' tables a process keeps: one per buffer being edited in it, give or take.
const FORM_SCAN_ROPES_KEPT: usize = 4;

/// `old`'s table moved onto `rope`, which differs from `old`'s text in chars
/// `[start, old_end)` → `[start, new_end)` (see the section comment). Also returns how many
/// states the pass visited — what a test reads to see the re-scan stayed local.
fn carry_form_scan_index(
    old: &FormScanIndex,
    old_rope: &ropey::Rope,
    rope: &ropey::Rope,
    (start, old_end, new_end): (usize, usize, usize),
) -> (FormScanIndex, usize) {
    let char_delta = rope.len_chars() as i64 - old_rope.len_chars() as i64;
    let byte_delta = rope.len_bytes() as i64 - old_rope.len_bytes() as i64;
    // A safepoint's state is decided by the text up to AND including the character at it
    // (the token before it ended there), so one AT `start` is not kept.
    let kept = old.points.partition_point(|p| (p.ch as usize) < start);
    let mut points: Vec<ScanPoint> = old.points[..kept].to_vec();
    let from = points.last().copied().unwrap_or_else(ScanPoint::start);
    let mut next_at = if kept > 0 {
        from.byte as usize + SCAN_POINT_STRIDE
    } else {
        0
    };
    // A form start the old table recorded, as an offset in the new text: unchanged before
    // the edit, shifted after it, and unknown inside it — or at 0 when the edit is at 0,
    // where 0 means "none yet" and a moved form start alike.
    let moved = |offset: u32| -> Option<u32> {
        let offset = offset as usize;
        if offset < start {
            Some(offset as u32)
        } else if offset >= old_end && offset > 0 {
            Some((offset as i64 + char_delta) as u32)
        } else {
            None
        }
    };
    let mut candidate = old.points.partition_point(|p| (p.ch as usize) < old_end);
    let mut takeover: Option<usize> = None;
    let mut visited = 0usize;
    form_scan(&mut RopeCursor::new(rope), from, usize::MAX, |p| {
        visited += 1;
        if (p.ch as usize) >= new_end {
            let old_ch = (p.ch as i64 - char_delta) as u32;
            while candidate < old.points.len() && old.points[candidate].ch < old_ch {
                candidate += 1;
            }
            if let Some(q) = old.points.get(candidate)
                && q.ch == old_ch
                && q.depth == p.depth
                && (q.byte as i64 + byte_delta) as u32 == p.byte
                && moved(q.best) == Some(p.best)
                && moved(q.prev) == Some(p.prev)
            {
                takeover = Some(candidate);
                return false;
            }
        }
        if (p.byte as usize) >= next_at {
            points.push(p);
            next_at = p.byte as usize + SCAN_POINT_STRIDE;
        }
        true
    });
    if let Some(first) = takeover {
        points.extend(old.points[first..].iter().map(|q| ScanPoint {
            byte: (q.byte as i64 + byte_delta) as u32,
            ch: (q.ch as i64 + char_delta) as u32,
            depth: q.depth,
            best: moved(q.best).unwrap_or(q.best),
            prev: moved(q.prev).unwrap_or(q.prev),
        }));
    }
    (FormScanIndex { points }, visited)
}

/// The safepoint table for `rope`, or `None` for a text short enough to rescan: this
/// process's table for the same text when it has one, else one carried from the nearest
/// rope it kept (see the section comment), else a fresh pass.
fn rope_form_scan_table(
    heap: &mut Heap,
    rope: &ropey::Rope,
) -> Option<std::sync::Arc<FormScanIndex>> {
    if rope.len_bytes() < SCAN_TABLE_MIN_BYTES {
        return None;
    }
    let kept = &mut heap.extras_mut().form_scan_ropes;
    let mut nearest: Option<(usize, (usize, usize, usize))> = None;
    for (at, (old_rope, _)) in kept.iter().enumerate() {
        match super::editor_native::changed_chars(old_rope, rope) {
            None => {
                let entry = kept.remove(at);
                let table = entry.1.clone();
                kept.insert(0, entry);
                return table.downcast::<FormScanIndex>().ok();
            }
            Some(change) => {
                let span = |(start, _, new_end): (usize, usize, usize)| new_end - start;
                if nearest.is_none_or(|(_, best)| span(change) < span(best)) {
                    nearest = Some((at, change));
                }
            }
        }
    }
    let carried = nearest.and_then(|(at, change)| {
        let (old_rope, old_table) = &kept[at];
        let old = old_table.downcast_ref::<FormScanIndex>()?;
        Some(carry_form_scan_index(old, old_rope, rope, change).0)
    });
    let table = std::sync::Arc::new(
        carried.unwrap_or_else(|| build_form_scan_index(&mut RopeCursor::new(rope))),
    );
    kept.insert(0, (rope.clone(), table.clone()));
    kept.truncate(FORM_SCAN_ROPES_KEPT);
    Some(table)
}

/// `(best, prev)` for `pos` in the text `v` — a string (its table cached on it) or a rope
/// (its table carried across edits). `who` names the caller in a type error.
fn form_start_pair_of(
    heap: &mut Heap,
    who: &str,
    v: Value,
    pos: i64,
) -> Result<(usize, usize), LispError> {
    if let Value::Rope(id) = v {
        // An `Arc` clone, so the heap is free for the table cache.
        let rope = heap.rope(id).clone();
        if rope.len_bytes() == 0 || pos < 0 {
            return Ok((0, 0));
        }
        let table = rope_form_scan_table(heap, &rope);
        return Ok(form_start_pair(
            &mut RopeCursor::new(&rope),
            table.as_deref(),
            pos as usize,
        ));
    }
    let h: &Heap = heap;
    let mut s = expect_text_string(h, who, v)?;
    if s.is_empty() || pos < 0 {
        return Ok((0, 0));
    }
    let table = form_scan_table(h, v, s.len());
    Ok(form_start_pair(&mut s, table.as_deref(), pos as usize))
}

/// The string `v`, or a type error naming both texts a form scan reads.
fn expect_text_string<'h>(heap: &'h Heap, who: &str, v: Value) -> Result<&'h str, LispError> {
    match v {
        Value::Str(_) => expect_string_ref(heap, who, v),
        other => Err(LispError::wrong_type(heap, who, "string or rope", other)),
    }
}

pub(super) fn scan_form_start_2(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "reflect/scan-form-start-2";
    let pos = expect_int(heap, who, arg(args, 1))?;
    let (best, prev) = form_start_pair_of(heap, who, arg(args, 0), pos)?;
    let items = vec![Value::int(prev as i64), Value::int(best as i64)];
    Ok(heap.alloc_vector(items))
}

/// Forward from byte offset `start_byte` (whose char index is `start_ch`) in `text` over
/// [`lex`]'s tokens, tracking bracket depth. Returns the char offset just after `nforms`
/// top-level (depth-returns-to-0) forms have completed, or the char length of `text` if it
/// ends first. A depth-0 open bracket begins a form; the close that returns depth to 0
/// completes it.
///
/// This is the byte-native form of `tool/sexp`'s interpreted `sexp-scan` — one native
/// pass over the ~3-form window instead of an O(window) `char-at` loop in Brood, which
/// was ~85% of every structural motion.
fn scan_form_end_bytes<T: ScanText>(
    text: &mut T,
    start_byte: usize,
    start_ch: usize,
    nforms: i64,
) -> usize {
    let len = text.len();
    let mut byte = start_byte.min(len);
    let mut ch = start_ch;
    let mut depth: i64 = 0;
    let mut left = nforms;
    while byte < len && !(depth == 0 && left <= 0) {
        match lex(text, &mut byte, &mut ch) {
            Lexeme::Open => depth += 1,
            Lexeme::Close => {
                depth = (depth - 1).max(0);
                if depth == 0 {
                    left -= 1;
                }
            }
            Lexeme::Other => {}
        }
    }
    ch
}

/// `(scan-form-end s from n-forms)` — the char offset just after `n-forms` top-level forms
/// starting at char offset `from`, skipping strings/comments and tracking bracket depth, or
/// the text's length if it ends first. The forward window-end companion to
/// `scan-form-start`: `tool/sexp`'s `narrow` uses the pair to bound structural motion to the
/// neighbourhood of point (previous, enclosing, next form) in one native pass, where it used
/// an interpreted `char-at` loop — the dominant cost of every keystroke-driven motion. A
/// rope is read in place from `from`, so the cost is the window's, not the document's.
pub(super) fn scan_form_end(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "reflect/scan-form-end";
    let from = expect_int(heap, who, arg(args, 1))?.max(0) as usize;
    let nforms = expect_int(heap, who, arg(args, 2))?;
    let h: &Heap = heap;
    let v = arg(args, 0);
    if let Value::Rope(id) = v {
        let rope = h.rope(id);
        let from = from.min(rope.len_chars());
        let end = scan_form_end_bytes(
            &mut RopeCursor::new(rope),
            rope.char_to_byte(from),
            from,
            nforms,
        );
        return Ok(Value::int(end as i64));
    }
    let mut s = expect_text_string(h, who, v)?;
    // `from` is a char index; convert to a byte offset in O(1)+bounded via the string's
    // ADR-213 char↔byte index (a fresh `Vec<char>` walk would reintroduce the O(pos) cost
    // this primitive exists to remove).
    let start_byte = match v {
        Value::Str(id) => h.str_char_to_byte(id, from),
        _ => s.char_indices().nth(from).map_or(s.len(), |(b, _)| b),
    };
    let end = scan_form_end_bytes(&mut s, start_byte, from, nforms);
    Ok(Value::int(end as i64))
}

pub(super) fn scan_form_start(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "reflect/scan-form-start";
    let pos = expect_int(heap, who, arg(args, 1))?;
    let (best, _) = form_start_pair_of(heap, who, arg(args, 0), pos)?;
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
    if let Some(last) = runs.last_mut()
        && heap.equal(last.2, face)
    {
        last.1 = lhi;
        return;
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
                )));
            }
        };
        match (parts.first(), parts.get(1), parts.get(2)) {
            (Some(Value::Int(s)), Some(Value::Int(e)), Some(f)) => out.push((*s, *e, *f)),
            _ => {
                return Err(LispError::runtime(format!(
                    "{}: each span must be [int int face]",
                    who
                )));
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
    let text = expect_string(heap, "%span-runs", arg(args, 0))?;
    let base = expect_int(heap, "%span-runs", arg(args, 1))?;
    let spans = read_spans(heap, "%span-runs", arg(args, 2))?;
    let ranges = match args.get(3) {
        Some(r) => read_spans(heap, "%span-runs", *r)?,
        None => Vec::new(),
    };
    let chars: Vec<char> = text.chars().collect();
    // `base` is caller-controlled (any i64); guard the absolute end against i64
    // overflow so a Lisp program can't panic the host. With a valid `end`, every
    // `lo`/`hi` handed to `span_runs_push` is provably in `[base, end]`.
    let end = base.checked_add(chars.len() as i64).ok_or_else(|| {
        LispError::runtime(format!(
            "%span-runs: base {base} plus text length {} overflows i64",
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
            heap.alloc_vector2(sv, f)
        })
        .collect();
    Ok(heap.list_from_slice(&out))
}

#[cfg(test)]
mod form_scan_tests {
    use super::*;

    /// The obvious implementation, kept as the oracle: a `Vec<char>` walk truncated at
    /// `pos + 1`, same lexical rules as `form_scan` (a depth-0 open bracket outside
    /// strings/comments is a form start). Every case below asserts the byte-level scan
    /// and its safepoint resume agree with THIS at every position.
    fn oracle(text: &str, pos: usize) -> (usize, usize) {
        let chars: Vec<char> = text.chars().take(pos + 1).collect();
        let n = chars.len();
        if n == 0 {
            return (0, 0);
        }
        let pos = pos.min(n - 1);
        let (mut best, mut prev) = (0usize, 0usize);
        let mut depth = 0usize;
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
                '(' | '[' | '{' => {
                    if depth == 0 {
                        if i > 0 {
                            prev = best;
                        }
                        best = i;
                    }
                    depth += 1;
                    i += 1;
                }
                ')' | ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
            }
        }
        (best, prev)
    }

    /// The table `form_scan` builds over `text` at a `stride` of bytes.
    fn index_at_stride<T: ScanText>(text: &mut T, stride: usize) -> FormScanIndex {
        let mut points: Vec<ScanPoint> = Vec::new();
        let mut next_at = 0usize;
        form_scan(text, ScanPoint::start(), usize::MAX, |p| {
            if (p.byte as usize) >= next_at {
                points.push(p);
                next_at = p.byte as usize + stride;
            }
            true
        });
        FormScanIndex { points }
    }

    /// Both engines at every char index of `text`, over the string and over a rope of it:
    /// no table (the short-text path), and with a table built at a deliberately tiny stride
    /// so a resume happens between almost every pair of positions — the case a 4 KB stride
    /// would never reach in a test-sized fixture.
    fn agrees_everywhere(text: &str) {
        let nchars = text.chars().count();
        let rope = ropey::Rope::from_str(text);
        // 3-byte stride: resume constantly
        let table = index_at_stride(&mut &*text, 3);
        let rope_table = index_at_stride(&mut RopeCursor::new(&rope), 3);
        for pos in 0..=nchars + 1 {
            let want = oracle(text, pos);
            assert_eq!(
                form_start_pair(&mut &*text, None, pos),
                want,
                "no table, pos {} of {:?}",
                pos,
                text
            );
            assert_eq!(
                form_start_pair(&mut &*text, Some(&table), pos),
                want,
                "resumed from a safepoint, pos {} of {:?}",
                pos,
                text
            );
            assert_eq!(
                form_start_pair(&mut RopeCursor::new(&rope), Some(&rope_table), pos),
                want,
                "a rope, resumed from a safepoint, pos {} of {:?}",
                pos,
                text
            );
            let start_byte = text.char_indices().nth(pos).map_or(text.len(), |(b, _)| b);
            for forms in 1..=3 {
                assert_eq!(
                    scan_form_end_bytes(
                        &mut RopeCursor::new(&rope),
                        start_byte,
                        pos.min(nchars),
                        forms
                    ),
                    scan_form_end_bytes(&mut &*text, start_byte, pos.min(nchars), forms),
                    "form end over a rope, {} forms from {} of {:?}",
                    forms,
                    pos,
                    text
                );
            }
        }
    }

    /// The shapes that make this lexer non-trivial: a bracket inside a string or a
    /// comment must NOT count, an open bracket inside an UNCLOSED form must not count
    /// however it is indented (the mis-indented multi-arity defn that motivated depth
    /// tracking), escapes must not end a string early, an unterminated string swallows
    /// the rest, and multi-byte text must not shift the char indices the language sees.
    #[test]
    fn agrees_with_the_pre_table_scan() {
        for text in [
            "",
            "(",
            "(a)\n(b)\n(c)\n",
            "(a)\n  (b)\n(c)",
            "(defn fib\n((0) 0)\n((1) 1)\n((n) n)))\n(b)\n",
            ")\n(a)\n",
            "(a\n(b)\n(c)",
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
        let rope = ropey::Rope::from_str(&text);
        let table = build_form_scan_index(&mut text.as_str());
        let rope_table = build_form_scan_index(&mut RopeCursor::new(&rope));
        assert!(table.points.len() >= 3, "the fixture spans several strides");
        assert_eq!(
            table.points, rope_table.points,
            "a rope's table is the string's"
        );
        // Every 7th position (an exhaustive sweep of 4 K chars x the oracle is needless).
        for pos in (0..nchars).step_by(7) {
            assert_eq!(
                form_start_pair(&mut text.as_str(), Some(&table), pos),
                oracle(&text, pos),
                "pos {}",
                pos
            );
            assert_eq!(
                form_start_pair(&mut RopeCursor::new(&rope), Some(&rope_table), pos),
                oracle(&text, pos),
                "rope, pos {}",
                pos
            );
        }
    }

    /// A Brood-ish document past several strides, with the shapes a column-0 guess gets
    /// wrong: a docstring whose lines start with `(`, a comment holding brackets, and an
    /// escaped quote.
    fn carry_fixture(strides: usize) -> String {
        let mut text = String::new();
        let mut n = 0;
        while text.len() < SCAN_POINT_STRIDE * strides {
            text.push_str(&format!(
                "(defn f{n} (a b)\n  \"Doc for f{n}, with a\n(column-0 bracket) and a \\\" quote.\"\n  ; a (comment) here\n  (+ a [b {{:k \"é\"}}]))\n\n"
            ));
            n += 1;
        }
        text
    }

    /// Every state a fresh pass over `rope` passes through, by byte.
    fn true_states(rope: &ropey::Rope) -> std::collections::HashMap<u32, ScanPoint> {
        let mut states = std::collections::HashMap::new();
        form_scan(
            &mut RopeCursor::new(rope),
            ScanPoint::start(),
            usize::MAX,
            |p| {
                states.insert(p.byte, p);
                true
            },
        );
        states
    }

    /// A table carried across an edit holds only true states and answers as a fresh one.
    fn assert_carried(table: &FormScanIndex, rope: &ropey::Rope, label: &str) {
        let states = true_states(rope);
        for p in &table.points {
            assert_eq!(
                states.get(&p.byte),
                Some(p),
                "{label}: safepoint at byte {}",
                p.byte
            );
        }
        let text = rope.to_string();
        let nchars = rope.len_chars();
        for pos in (0..=nchars)
            .step_by(11)
            .chain([nchars.saturating_sub(1), nchars])
        {
            assert_eq!(
                form_start_pair(&mut RopeCursor::new(rope), Some(table), pos),
                form_start_pair(&mut text.as_str(), None, pos),
                "{label}: pos {pos}"
            );
        }
    }

    /// Edits made one after another, each table carried from the last: typing inside a
    /// form (the table takes over again at the next form), a `"` that turns the rest of the
    /// document into a string and the `"` that closes it, a lone `(` that never rebalances,
    /// a whole form inserted at the very start and removed from the very end, and a
    /// multi-byte character. Every carried table must hold only true states and answer as a
    /// fresh scan does.
    #[test]
    fn a_carried_table_matches_a_fresh_one() {
        let mut rope = ropey::Rope::from_str(&carry_fixture(4));
        let mut table = build_form_scan_index(&mut RopeCursor::new(&rope));
        let middle = rope.len_chars() / 2;
        let edits: Vec<(&str, usize, usize, &str)> = vec![
            ("type inside a form", middle, 0, "x"),
            ("delete inside a form", middle, 1, ""),
            ("open a string", 40, 0, "\""),
            ("close it again", 40, 1, ""),
            ("a lone open", middle, 0, "("),
            ("remove it", middle, 1, ""),
            ("a form at the start", 0, 0, "(new form)\n"),
            ("drop the start's form", 0, 11, ""),
            ("multi-byte", middle + 3, 0, "🙂é"),
            ("trim the end", usize::MAX, 30, ""),
            ("a column-0 bracket in a docstring", 100, 0, "\n(not a form"),
        ];
        for (label, at, removed, inserted) in edits {
            let old = rope.clone();
            let at = if at == usize::MAX {
                rope.len_chars() - removed
            } else {
                at
            };
            rope.remove(at..at + removed);
            rope.insert(at, inserted);
            let Some(change) = super::super::editor_native::changed_chars(&old, &rope) else {
                continue;
            };
            table = carry_form_scan_index(&table, &old, &rope, change).0;
            assert_carried(&table, &rope, label);
        }
    }

    /// The point of carrying: a keystroke inside one form re-scans that form and at most a
    /// stride, not the document — counted as the states the carried pass visits.
    #[test]
    fn a_keystroke_rescans_a_form_not_the_document() {
        let old = ropey::Rope::from_str(&carry_fixture(32));
        let table = build_form_scan_index(&mut RopeCursor::new(&old));
        let mut rope = old.clone();
        let at = old.len_chars() / 3;
        rope.insert(at, "x");
        let change = super::super::editor_native::changed_chars(&old, &rope).unwrap();
        let (carried, visited) = carry_form_scan_index(&table, &old, &rope, change);
        let total = true_states(&rope).len();
        assert!(
            visited * 8 < total,
            "a keystroke re-scanned {visited} of {total} states"
        );
        assert_carried(&carried, &rope, "one keystroke");
    }
}
